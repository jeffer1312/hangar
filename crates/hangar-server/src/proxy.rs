//! Repasse ao Python de tudo que o hangar-server não atende sozinho, inclusive WebSocket,
//! upload e streaming. Nunca segue redirect: a resposta volta como veio.
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};

pub type HttpClient = Client<HttpConnector, Body>;

/// Abaixo dos 5 s em que o uvicorn fecha a conexão ociosa: reaproveitar uma que ele está
/// fechando derruba o pedido no meio e vira 502 na tela.
pub const POOL_IDLE: Duration = Duration::from_secs(3);

pub fn client() -> HttpClient {
    Client::builder(TokioExecutor::new())
        .pool_idle_timeout(POOL_IDLE)
        .pool_timer(TokioTimer::new())
        .build_http()
}

/// Cliente já resolvido pelo `TrustedHosts`. Vai ao Python como valor único do X-Forwarded-For:
/// o uvicorn confia em 127.0.0.1 (o Rust) e chega ao mesmo cliente que chegaria sem o Rust.
pub struct Forward {
    pub client_ip: String,
    pub https: bool,
}

const HOP: [&str; 7] = ["connection", "keep-alive", "proxy-connection", "transfer-encoding", "te", "trailer", "upgrade"];

pub async fn forward(http: &HttpClient, upstream: SocketAddr, mut req: Request, fwd: &Forward) -> Response {
    let upgrade = req.headers().contains_key(header::UPGRADE);
    let path = req.uri().path_and_query().map(|p| p.as_str().to_owned()).unwrap_or_else(|| "/".into());
    prepare(req.headers_mut(), fwd, upgrade);
    if upgrade {
        return forward_upgrade(upstream, req, &path).await;
    }
    let Ok(uri) = format!("http://{upstream}{path}").parse::<Uri>() else {
        return (StatusCode::BAD_REQUEST, "hangar-server: caminho inválido").into_response();
    };
    *req.uri_mut() = uri;
    match http.request(req).await {
        Ok(resp) => {
            let (mut parts, body) = resp.into_parts();
            for name in HOP {
                parts.headers.remove(name);
            }
            Response::from_parts(parts, Body::new(body))
        }
        Err(e) => bad_gateway(&e),
    }
}

fn prepare(h: &mut HeaderMap, fwd: &Forward, upgrade: bool) {
    for name in HOP {
        // No aperto de mão do WebSocket, Connection e Upgrade são o próprio pedido.
        if upgrade && (name == "connection" || name == "upgrade") {
            continue;
        }
        h.remove(name);
    }
    // Só o próprio hangar-server fala com as rotas internas.
    h.remove("x-hangar-internal");
    h.remove("x-forwarded-for");
    h.remove("x-forwarded-proto");
    if let Ok(v) = HeaderValue::from_str(&fwd.client_ip) {
        h.insert("x-forwarded-for", v);
    }
    h.insert("x-forwarded-proto", HeaderValue::from_static(if fwd.https { "https" } else { "http" }));
}

/// Conexão própria por WebSocket: o pedido vai em forma de origem e, com o 101, os dois lados
/// viram um cano de bytes.
async fn forward_upgrade(upstream: SocketAddr, mut req: Request, path: &str) -> Response {
    let Ok(uri) = path.parse::<Uri>() else {
        return (StatusCode::BAD_REQUEST, "hangar-server: caminho inválido").into_response();
    };
    *req.uri_mut() = uri;
    let client_side = hyper::upgrade::on(&mut req);
    let stream = match tokio::net::TcpStream::connect(upstream).await {
        Ok(s) => s,
        Err(e) => return bad_gateway(&e),
    };
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await {
        Ok(x) => x,
        Err(e) => return bad_gateway(&e),
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    let mut resp = match sender.send_request(req).await {
        Ok(r) => r,
        Err(e) => return bad_gateway(&e),
    };
    if resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        let (parts, body) = resp.into_parts();
        return Response::from_parts(parts, Body::new(body));
    }
    let upstream_side = hyper::upgrade::on(&mut resp);
    tokio::spawn(async move {
        let (Ok(c), Ok(u)) = tokio::join!(client_side, upstream_side) else { return };
        let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(c), &mut TokioIo::new(u)).await;
    });
    let (parts, _) = resp.into_parts();
    Response::from_parts(parts, Body::empty())
}

fn bad_gateway(e: &dyn std::fmt::Display) -> Response {
    tracing::warn!("repasse ao Python falhou: {e}");
    (StatusCode::BAD_GATEWAY, "hangar-server: o backend não respondeu").into_response()
}
