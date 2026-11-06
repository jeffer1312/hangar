//! Tudo que entra pelo Connect veio da internet, embora o socket diga loopback (o Caddy é local).
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::{ConnectInfo, Connected};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::Response;
use axum::serve::IncomingStream;
use futures_util::StreamExt;
use serde_json::json;
use tokio::net::TcpListener;

use super::devices::DeviceId;
use crate::auth::{self, COOKIE, COOKIE_HOST};
use crate::devices::CONNECT_PEER;
use crate::routes::{AppState, cors, on_connect_port};
use crate::session_write::json_response;
use crate::share::gate::{Watcher, is_websocket, preflight};

/// Servir com `into_make_service_with_connect_info::<ConnectConn>()` num `TcpListener` puro. O socket
/// real nunca vira `ConnectInfo<SocketAddr>`: rota fora de `rewrite_peer` falha, em vez de ver um
/// cliente local.
#[derive(Clone, Copy, Debug)]
pub struct ConnectConn;

impl Connected<IncomingStream<'_, TcpListener>> for ConnectConn {
    fn connect_info(_: IncomingStream<'_, TcpListener>) -> Self { Self }
}

/// Põe o par do Connect e apaga os `X-Forwarded-*`, que aqui ninguém de confiança escreveu.
pub async fn rewrite_peer(mut req: Request) -> Request {
    let forged: Vec<_> = req.headers().keys().filter(|name| name.as_str().starts_with("x-forwarded-")).cloned().collect();
    for name in forged {
        req.headers_mut().remove(name);
    }
    let peer: IpAddr = CONNECT_PEER.parse().expect("CONNECT_PEER é um endereço");
    req.extensions_mut().insert(ConnectInfo(SocketAddr::new(peer, 0)));
    req
}

/// Pedido de aparelho, depois da troca: as rotas que distinguem do dono de verdade leem esta extensão.
#[derive(Clone, Debug)]
pub struct DeviceCaller(pub DeviceId);

/// Em todo listener do dono, troca a credencial de aparelho pela senha dentro do processo. Na entrada
/// do Connect nega por padrão, antes de qualquer corpo e sem tocar no contador de `hangar_auth`.
pub async fn device_token_layer(State(st): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let connect = on_connect_port(&req);
    let https = req.extensions().get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|ConnectInfo(peer)| st.cfg.trusted.resolve(peer.ip(), req.headers()).1);
    let token = auth::presented_token(req.headers(), req.uri().query(), req.method(), https);
    if let Some(token) = &token && let Some(id) = st.connect_devices.verify(token) {
        return serve_device(&st, req, next, id, token, connect).await;
    }
    if !connect {
        return next.run(req).await;
    }
    if let Some(token) = &token {
        let (service, lookup) = (st.login_guests.clone(), token.clone());
        let guest = tokio::task::spawn_blocking(move || service.store.lookup(&lookup)).await.ok().flatten();
        if guest.is_some() {
            // Sobra só a credencial do convidado: um `?token=` junto seria um palpite grátis pela senha.
            rewrite_credentials(&mut req, |value| (value == token.as_slice()).then(|| value.to_vec()));
            return next.run(req).await;
        }
    }
    if is_public(req.method(), req.uri().path()) {
        rewrite_credentials(&mut req, |_| None);
        return next.run(req).await;
    }
    if let Some(response) = preflight(&req) {
        return response;
    }
    refusal(&req, StatusCode::UNAUTHORIZED, "erro_nao_autorizado", "unauthorized")
}

async fn serve_device(st: &AppState, mut req: Request, next: Next, id: DeviceId, token: &[u8], connect: bool) -> Response {
    if hands_out_owner_password(req.uri().path()) {
        return refusal(&req, StatusCode::FORBIDDEN, "connect_owner_secret_refused", "rota só do dono");
    }
    // A origem do Connect é de todos que entram por ela; quem encheu o contador não foi este aparelho.
    if connect && let Err(error) = st.auth.clear_origin(CONNECT_PEER).await {
        return auth::unavailable(error);
    }
    let owner = st.cfg.auth_token.as_bytes();
    rewrite_credentials(&mut req, |value| {
        if value == token { Some(owner.to_vec()) } else if connect { None } else { Some(value.to_vec()) }
    });
    let watcher = Watcher::device(st.connect_devices.clone(), id.clone());
    req.extensions_mut().insert(watcher.clone());
    req.extensions_mut().insert(DeviceCaller(id));
    let response = next.run(req).await;
    let sse = response.headers().get(header::CONTENT_TYPE).is_some_and(|v| v.as_bytes().starts_with(b"text/event-stream"));
    if !sse {
        return response;
    }
    let (parts, body) = response.into_parts();
    let stream = body.into_data_stream().take_until(async move { watcher.until_ended().await });
    Response::from_parts(parts, Body::from_stream(stream))
}

/// Caminho cru e exato: a variante codificada cai na recusa, nunca numa rota que decodifica diferente.
fn is_public(method: &Method, path: &str) -> bool {
    match path {
        "/api/peers/ping" | "/api/peers/prova" => method == Method::GET,
        "/api/connect/redeem" | "/api/connect/redeem/status" => method == Method::POST,
        // As rotas do sync que não exigem o dono: login próprio, e o resto vale pelo cookie `cp_sync`.
        _ => path.strip_prefix("/api/sync/").is_some_and(|rest| {
            matches!(rest, "status" | "register" | "prelogin" | "login" | "logout" | "vault" | "guests")
                || rest.strip_prefix("guests/").is_some_and(|tail| tail.ends_with("/delete"))
        }),
    }
}

/// Decodificado como `machine_peers::handle` decodifica; revogar o aparelho não desfaz a senha entregue.
fn hands_out_owner_password(path: &str) -> bool {
    let path = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
    let path = path.trim_end_matches('/');
    path == "/api/alcance/pareamento" || path.strip_prefix("/api/peers/").is_some_and(|rest| rest.ends_with("/token"))
}

fn refusal(req: &Request, status: StatusCode, code: &str, msg: &str) -> Response {
    let mut response = json_response(status, json!({"detail": {"code": code, "params": {}, "msg": msg}}));
    if is_websocket(req) {
        response.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("close"));
    }
    cors(req.headers(), response.headers_mut());
    response
}

/// Passa cada credencial do pedido (`Authorization`, `?token=` e os dois cookies) por `swap`; `None` apaga.
fn rewrite_credentials(req: &mut Request, swap: impl Fn(&[u8]) -> Option<Vec<u8>>) {
    let headers = req.headers_mut();
    let authorization: Vec<HeaderValue> = headers.get_all(header::AUTHORIZATION).iter().cloned().collect();
    headers.remove(header::AUTHORIZATION);
    for value in authorization {
        let bearer = value.as_bytes().strip_prefix(b"Bearer ");
        let next = swap(bearer.unwrap_or(value.as_bytes())).map(|token| match bearer {
            Some(_) => [b"Bearer ".as_slice(), token.as_slice()].concat(),
            None => token,
        });
        if let Some(Ok(value)) = next.map(|bytes| HeaderValue::from_bytes(&bytes)) {
            headers.append(header::AUTHORIZATION, value);
        }
    }
    let cookies: Vec<HeaderValue> = headers.get_all(header::COOKIE).iter().cloned().collect();
    headers.remove(header::COOKIE);
    for value in cookies {
        let next = match value.to_str() {
            Ok(raw) => rewrite_cookie(raw, &swap).map(String::into_bytes),
            Err(_) => swap(value.as_bytes()),
        };
        if let Some(Ok(value)) = next.map(|bytes| HeaderValue::from_bytes(&bytes)) {
            headers.append(header::COOKIE, value);
        }
    }
    rewrite_query(req, &swap);
}

fn rewrite_cookie(raw: &str, swap: &impl Fn(&[u8]) -> Option<Vec<u8>>) -> Option<String> {
    let kept: Vec<String> = raw.split(';').filter_map(|chunk| {
        let (name, value) = chunk.split_once('=').unwrap_or(("", chunk));
        let name = name.trim();
        if name != COOKIE && name != COOKIE_HOST {
            return Some(chunk.trim().to_owned());
        }
        let value = value.trim();
        let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
        swap(value.as_bytes()).map(|next| format!("{name}={}", String::from_utf8_lossy(&next)))
    }).collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

fn rewrite_query(req: &mut Request, swap: &impl Fn(&[u8]) -> Option<Vec<u8>>) {
    let Some(query) = req.uri().query() else { return };
    let kept: Vec<String> = query.split('&').filter_map(|pair| match form_urlencoded::parse(pair.as_bytes()).next() {
        Some((key, value)) if key == "token" => swap(value.as_bytes())
            .map(|next| format!("token={}", form_urlencoded::byte_serialize(&next).collect::<String>())),
        _ => Some(pair.to_owned()),
    }).collect();
    let path = req.uri().path();
    let target = if kept.is_empty() { path.to_owned() } else { format!("{path}?{}", kept.join("&")) };
    let mut parts = req.uri().clone().into_parts();
    parts.path_and_query = target.parse().ok();
    // Partes vindas de um `Uri` válido e valor codificado: não há como falhar de verdade.
    *req.uri_mut() = Uri::from_parts(parts).expect("caminho reescrito");
}
