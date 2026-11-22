//! Ponte local do vídeo de outra máquina. A libmpv pede a `http://127.0.0.1:<porta>/<id>` sem credencial nenhuma, e só
//! esta ponte fala com o servidor da sessão, com o token no cabeçalho. O ffmpeg escolhe o formato pelo conteúdo: um
//! `.mp4` que fosse manifesto (HLS/DASH), ou um redirecionamento, levaria a libmpv a outro endereço, e o token iria
//! junto se morasse nela.
use std::{fmt, net::Ipv4Addr, sync::Arc, time::Duration};
use reqwest::{Client, Method, StatusCode, Url, header};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}, runtime::Handle, task::{JoinHandle, JoinSet}};

const HEAD_LIMIT: usize = 16 * 1024;
const READ_LIMIT: Duration = Duration::from_secs(10);

/// Da resposta do servidor, a libmpv só recebe estes; cookie e o resto ficam aqui.
const PASSED: [&str; 4] = ["content-type", "content-length", "content-range", "accept-ranges"];

/// A rota do vídeo no servidor e a credencial dela. O `Debug` não mostra o token.
pub struct Upstream { url: Url, auth: header::HeaderValue }

impl Upstream {
    /// `None`: endereço que não é http(s) ou token que não cabe num cabeçalho.
    pub fn new(url: &str, token: &str) -> Option<Upstream> {
        let url = Url::parse(url).ok().filter(|url| matches!(url.scheme(), "http" | "https"))?;
        let mut auth = header::HeaderValue::from_str(&format!("Bearer {}", token.trim())).ok()?;
        auth.set_sensitive(true);
        Some(Upstream { url, auth })
    }
}

impl fmt::Debug for Upstream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Upstream").field("url", &self.url.as_str()).field("auth", &"<hidden>").finish()
    }
}

/// Vive enquanto o player vive: soltar para de escutar e corta os pedidos em andamento.
pub(super) struct Proxy { address: String, task: JoinHandle<()> }

impl Proxy {
    /// O que a libmpv abre: sem credencial, só um caminho que ninguém adivinha.
    pub(super) fn address(&self) -> &str { &self.address }
}

impl Drop for Proxy {
    fn drop(&mut self) { self.task.abort(); }
}

pub(super) fn start(runtime: &Handle, upstream: Upstream) -> std::io::Result<Proxy> {
    let id = random_id()?;
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    // O `from_std` precisa do reator do runtime; falhando aqui, o player mostra o erro em vez de esperar para sempre.
    let listener = { let _inside = runtime.enter(); TcpListener::from_std(listener)? };
    let address = format!("http://127.0.0.1:{}/{id}", listener.local_addr()?.port());
    // Redirecionamento não é seguido: o token só vai ao endereço da sessão.
    let client = Client::builder().connect_timeout(Duration::from_secs(10)).read_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none()).build().map_err(std::io::Error::other)?;
    let task = runtime.spawn(serve(listener, Arc::new(Shared { path: format!("/{id}"), upstream, client })));
    Ok(Proxy { address, task })
}

struct Shared { path: String, upstream: Upstream, client: Client }

async fn serve(listener: TcpListener, shared: Arc<Shared>) {
    // Abortar esta tarefa solta o `JoinSet`, que aborta cada pedido em andamento.
    let mut connections = JoinSet::new();
    loop {
        let socket = match listener.accept().await {
            Ok((socket, _)) => socket,
            // Sem descritor livre, por exemplo: espera em vez de girar.
            Err(_) => { tokio::time::sleep(Duration::from_millis(100)).await; continue; }
        };
        while connections.try_join_next().is_some() {}
        let shared = shared.clone();
        connections.spawn(async move { answer(socket, &shared).await });
    }
}

/// Um pedido por conexão (`Connection: close`): a busca do ffmpeg abre outra a cada salto.
async fn answer(mut socket: TcpStream, shared: &Shared) {
    let asked = match tokio::time::timeout(READ_LIMIT, read_head(&mut socket)).await.ok().flatten() {
        Some(head) => parse(&head, &shared.path),
        None => Err(StatusCode::BAD_REQUEST),
    };
    let asked = match asked {
        Ok(asked) => asked,
        Err(status) => { let _ = socket.write_all(empty(status).as_bytes()).await; return; }
    };
    let mut response = match request(&shared.client, &shared.upstream, &asked).send().await {
        Ok(response) => response,
        Err(error) => {
            crate::log_line(&format!("video proxy: upstream request to {} failed ({}): {}",
                shared.upstream.url.path(), error_kind(&error), error.without_url()));
            let _ = socket.write_all(empty(StatusCode::BAD_GATEWAY).as_bytes()).await;
            return;
        }
    };
    let status = response.status();
    if !status.is_success() && status != StatusCode::RANGE_NOT_SATISFIABLE {
        crate::log_line(&format!("video proxy: upstream {} answered {}", shared.upstream.url.path(), status.as_u16()));
        let shown = if status.is_redirection() { StatusCode::BAD_GATEWAY } else { status };
        let _ = socket.write_all(empty(shown).as_bytes()).await;
        return;
    }
    if socket.write_all(response_head(status, response.headers()).as_bytes()).await.is_err() || asked.head { return; }
    // Falha no meio corta a conexão: o corpo chega curto e a libmpv pede de novo dali.
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => if socket.write_all(&chunk).await.is_err() { return },
            Ok(None) => return,
            Err(error) => {
                crate::log_line(&format!("video proxy: upstream body from {} broke ({}): {}",
                    shared.upstream.url.path(), error_kind(&error), error.without_url()));
                return;
            }
        }
    }
}

/// O tipo da falha sem o endereço, que pode levar consulta.
fn error_kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() { "timeout" }
    else if error.is_connect() { "connect" }
    else if error.is_body() { "body" }
    else if error.is_decode() { "decode" }
    else if error.is_request() { "request" }
    else { "other" }
}

#[derive(Debug, PartialEq)]
struct Asked { head: bool, range: Option<String> }

/// Só `GET`/`HEAD` no caminho deste vídeo, sem consulta; o único cabeçalho que segue para o servidor é o `Range`.
fn parse(head: &str, path: &str) -> Result<Asked, StatusCode> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(target), Some(version)) = (first.next(), first.next(), first.next()) else { return Err(StatusCode::BAD_REQUEST) };
    if !version.starts_with("HTTP/1.") { return Err(StatusCode::BAD_REQUEST); }
    if !same(target.as_bytes(), path.as_bytes()) { return Err(StatusCode::NOT_FOUND); }
    let head = match method { "GET" => false, "HEAD" => true, _ => return Err(StatusCode::METHOD_NOT_ALLOWED) };
    let mut range = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        if !name.trim().eq_ignore_ascii_case("range") { continue; }
        let value = value.trim();
        if !valid_range(value) { return Err(StatusCode::BAD_REQUEST); }
        range = Some(value.to_owned());
    }
    Ok(Asked { head, range })
}

fn valid_range(value: &str) -> bool {
    value.strip_prefix("bytes=").is_some_and(|spec| !spec.is_empty() && spec.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b',' | b' ')))
}

fn request(client: &Client, upstream: &Upstream, asked: &Asked) -> reqwest::RequestBuilder {
    let builder = client.request(if asked.head { Method::HEAD } else { Method::GET }, upstream.url.clone())
        .header(header::AUTHORIZATION, upstream.auth.clone());
    match &asked.range { Some(range) => builder.header(header::RANGE, range.as_str()), None => builder }
}

fn status_line(status: StatusCode) -> String { format!("HTTP/1.1 {} {}\r\n", status.as_u16(), status.canonical_reason().unwrap_or("")) }

fn empty(status: StatusCode) -> String { format!("{}Content-Length: 0\r\nConnection: close\r\n\r\n", status_line(status)) }

fn response_head(status: StatusCode, headers: &header::HeaderMap) -> String {
    let mut out = status_line(status);
    for name in PASSED {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) { out.push_str(&format!("{name}: {value}\r\n")); }
    }
    out.push_str("Connection: close\r\n\r\n");
    out
}

async fn read_head(socket: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let read = socket.read(&mut chunk).await.ok().filter(|n| *n > 0)?;
        head.extend_from_slice(&chunk[..read]);
        if head.len() > HEAD_LIMIT { return None; }
    }
    String::from_utf8(head).ok()
}

fn same(a: &[u8], b: &[u8]) -> bool { a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0 }

fn random_id() -> std::io::Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_| std::io::Error::other("no randomness"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(url: &str) -> Upstream { Upstream::new(url, "fixture-owner").unwrap() }

    #[test]
    fn only_this_video_path_with_get_or_head() {
        let path = "/abc123";
        assert_eq!(parse("GET /abc123 HTTP/1.1\r\nHost: x\r\nRange: bytes=10-\r\n\r\n", path), Ok(Asked { head: false, range: Some("bytes=10-".into()) }));
        assert_eq!(parse("HEAD /abc123 HTTP/1.1\r\n\r\n", path), Ok(Asked { head: true, range: None }));
        for other in ["GET / HTTP/1.1", "GET /abc12 HTTP/1.1", "GET /abc123/x HTTP/1.1", "GET /abc123?x=1 HTTP/1.1", "GET http://evil/abc123 HTTP/1.1"] {
            assert_eq!(parse(&format!("{other}\r\n\r\n"), path), Err(StatusCode::NOT_FOUND), "{other}");
        }
        assert_eq!(parse("POST /abc123 HTTP/1.1\r\n\r\n", path), Err(StatusCode::METHOD_NOT_ALLOWED));
        assert_eq!(parse("GET /abc123 HTTP/1.1\r\nRange: lines=1\r\n\r\n", path), Err(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn upstream_request_carries_range_and_authorization() {
        let url = "http://server.invalid/api/sessions/s/uploads/v.mp4";
        let req = request(&Client::new(), &upstream(url), &Asked { head: false, range: Some("bytes=5-".into()) }).build().unwrap();
        assert_eq!(req.method(), &Method::GET);
        assert_eq!(req.url().as_str(), url);
        assert_eq!(req.headers()[header::AUTHORIZATION], "Bearer fixture-owner");
        assert_eq!(req.headers()[header::RANGE], "bytes=5-");
    }

    #[test]
    fn mpv_side_gets_only_the_media_headers() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "video/mp4".parse().unwrap());
        headers.insert(header::CONTENT_RANGE, "bytes 0-3/4".parse().unwrap());
        headers.insert(header::SET_COOKIE, "s=1".parse().unwrap());
        headers.insert(header::AUTHORIZATION, "Bearer fixture-owner".parse().unwrap());
        let head = response_head(StatusCode::PARTIAL_CONTENT, &headers);
        assert!(head.starts_with("HTTP/1.1 206 Partial Content\r\n"), "{head}");
        assert!(head.contains("content-type: video/mp4\r\n") && head.contains("content-range: bytes 0-3/4\r\n"), "{head}");
        let lower = head.to_ascii_lowercase();
        assert!(!lower.contains("authorization") && !lower.contains("fixture-owner") && !lower.contains("set-cookie"), "{head}");
        assert!(!format!("{:?}", upstream("http://s.invalid/x")).contains("fixture-owner"));
    }

    /// Servidor de mentira: entrega a cabeça de cada pedido e responde `reply`.
    async fn fake_server(reply: &'static str) -> (String, async_channel::Receiver<String>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let url = format!("http://127.0.0.1:{}/api/sessions/s/uploads/v.mp4", listener.local_addr().unwrap().port());
        let (tx, rx) = async_channel::unbounded();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let _ = tx.send(read_head(&mut socket).await.unwrap_or_default()).await;
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        (url, rx)
    }

    /// O que a libmpv faria: um pedido cru, sem credencial.
    async fn ask(address: &str, path: &str, extra: &str) -> String {
        let port = Url::parse(address).unwrap().port().unwrap();
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
        socket.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}\r\n").as_bytes()).await.unwrap();
        let mut out = Vec::new();
        socket.read_to_end(&mut out).await.unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn proxy_adds_the_token_only_upstream_and_refuses_other_paths() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (url, seen) = fake_server("HTTP/1.1 206 Partial Content\r\nContent-Type: video/mp4\r\nContent-Range: bytes 10-13/14\r\n\
                Content-Length: 4\r\nConnection: close\r\n\r\nabcd").await;
            let proxy = start(&Handle::current(), upstream(&url)).unwrap();
            assert!(proxy.address().starts_with("http://127.0.0.1:") && !proxy.address().contains("fixture-owner"));
            let refused = ask(proxy.address(), "/api/sessions/s/uploads/v.mp4", "").await;
            assert!(refused.starts_with("HTTP/1.1 404"), "{refused}");
            assert!(seen.is_empty(), "caminho recusado não chega ao servidor");
            let path = Url::parse(proxy.address()).unwrap().path().to_owned();
            let reply = ask(proxy.address(), &path, "Range: bytes=10-\r\n").await;
            assert!(reply.starts_with("HTTP/1.1 206 Partial Content\r\n") && reply.ends_with("\r\n\r\nabcd"), "{reply}");
            assert!(reply.contains("content-range: bytes 10-13/14\r\n") && !reply.contains("fixture-owner"), "{reply}");
            let head = seen.recv().await.unwrap().to_ascii_lowercase();
            assert!(head.starts_with("get /api/sessions/s/uploads/v.mp4 http/1.1\r\n"), "{head}");
            assert!(head.contains("authorization: bearer fixture-owner\r\n") && head.contains("range: bytes=10-\r\n"), "{head}");
        });
    }

    #[test]
    fn redirect_from_the_server_is_not_followed() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (url, seen) = fake_server("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            let proxy = start(&Handle::current(), upstream(&url)).unwrap();
            let path = Url::parse(proxy.address()).unwrap().path().to_owned();
            let reply = ask(proxy.address(), &path, "").await;
            assert!(reply.starts_with("HTTP/1.1 502"), "{reply}");
            assert_eq!(seen.len(), 1);
        });
    }
}
