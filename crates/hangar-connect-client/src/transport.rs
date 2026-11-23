//! O que sai do processo: pedidos HTTP e o relógio da espera pela aprovação. As provas trocam os dois.
use std::future::Future;
use std::time::{Duration, Instant};

use crate::error::{Code, Error};

const SECRET_HEADERS: [&str; 3] = ["authorization", "x-hangar-device-secret", "cookie"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Default)]
pub struct Response {
    pub status: u16,
    /// Os valores de `Set-Cookie`, um por linha da resposta.
    pub set_cookies: Vec<String>,
    pub body: Vec<u8>,
}

/// Rede e relógio. Pedido sem resposta volta como [`Error::network`]; redirecionamento volta como resposta
/// 3xx, sem ser seguido: a senha iria junto para outro endereço.
pub trait Transport: Send + Sync {
    fn send(&self, request: Request) -> impl Future<Output = Result<Response, Error>> + Send;
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
    fn now(&self) -> Instant;
}

/// O transporte de verdade, sobre o `reqwest` de quem chama (o servidor passa o `http_client` do core).
#[derive(Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    max_body: Option<usize>,
}

impl ReqwestTransport {
    /// Desliga o redirecionamento no `builder` recebido.
    ///
    /// # Errors
    ///
    /// [`crate::Code::NetworkFailed`] quando o `reqwest` não monta o cliente (TLS indisponível).
    pub fn new(builder: reqwest::ClientBuilder) -> Result<Self, Error> {
        let client = builder.redirect(reqwest::redirect::Policy::none()).build()
            .map_err(|_| Error::network("cliente HTTP indisponível"))?;
        Ok(Self { client, max_body: None })
    }

    /// Corpo de resposta maior que `limit` não é lido, nem em parte: volta [`Code::ConnectResponseInvalid`].
    pub fn with_max_body(mut self, limit: usize) -> Self {
        self.max_body = Some(limit);
        self
    }
}

impl Transport for ReqwestTransport {
    fn send(&self, request: Request) -> impl Future<Output = Result<Response, Error>> + Send {
        let (client, max_body) = (self.client.clone(), self.max_body);
        async move {
            let method = match request.method {
                Method::Get => reqwest::Method::GET,
                Method::Post => reqwest::Method::POST,
                Method::Put => reqwest::Method::PUT,
                Method::Delete => reqwest::Method::DELETE,
            };
            let mut builder = client.request(method, &request.url).timeout(request.timeout);
            for (name, value) in request.headers {
                let mut value = reqwest::header::HeaderValue::from_str(&value).map_err(|_| Error::network("cabeçalho inválido"))?;
                value.set_sensitive(SECRET_HEADERS.iter().any(|secret| secret.eq_ignore_ascii_case(name)));
                builder = builder.header(name, value);
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            let network = |error: reqwest::Error| Error::network(error.without_url().to_string());
            let mut response = builder.send().await.map_err(network)?;
            let status = response.status().as_u16();
            let set_cookies = response.headers().get_all(reqwest::header::SET_COOKIE).iter()
                .filter_map(|value| value.to_str().ok().map(str::to_owned)).collect();
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(network)? {
                if max_body.is_some_and(|limit| body.len() + chunk.len() > limit) {
                    return Err(Error::new(Code::ConnectResponseInvalid, None));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Response { status, set_cookies, body })
        }
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}


#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    // Cabeçalho e corpo podem chegar em leituras separadas.
    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let read = socket.read(&mut chunk).await.unwrap();
            raw.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&raw).into_owned();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end].lines()
                    .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                    .unwrap_or(0);
                if raw.len() >= end + 4 + length || read == 0 {
                    return text;
                }
            } else if read == 0 {
                return text;
            }
        }
    }

    // O único teste com socket: o resto do crate usa o dublê de `fake.rs`.
    #[tokio::test]
    async fn real_socket_returns_status_cookies_and_body_without_following_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut seen = Vec::new();
            for answer in [
                "HTTP/1.1 200 OK\r\nSet-Cookie: cp_sync=abc; HttpOnly; Path=/\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                seen.push(read_request(&mut socket).await);
                socket.write_all(answer.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
            seen
        });
        let transport = ReqwestTransport::new(reqwest::Client::builder()).unwrap();
        let request = |method, path: &str, body: Option<&str>| Request {
            method, url: format!("{base}{path}"), body: body.map(|b| b.as_bytes().to_vec()), timeout: Duration::from_secs(5),
            headers: vec![("Content-Type", "application/json".into()), ("Authorization", "Bearer t".into())],
        };

        let first = transport.send(request(Method::Post, "/api/sync/login", Some("{\"a\":1}"))).await.unwrap();
        assert_eq!((first.status, first.body.as_slice()), (200, &b"{\"ok\":true}"[..]));
        assert_eq!(first.set_cookies, ["cp_sync=abc; HttpOnly; Path=/"]);
        let second = transport.send(request(Method::Get, "/v1/machines", None)).await.unwrap();
        assert_eq!(second.status, 302, "o redirecionamento não pode ser seguido");

        let seen = server.await.unwrap();
        assert!(seen[0].starts_with("POST /api/sync/login HTTP/1.1"), "{}", seen[0]);
        assert!(seen[0].contains("{\"a\":1}"));
        assert!(seen[0].to_ascii_lowercase().contains("authorization: bearer t"));
        assert!(seen[1].starts_with("GET /v1/machines HTTP/1.1"), "{}", seen[1]);
    }

    #[tokio::test]
    async fn closed_port_is_network() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let transport = ReqwestTransport::new(reqwest::Client::builder()).unwrap();
        let error = transport.send(Request {
            method: Method::Get, url: format!("http://{address}/"), headers: Vec::new(), body: None, timeout: Duration::from_secs(5),
        }).await.unwrap_err();
        assert_eq!(error.code(), crate::Code::NetworkFailed);
    }
}
