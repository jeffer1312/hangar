//! A volta do login do proxy: o navegador desta máquina cai em `localhost:<porta>`, e o proxy pode estar noutra.
//! Enquanto o login espera, o app escuta a porta que o próprio link pede e entrega a volta ao servidor.
use super::*;
use std::{pin::Pin, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// O login do proxy expira antes disso; a porta não fica presa se a pessoa desistir sem fechar o assistente.
const LIFETIME: Duration = Duration::from_secs(15 * 60);
const HEAD_LIMIT: usize = 16 * 1024;
const READ_LIMIT: Duration = Duration::from_secs(5);

/// Para de escutar ao sair de cena.
pub(super) struct Relay(pub(super) tokio::task::JoinHandle<()>);

impl Drop for Relay {
    fn drop(&mut self) { self.0.abort(); }
}

/// Para onde o link manda o navegador voltar, e o `state` que a volta precisa trazer.
#[derive(Debug, PartialEq)]
pub(super) struct Return { port: u16, path: String, pub(super) state: String }

/// Vale para qualquer provedor do proxy: a porta e o caminho saem do `redirect_uri` do link.
pub(super) fn target(auth_url: &str) -> Option<Return> {
    let url = url::Url::parse(auth_url).ok()?;
    let param = |name: &str| url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned());
    let state = param("state").filter(|state| !state.is_empty())?;
    let back = url::Url::parse(&param("redirect_uri")?).ok()?;
    let local = back.scheme() == "http" && matches!(back.host_str(), Some("localhost" | "127.0.0.1"));
    Some(Return { port: back.port().filter(|_| local)?, path: back.path().to_owned(), state })
}

/// `None` = a porta já tem dono (o proxy está nesta máquina e recebe a volta ele mesmo): fica o colar.
pub(super) fn bind(back: &Return) -> Option<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, back.port)).ok()?;
    listener.set_nonblocking(true).ok()?;
    Some(listener)
}

pub(super) struct Pages { pub done: String, pub failed: String }

pub(super) async fn serve<D>(listener: std::net::TcpListener, back: Return, api: Api, done: D, pages: Pages)
where D: Fn(CliproxyReply) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + 'static {
    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else { return };
    let _ = tokio::time::timeout(LIFETIME, async {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let query = tokio::time::timeout(READ_LIMIT, read_head(&mut socket)).await.ok().flatten()
                .and_then(|head| matching_query(&head, &back));
            let Some(query) = query else {
                let _ = socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                continue;
            };
            let body = json!({"redirect_url": format!("http://localhost:{}{}?{query}", back.port, back.path)});
            let sent = api.server_send(reqwest::Method::POST, &["engines", "cliproxy", "login", &back.state, "callback"], Some(body), 30).await;
            let ok = sent.is_ok();
            done(CliproxyReply::Callback(back.state.clone(), sent)).await;
            let _ = socket.write_all(page(if ok { &pages.done } else { &pages.failed }).as_bytes()).await;
            // Falhou: a mesma página recarregada tenta de novo.
            if ok { return; }
        }
    }).await;
}

async fn read_head(socket: &mut tokio::net::TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let read = socket.read(&mut chunk).await.ok().filter(|n| *n > 0)?;
        head.extend_from_slice(&chunk[..read]);
        if head.len() > HEAD_LIMIT { return None; }
    }
    String::from_utf8(head).ok()
}

/// Só a volta deste login: `GET` no caminho do link e com o mesmo `state`. O resto (ícone, outro login) é 404.
fn matching_query(head: &str, back: &Return) -> Option<String> {
    let mut line = head.lines().next()?.split(' ');
    let (method, target) = (line.next()?, line.next()?);
    let (path, query) = target.split_once('?')?;
    let state = url::form_urlencoded::parse(query.as_bytes()).find(|(key, _)| key == "state").map(|(_, value)| value.into_owned());
    (method == "GET" && path == back.path && state.as_deref() == Some(back.state.as_str())).then(|| query.to_owned())
}

fn page(text: &str) -> String {
    let text = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let body = format!("<!doctype html><meta charset=\"utf-8\"><title>Hangar</title>\
        <body style=\"font-family:system-ui,sans-serif;display:grid;place-items:center;min-height:90vh;margin:0\"><p>{text}</p></body>");
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    const CODEX: &str = "https://auth.openai.com/oauth/authorize?client_id=x&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&state=abc";

    #[test]
    fn the_return_comes_from_the_link_of_any_provider() {
        assert_eq!(target(CODEX), Some(Return { port: 1455, path: "/auth/callback".into(), state: "abc".into() }));
        let claude = "https://claude.ai/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A54545%2Fcallback&state=s";
        assert_eq!(target(claude), Some(Return { port: 54545, path: "/callback".into(), state: "s".into() }));
        assert_eq!(target("https://x.test/a?redirect_uri=https%3A%2F%2Fevil.test%3A1455%2Fcb&state=s"), None);
        assert_eq!(target("https://x.test/a?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fcb"), None);
        assert_eq!(target("https://x.test/a?redirect_uri=http%3A%2F%2Flocalhost%2Fcb&state=s"), None);
    }

    #[test]
    fn only_the_return_of_this_login_passes() {
        let back = target(CODEX).unwrap();
        let get = |target: &str| format!("GET {target} HTTP/1.1\r\nHost: localhost:1455\r\n\r\n");
        assert_eq!(matching_query(&get("/auth/callback?code=c0de&state=abc"), &back).as_deref(), Some("code=c0de&state=abc"));
        assert_eq!(matching_query(&get("/auth/callback?code=c0de&state=outro"), &back), None);
        assert_eq!(matching_query(&get("/favicon.ico"), &back), None);
        assert_eq!(matching_query(&get("/callback?code=c0de&state=abc"), &back), None);
        assert_eq!(matching_query("POST /auth/callback?code=c&state=abc HTTP/1.1\r\n\r\n", &back), None);
    }
}
