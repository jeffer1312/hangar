//! Servidor do hangar-preview dentro do app nativo: mesmo contrato do `shell/preview_srv.cjs` (porta efêmera, token
//! sorteado, `~/.hangar/nav/_srv.json`), para o CLI, o backend e o Jev não mudarem.
use std::{path::PathBuf, sync::Arc, time::Duration};

use futures::channel::oneshot;
use serde_json::{Value, json};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream, runtime::Runtime};

use super::{control::{Reply, Secret}, preview_fmt::sidecar_name};

const HEAD_MAX: usize = 16 * 1024;
const BODY_MAX: usize = 128 * 1024;
/// O verbo mais lento (wait, eval, shot) tem teto de 15 s; este cobre a fila de dois deles.
const REPLY_SECS: u64 = 45;
/// Abaixo dos 120 s em que o Bash do Claude Code mata um comando.
const LOGIN_SECS: u64 = 110;
/// Quanto um `_srv.json` órfão pode durar com este app aberto.
const RECLAIM_EVERY: Duration = Duration::from_secs(3);

pub enum Request {
    /// Verbo do `/cmd`, o do hangar-preview.
    /// `secret`: o `valor` do corpo (senha do cofre do `fill-secret`), fora dos `args`.
    Command { key: String, verb: String, args: Vec<String>, tab: Option<i64>, secret: Option<Secret>, reply: oneshot::Sender<Reply> },
    /// Comando CDP do `/cdp`, já conferido contra `relay::RELAYED`.
    Cdp { key: String, method: String, params: Value, reply: oneshot::Sender<Result<Value, String>> },
    /// Espectador novo da tela remota; tira o anterior daquele navegador.
    Watch { key: String, viewer: super::relay::Viewer, reply: oneshot::Sender<Result<(), String>> },
    /// O espectador `id` saiu; não vale se outro já o substituiu.
    Unwatch { key: String, id: u64 },
    /// Pipe do Chromium e página visível da sessão, para o cliente CDP de fora (`/cdp/browser`).
    #[cfg(target_os = "linux")]
    Pipe { key: String, reply: oneshot::Sender<Result<(super::chromium::pipe::Port, String), String>> },
    /// Sessão `name` do servidor desta máquina para o `/cdp/browser/<nome>` sem token: abre o navegador dela se ainda
    /// não houver e devolve a chave; `(status, texto)` quando não dá.
    #[cfg(target_os = "linux")]
    Open { name: String, reply: oneshot::Sender<Result<String, (u16, String)>> },
}

#[derive(Debug)]
struct Head {
    method: String, path: String, auth: Option<String>, length: usize, upgrade: Option<String>, ws_key: Option<String>,
    origin: Option<String>, host: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Route { Cmd, Cdp(String), Browser(String), Local(String) }

/// `cdp`: a página é de fora do app (`hangar-preview --cdp`), e `alvo` escolhe qual; aí a chave de sessão não entra.
struct Body { key: String, verb: String, args: Vec<String>, tab: Option<i64>, cdp: Option<String>, alvo: Option<String>, secret: Option<Secret> }

fn nav_dir() -> Option<PathBuf> { std::env::home_dir().map(|h| h.join(".hangar").join("nav")) }

/// Escrita em tmp + rename: o CLI nunca lê um arquivo pela metade.
fn write_json(name: &str, value: &Value) -> std::io::Result<()> {
    write_json_in(&nav_dir().ok_or_else(|| std::io::Error::other("sem pasta do usuario"))?, name, value)
}

fn write_json_in(dir: &std::path::Path, name: &str, value: &Value) -> std::io::Result<()> {
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    // O `_srv.json` leva o token: só o dono lê, como no shell Electron.
    #[cfg(unix)]
    {
        use std::{io::Write, os::unix::fs::{DirBuilderExt, OpenOptionsExt}};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?.write_all(value.to_string().as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)?;
        std::fs::write(&tmp, value.to_string())?;
    }
    std::fs::rename(tmp, dir.join(format!("{name}.json")))
}

/// Outra execução do app com outra pasta de configuração (teste, prova de tela) escapa da trava de janela única,
/// publica o servidor dela neste arquivo e, ao morrer, deixa o CLI e o MCP numa porta sem dono. `true` = retomado.
async fn reclaim(dir: &std::path::Path, own: &Value) -> std::io::Result<bool> {
    let current = std::fs::read(dir.join("_srv.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    if current.as_ref() == Some(own) { return Ok(false); }
    let other_pid = current.as_ref().and_then(|v| v["pid"].as_u64()).filter(|p| Some(*p) != own["pid"].as_u64());
    match other_pid.and_then(pid_alive) {
        Some(true) => return Ok(false),
        Some(false) => {}
        // ponytail: sem pid legível, porta que atende = dono vivo; porta reciclada fica sem retomada até o app reabrir.
        None => {
            let other = current.as_ref().and_then(|v| v["porta"].as_u64()).filter(|p| Some(*p) != own["porta"].as_u64())
                .and_then(|p| u16::try_from(p).ok());
            if let Some(port) = other
                && matches!(tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(("127.0.0.1", port))).await, Ok(Ok(_))) {
                return Ok(false);
            }
        }
    }
    write_json_in(dir, "_srv", own)?;
    Ok(true)
}

fn now_ms() -> u128 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()) }

/// Sidecar no formato do Electron (`url`/`targetId` no topo, `abas` ao lado), com o `pid` do dono para o `list`.
pub fn write_sidecar(key: &str, url: &str, title: &str) {
    let url = &without_token(url);
    let tab = json!({"id": 1, "url": url, "titulo": title, "targetId": null});
    let mut value = json!({"chave": key, "url": url, "targetId": null, "ts": now_ms(), "ativa": 1, "abas": [tab], "pid": std::process::id()});
    // Endereço do CDP de navegador inteiro desta sessão, sem o token: ele vai no `Authorization`, lido do `_srv.json`.
    if cfg!(target_os = "linux") && let Some(port) = PORT.get() {
        value["cdp"] = browser_endpoint(*port, key).into();
        value["cdp_local"] = local_endpoint(*port, key.rsplit("::").next().unwrap_or(key)).into();
    }
    if let Err(e) = write_json(&sidecar_name(key), &value) { eprintln!("[nav] sidecar de {key} nao gravado: {e}"); }
}

static PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

fn browser_endpoint(port: u16, key: &str) -> String {
    format!("ws://127.0.0.1:{port}/cdp/browser?{}", url::form_urlencoded::Serializer::new(String::new()).append_pair("chave", key).finish())
}

/// O mesmo CDP sem token, por nome de sessão do servidor desta máquina: para quem só recebe uma URL.
fn local_endpoint(port: u16, name: &str) -> String {
    let encoded = url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>().replace('+', "%20");
    format!("ws://127.0.0.1:{port}/cdp/browser/{encoded}")
}

/// A casca das páginas nasce com `?token=` e só o tira depois de carregar; o sidecar é lido pelo
/// `hangar-preview`, cuja saída vai para o transcript.
fn without_token(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw) else { return raw.to_owned() };
    if !url.query_pairs().any(|(k, _)| k == "token") { return raw.to_owned(); }
    let rest: Vec<(String, String)> = url.query_pairs().filter(|(k, _)| k != "token").map(|(k, v)| (k.into(), v.into())).collect();
    if rest.is_empty() { url.set_query(None) } else { url.query_pairs_mut().clear().extend_pairs(rest); }
    url.into()
}

pub fn remove_sidecar(key: &str) {
    let Some(dir) = nav_dir() else { return };
    match std::fs::remove_file(dir.join(format!("{}.json", sidecar_name(key)))) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => eprintln!("[nav] sidecar de {key} nao apagado: {e}"),
        _ => {}
    }
}

/// Só o app nativo grava `pid`: sidecar do Electron (sem `pid`) nunca é tocado.
/// Sidecar de outro app nativo ainda vivo (uma execução de prova sobe ao lado do principal) também fica.
fn is_stale_native_sidecar(value: &Value, pid: u32) -> bool {
    value.get("pid").is_some_and(|p| p.as_u64() != Some(u64::from(pid)) && p.as_u64().and_then(pid_alive) != Some(true))
}

/// `None` onde não há como saber sem custo (fora do Linux).
fn pid_alive(pid: u64) -> Option<bool> {
    cfg!(target_os = "linux").then(|| std::path::Path::new(&format!("/proc/{pid}")).exists())
}

/// O app não apaga os sidecars ao sair; sem isto, cada reinício deixa navegadores fantasmas para o CLI e o backend.
fn drop_stale_sidecars() {
    let Some(dir) = nav_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for path in entries.flatten().map(|e| e.path()) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with(['_', '.']) || !name.ends_with(".json") { continue; }
        let Some(value) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        if is_stale_native_sidecar(&value, std::process::id()) && let Err(e) = std::fs::remove_file(&path) {
            eprintln!("[nav] sidecar antigo {name} nao apagado: {e}");
        }
    }
}

fn same(a: &[u8], b: &[u8]) -> bool { a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0 }

fn new_token() -> std::io::Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 24];
    ring::rand::SystemRandom::new().fill(&mut bytes).map_err(|_| std::io::Error::other("sem aleatoriedade"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn parse_head(text: &str) -> Result<Head, (u16, String)> {
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, path) = (first.next().unwrap_or("").to_owned(), first.next().unwrap_or("").to_owned());
    let mut head = Head { method, path, auth: None, length: 0, upgrade: None, ws_key: None, origin: None, host: None };
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        match name.trim().to_ascii_lowercase().as_str() {
            "authorization" => head.auth = Some(value.trim().to_owned()),
            "upgrade" => head.upgrade = Some(value.trim().to_owned()),
            "sec-websocket-key" => head.ws_key = Some(value.trim().to_owned()),
            "origin" => head.origin = Some(value.trim().to_owned()),
            "host" => head.host = Some(value.trim().to_owned()),
            "content-length" => head.length = value.trim().parse().map_err(|_| (400, "erro: corpo invalido".to_string()))?,
            _ => {}
        }
    }
    if head.length > BODY_MAX { return Err((413, "erro: corpo grande demais".into())); }
    Ok(head)
}

fn check(head: &Head, token: &str, port: u16) -> Result<Route, (u16, String)> {
    let (path, query) = head.path.split_once('?').unwrap_or((&head.path, ""));
    if head.method == "GET" && let Some(name) = path.strip_prefix("/cdp/browser/") { return check_local(head, name, port); }
    let key = || url::form_urlencoded::parse(query.as_bytes()).find_map(|(name, value)| (name == "chave").then(|| value.into_owned())).unwrap_or_default();
    let route = match (head.method.as_str(), path) {
        ("POST", "/cmd") => Route::Cmd,
        ("GET", "/cdp") => Route::Cdp(key()),
        ("GET", "/cdp/browser") => Route::Browser(key()),
        _ => return Err((404, "erro: rota desconhecida".into())),
    };
    if !same(head.auth.as_deref().unwrap_or("").as_bytes(), format!("Bearer {token}").as_bytes()) { return Err((401, "erro: token invalido".into())); }
    if let Route::Cdp(key) | Route::Browser(key) = &route {
        if key.is_empty() { return Err((400, format!("erro: {path} precisa de ?chave="))); }
        if !head.upgrade.as_deref().is_some_and(|u| u.eq_ignore_ascii_case("websocket")) || head.ws_key.is_none() {
            return Err((400, format!("erro: {path} so aceita WebSocket")));
        }
    }
    Ok(route)
}

/// Sem token: só serve quem não é página web (navegador sempre manda `Origin`; o Playwright não) e chegou pelo
/// próprio loopback (`Host` de outro nome é rebinding de DNS).
fn check_local(head: &Head, name: &str, port: u16) -> Result<Route, (u16, String)> {
    if head.origin.is_some() { return Err((403, "erro: origin_refused: o CDP sem token recusa pedido com Origin".into())); }
    let host = head.host.as_deref().unwrap_or("");
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Err((403, "erro: host_refused: o CDP sem token so atende 127.0.0.1 ou localhost nesta porta".into()));
    }
    let name = url::form_urlencoded::parse(format!("n={}", name.replace('+', "%2B")).as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default();
    if name.is_empty() || name.contains('/') { return Err((404, "erro: session_not_found: falta o nome da sessao em /cdp/browser/<nome>".into())); }
    if !head.upgrade.as_deref().is_some_and(|u| u.eq_ignore_ascii_case("websocket")) || head.ws_key.is_none() {
        return Err((400, "erro: websocket_required: /cdp/browser/<nome> so aceita WebSocket".into()));
    }
    Ok(Route::Local(name))
}

fn parse_body(bytes: &[u8]) -> Result<Body, String> {
    let invalid = || "erro: corpo invalido".to_string();
    let v: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let cdp = v["cdp"].as_str().map(str::to_owned);
    let key = match v["chave"].as_str() { Some(k) => k.to_owned(), None if cdp.is_some() => String::new(), None => return Err(invalid()) };
    let verb = v["verbo"].as_str().ok_or_else(invalid)?.to_owned();
    let args = v["args"].as_array().into_iter().flatten().map(|a| a.as_str().map_or_else(|| a.to_string(), str::to_owned)).collect();
    // shortcut: o corpo lido e o `Value` não são zerados; só o valor que fica vivo no controlador é.
    let secret = v["valor"].as_str().map(|s| Secret::new(s.to_owned()));
    Ok(Body { key, verb, args, tab: v["aba"].as_i64(), cdp, alvo: v["alvo"].as_str().map(str::to_owned), secret })
}

pub fn start(runtime: &Runtime, requests: async_channel::Sender<Request>) -> std::io::Result<u16> {
    // A porta da execução anterior primeiro: o `/cdp/browser/<nome>` que alguém guardou continua valendo.
    let previous = nav_dir().and_then(|d| std::fs::read(d.join("_srv.json")).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["porta"].as_u64()).and_then(|p| u16::try_from(p).ok());
    let listener = match previous.and_then(|p| std::net::TcpListener::bind(("127.0.0.1", p)).ok()) {
        Some(listener) => listener,
        None => std::net::TcpListener::bind(("127.0.0.1", 0))?,
    };
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let token = new_token()?;
    // Converte antes de publicar a porta: servidor que não sobe nunca aparece no `_srv.json`.
    let listener = { let _guard = runtime.enter(); tokio::net::TcpListener::from_std(listener)? };
    drop_stale_sidecars();
    let own = json!({"porta": port, "token": token, "pid": std::process::id(), "ts": now_ms()});
    write_json("_srv", &own)?;
    let _ = PORT.set(port);
    if let Some(dir) = nav_dir() {
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(RECLAIM_EVERY).await;
                match reclaim(&dir, &own).await {
                    Ok(true) => crate::log_line("nav: _srv.json apontava um servidor que não atende; retomado por este app"),
                    Ok(false) => {}
                    Err(e) => crate::log_line(&format!("nav: _srv.json não retomado: {e}")),
                }
            }
        });
    }
    let token: Arc<str> = token.into();
    runtime.spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => { tokio::spawn(serve(stream, token.clone(), requests.clone())); }
                // Erro de accept (ex.: limite de sockets) é passageiro: sair do laço mataria o servidor calado.
                Err(e) => { eprintln!("[nav] accept falhou: {e}"); tokio::time::sleep(Duration::from_millis(100)).await; }
            }
        }
    });
    Ok(port)
}

async fn serve(mut stream: TcpStream, token: Arc<str>, requests: async_channel::Sender<Request>) {
    let (status, text) = match tokio::time::timeout(Duration::from_secs(10), read_request(&mut stream)).await {
        Err(_) => return,
        Ok(Err(fail)) => fail,
        Ok(Ok((head, body))) => match check(&head, &token, stream.local_addr().map_or(0, |a| a.port())) {
            Err(fail) => fail,
            Ok(Route::Cmd) => (200, answer(&body, &requests).await),
            Ok(Route::Cdp(key)) => {
                let ws_key = head.ws_key.unwrap_or_default();
                return super::relay::serve(stream, &ws_key, key, requests).await;
            }
            #[cfg(target_os = "linux")]
            Ok(Route::Browser(key)) => {
                let ws_key = head.ws_key.unwrap_or_default();
                return super::chromium::proxy::serve(stream, &ws_key, key, requests).await;
            }
            #[cfg(not(target_os = "linux"))]
            Ok(Route::Browser(_)) => (501, "erro: /cdp/browser so existe no app nativo do Linux".into()),
            #[cfg(target_os = "linux")]
            Ok(Route::Local(name)) => match open_local(name, &requests).await {
                Ok(key) => return super::chromium::proxy::serve(stream, &head.ws_key.unwrap_or_default(), key, requests).await,
                Err(fail) => fail,
            },
            #[cfg(not(target_os = "linux"))]
            Ok(Route::Local(_)) => (501, "erro: /cdp/browser so existe no app nativo do Linux".into()),
        },
    };
    respond(&mut stream, status, &text).await;
}

/// Prazo para o navegador que o pedido abriu ficar pronto (o Chromium pode estar subindo).
#[cfg(target_os = "linux")]
const OPEN_SECS: u64 = 20;

/// Abre (se preciso) o navegador da sessão e espera o motor dela: a resposta é o 101 do proxy ou um erro, nunca um
/// socket aceito e parado.
#[cfg(target_os = "linux")]
async fn open_local(name: String, requests: &async_channel::Sender<Request>) -> Result<String, (u16, String)> {
    let closing = || (503, "erro: app_closing: o app nativo esta fechando".to_string());
    let (tx, rx) = oneshot::channel();
    requests.send(Request::Open { name: name.clone(), reply: tx }).await.map_err(|_| closing())?;
    let key = match tokio::time::timeout(Duration::from_secs(10), rx).await {
        Ok(Ok(found)) => found?,
        _ => return Err((503, "erro: app_not_responding: o app nao respondeu".into())),
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(OPEN_SECS);
    loop {
        let (tx, rx) = oneshot::channel();
        requests.send(Request::Pipe { key: key.clone(), reply: tx }).await.map_err(|_| closing())?;
        if let Ok(Ok(Ok(_))) = tokio::time::timeout(Duration::from_secs(10), rx).await { return Ok(key); }
        if tokio::time::Instant::now() >= deadline {
            return Err((503, format!("erro: browser_not_ready: o navegador de {name} nao ficou pronto em {OPEN_SECS}s")));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Resposta HTTP de texto, uma por conexão, como o servidor do Electron.
pub(super) async fn respond(stream: &mut TcpStream, status: u16, text: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        431 => "Request Header Fields Too Large",
        _ => "Internal Server Error",
    };
    let response = format!("HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
    let _ = stream.write_all(response.as_bytes()).await;
}

async fn read_request(stream: &mut TcpStream) -> Result<(Head, Vec<u8>), (u16, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break i; }
        if buf.len() > HEAD_MAX { return Err((431, "erro: cabecalho grande demais".into())); }
        let n = stream.read(&mut chunk).await.map_err(|e| (500, format!("erro: {e}")))?;
        if n == 0 { return Err((400, "erro: corpo invalido".into())); }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = parse_head(&String::from_utf8_lossy(&buf[..end]))?;
    let mut body = buf.split_off(end + 4);
    while body.len() < head.length {
        let n = stream.read(&mut chunk).await.map_err(|e| (500, format!("erro: {e}")))?;
        if n == 0 { break; }
        body.extend_from_slice(&chunk[..n]);
        if body.len() > BODY_MAX { return Err((413, "erro: corpo grande demais".into())); }
    }
    body.truncate(head.length);
    Ok((head, body))
}

async fn answer(bytes: &[u8], requests: &async_channel::Sender<Request>) -> String {
    let body = match parse_body(bytes) { Ok(b) => b, Err(e) => return e };
    let path = (body.verb == "shot").then(|| body.args.first().cloned()).flatten();
    if body.verb == "shot" && path.is_none() { return "erro: shot precisa de um caminho de arquivo".into(); }
    let verb_is_login = body.verb == "login";
    let rx = if let Some(cdp) = body.cdp {
        if body.tab.is_some() { return "erro: --cdp nao tem abas: escolha a pagina com --alvo <trecho da url>".into(); }
        match super::external::submit(&cdp, body.alvo, body.verb, body.args) { Ok(rx) => rx, Err(e) => return e }
    } else {
        let (tx, rx) = oneshot::channel();
        if requests.send(Request::Command { key: body.key, verb: body.verb, args: body.args, tab: body.tab, secret: body.secret, reply: tx }).await.is_err() {
            return "erro: o app nativo esta fechando".into();
        }
        rx
    };
    // `login` espera a pessoa aprovar no painel e, às vezes, escolher o perfil.
    let secs = if verb_is_login { LOGIN_SECS } else { REPLY_SECS };
    match tokio::time::timeout(Duration::from_secs(secs), rx).await {
        Err(_) if verb_is_login => format!("erro: o login nao terminou no painel em {secs}s (aprovacao ou escolha de perfil pendente)"),
        Err(_) => format!("erro: o app nao respondeu em {secs}s"),
        Ok(Err(_)) => "erro: o navegador fechou no meio do comando".into(),
        Ok(Ok(Reply::Text(text))) => text,
        Ok(Ok(Reply::Png(png))) => {
            let path = path.unwrap_or_default();
            // Fora da thread da interface: disco lento não trava a janela.
            match tokio::task::spawn_blocking({ let path = path.clone(); move || std::fs::write(path, png) }).await {
                Ok(Ok(())) => format!("ok: shot {path}"),
                Ok(Err(e)) => format!("erro: shot {path}: {e}"),
                Err(e) => format!("erro: shot {path}: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pid de um processo que já terminou.
    fn dead_pid() -> u64 {
        let mut child = if cfg!(windows) { std::process::Command::new("cmd").args(["/C", "exit"]).spawn() }
            else { std::process::Command::new("true").spawn() }.unwrap();
        child.wait().unwrap();
        u64::from(child.id())
    }

    async fn component_request(text: &str) -> (TcpStream, async_channel::Receiver<Request>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (requests, ui) = async_channel::unbounded();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve(stream, "component-proof".into(), requests).await;
        });
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(text.as_bytes()).await.unwrap();
        (client, ui, task)
    }

    async fn response_head(client: &mut TcpStream) -> String {
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                assert!(bytes.len() < HEAD_MAX);
                bytes.push(client.read_u8().await.unwrap());
            }
            String::from_utf8(bytes).unwrap()
        }).await.expect("o servidor precisa concluir o handshake")
    }

    #[tokio::test]
    async fn cdp_http_route_enforces_access_before_watch_and_unwatches_the_same_viewer() {
        let upgrade = "Upgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n";
        for (request, status) in [
            (format!("GET /cdp?chave=s%3A%3Anative HTTP/1.1\r\n{upgrade}\r\n"), 401),
            (format!("GET /cdp?chave=s%3A%3Anative&token=component-proof HTTP/1.1\r\n{upgrade}\r\n"), 401),
            (format!("GET /cdp HTTP/1.1\r\nAuthorization: Bearer component-proof\r\n{upgrade}\r\n"), 400),
            ("GET /cdp?chave=a HTTP/1.1\r\nAuthorization: Bearer component-proof\r\n\r\n".into(), 400),
            (format!("POST /cdp?chave=a HTTP/1.1\r\nAuthorization: Bearer component-proof\r\n{upgrade}\r\n"), 404),
        ] {
            let (mut client, ui, task) = component_request(&request).await;
            assert!(response_head(&mut client).await.starts_with(&format!("HTTP/1.1 {status} ")));
            tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
            assert!(ui.is_empty(), "recusa de acesso não pode registrar espectador");
        }
        let request = format!("GET /cdp?chave=s%3A%3Anative HTTP/1.1\r\nAuthorization: Bearer component-proof\r\n{upgrade}\r\n");
        let (mut client, ui, task) = component_request(&request).await;
        let Request::Watch { key, viewer, reply } = tokio::time::timeout(Duration::from_secs(2), ui.recv()).await.unwrap().unwrap()
            else { panic!("esperava registro do espectador") };
        assert_eq!(key, "s::native");
        let id = viewer.id;
        reply.send(Ok(())).unwrap();
        let head = response_head(&mut client).await;
        assert!(head.starts_with("HTTP/1.1 101 "));
        assert!(head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
        client.write_all(&[0x88, 0x82, 0, 0, 0, 0, 3, 232]).await.unwrap();
        let mut close = [0; 4];
        tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut close)).await.unwrap().unwrap();
        assert_eq!(close, [0x88, 2, 3, 232]);
        let Request::Unwatch { key, id: ended } = tokio::time::timeout(Duration::from_secs(2), ui.recv()).await.unwrap().unwrap()
            else { panic!("esperava retirada do espectador") };
        assert_eq!((key.as_str(), ended), ("s::native", id));
        tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn cdp_watch_refusal_and_missing_reply_never_upgrade() {
        let request = "GET /cdp?chave=native HTTP/1.1\r\nAuthorization: Bearer component-proof\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        for refuse in [true, false] {
            let (mut client, ui, task) = component_request(request).await;
            let Request::Watch { reply, .. } = tokio::time::timeout(Duration::from_secs(2), ui.recv()).await.unwrap().unwrap()
                else { panic!("esperava registro do espectador") };
            if refuse { reply.send(Err("erro: navegador ausente".into())).unwrap(); } else { drop(reply); }
            let mut response = String::new();
            tokio::time::timeout(Duration::from_secs(2), client.read_to_string(&mut response)).await.unwrap().unwrap();
            let (status, message) = if refuse { (404, "erro: navegador ausente") } else { (500, "erro: o app nao respondeu") };
            assert!(response.starts_with(&format!("HTTP/1.1 {status} ")));
            assert!(response.ends_with(message));
            assert!(!response.contains("101 Switching Protocols"));
            tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn cdp_pending_watch_has_a_ten_second_deadline() {
        let request = "GET /cdp?chave=native HTTP/1.1\r\nAuthorization: Bearer component-proof\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let since = std::time::Instant::now();
        let (mut client, ui, task) = component_request(request).await;
        let Request::Watch { reply, .. } = tokio::time::timeout(Duration::from_secs(2), ui.recv()).await.unwrap().unwrap()
            else { panic!("esperava registro do espectador") };
        let mut response = String::new();
        tokio::time::timeout(Duration::from_secs(12), client.read_to_string(&mut response)).await.unwrap().unwrap();
        assert!(since.elapsed() >= Duration::from_secs(10));
        assert!(response.starts_with("HTTP/1.1 500 "));
        assert!(response.ends_with("erro: o app nao respondeu"));
        assert!(reply.send(Ok(())).is_err(), "a espera expirada foi descartada");
        tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_sidecars_are_proved_in_a_separate_home() {
        let root = std::env::temp_dir().join(format!("hangar-native-sidecars-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir(&root).unwrap();
        let identity = new_token().unwrap();
        std::fs::write(root.join("proof-owner"), &identity).unwrap();
        struct OwnedRoot { path: PathBuf, identity: String }
        impl Drop for OwnedRoot {
            fn drop(&mut self) {
                if std::fs::read_to_string(self.path.join("proof-owner")).ok().as_deref() == Some(self.identity.as_str()) {
                    let _ = std::fs::remove_dir_all(&self.path);
                }
            }
        }
        let owned = OwnedRoot { path: root.clone(), identity };
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "browser::server::tests::isolated_native_sidecar_proof", "--ignored", "--test-threads=1"])
            .env_clear().env("HOME", &root).env("HANGAR_NATIVE_SIDECAR_PROOF", &root)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() { break status; }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("subprocesso de sidecars excedeu o prazo");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success(), "subprocesso de sidecars falhou");
        assert!(std::fs::read_to_string(root.join("completed")).ok().as_deref() == Some(owned.identity.as_str()), "o filtro precisa executar o caso filho");
        drop(owned);
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "executado somente pelo pai, com HOME exclusivo definido antes do processo"]
    fn isolated_native_sidecar_proof() {
        use std::os::unix::fs::PermissionsExt;
        let root = PathBuf::from(std::env::var_os("HANGAR_NATIVE_SIDECAR_PROOF").expect("subprocesso exclusivo"));
        assert_eq!(std::env::home_dir().as_deref(), Some(root.as_path()));
        assert!(root.join("proof-owner").is_file());
        let dir = nav_dir().unwrap();
        write_json("stale", &json!({"pid": dead_pid(), "chave": "old"})).unwrap();
        write_json("live", &json!({"pid": std::process::id(), "chave": "live"})).unwrap();
        write_json("legacy", &json!({"chave": "electron", "url": "http://legacy/"})).unwrap();
        write_json("_reserved", &json!({"pid": 0})).unwrap();
        std::fs::write(dir.join("bad.json"), "{").unwrap();
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        write_json("_srv", &json!({"porta": occupied.local_addr().unwrap().port()})).unwrap();
        let runtime = Runtime::new().unwrap();
        let (requests, ui) = async_channel::unbounded();
        let port = start(&runtime, requests).unwrap();
        assert_ne!(port, occupied.local_addr().unwrap().port());
        assert!(!dir.join("stale.json").exists());
        for name in ["live.json", "legacy.json", "_reserved.json", "bad.json"] { assert!(dir.join(name).is_file()); }
        let server: Value = serde_json::from_slice(&std::fs::read(dir.join("_srv.json")).unwrap()).unwrap();
        assert_eq!(server["porta"], port);
        assert_eq!(server["pid"], std::process::id());
        let token = server["token"].as_str().unwrap();
        assert!(!token.is_empty());
        runtime.block_on(async {
            let ui = tokio::spawn(async move {
                let Request::Command { key, verb, reply, .. } = ui.recv().await.unwrap() else { panic!("esperava comando") };
                assert_eq!((key.as_str(), verb.as_str()), ("proof::native", "url"));
                reply.send(Reply::Text("http://example.test/ready".into())).ok().unwrap();
            });
            let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let body = r#"{"chave":"proof::native","verbo":"url","args":[]}"#;
            let request = format!("POST /cmd HTTP/1.1\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            socket.write_all(request.as_bytes()).await.unwrap();
            let mut response = String::new();
            tokio::time::timeout(Duration::from_secs(2), socket.read_to_string(&mut response)).await.unwrap().unwrap();
            assert!(response.starts_with("HTTP/1.1 200 ") && response.ends_with("http://example.test/ready"));
            ui.await.unwrap();
        });
        let key = "http://127.0.0.1:18865::proof-native";
        write_sidecar(key, "http://example.test/one?raw=1&token=sensitive-query-value#anchor", "Primeira");
        let path = dir.join(format!("{}.json", sidecar_name(key)));
        let read = || serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap();
        let first = read();
        assert_eq!(first["url"], "http://example.test/one?raw=1#anchor");
        assert_eq!(first["abas"][0]["url"], first["url"]);
        assert_eq!(first["targetId"], Value::Null);
        assert_eq!(first["abas"][0]["targetId"], Value::Null);
        assert_eq!(first["ativa"], 1);
        assert_eq!(first["pid"], std::process::id());
        assert_eq!(first["chave"], key);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("sensitive-query-value"));
        let reading = Arc::new(std::sync::atomic::AtomicBool::new(true));
        std::thread::scope(|scope| {
            let running = reading.clone();
            let path = &path;
            let reader = scope.spawn(move || {
                let mut count = 0;
                while running.load(std::sync::atomic::Ordering::Relaxed) || count == 0 {
                    let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                    assert!(matches!(value["url"].as_str(), Some("http://example.test/one?raw=1#anchor" | "http://example.test/two")));
                    assert_eq!(value["abas"][0]["url"], value["url"]);
                    count += 1;
                }
            });
            for _ in 0..32 { write_sidecar(key, "http://example.test/two?token=sensitive-query-value", "Segunda"); }
            reading.store(false, std::sync::atomic::Ordering::Relaxed);
            reader.join().unwrap();
        });
        assert_eq!(read()["url"], "http://example.test/two");
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        for path in [path.clone(), dir.join("_srv.json")] {
            assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        remove_sidecar(key);
        assert!(!path.exists());
        assert!(dir.join("live.json").is_file());
        assert!(!std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.path().extension().is_some_and(|ext| ext == "tmp")));
        runtime.shutdown_timeout(Duration::from_secs(2));
        std::fs::write(root.join("completed"), std::fs::read(root.join("proof-owner")).unwrap()).unwrap();
    }

    #[tokio::test]
    async fn srv_file_is_taken_back_only_from_an_owner_that_no_longer_answers() {
        let dir = std::env::temp_dir().join(format!("hangar-srv-reclaim-{}-{}", std::process::id(), now_ms()));
        let live = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let dead = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let own = json!({"porta": live.local_addr().unwrap().port(), "token": "meu", "pid": std::process::id(), "ts": 1});
        let read = || serde_json::from_slice::<Value>(&std::fs::read(dir.join("_srv.json")).unwrap()).unwrap();

        assert!(reclaim(&dir, &own).await.unwrap(), "sem arquivo, o dono publica");
        assert_eq!(read(), own);
        assert!(!reclaim(&dir, &own).await.unwrap(), "arquivo já é deste processo");

        let other = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let alive = json!({"porta": other.local_addr().unwrap().port(), "token": "outro", "pid": 1, "ts": 2});
        write_json_in(&dir, "_srv", &alive).unwrap();
        assert!(!reclaim(&dir, &own).await.unwrap(), "outra instância viva não é desalojada");
        assert_eq!(read(), alive);

        write_json_in(&dir, "_srv", &json!({"porta": dead, "token": "morto", "pid": dead_pid(), "ts": 3})).unwrap();
        assert!(reclaim(&dir, &own).await.unwrap(), "porta sem dono é retomada");
        assert_eq!(read(), own);

        if cfg!(target_os = "linux") {
            let recycled = json!({"porta": other.local_addr().unwrap().port(), "token": "morto", "pid": dead_pid(), "ts": 4});
            write_json_in(&dir, "_srv", &recycled).unwrap();
            assert!(reclaim(&dir, &own).await.unwrap(), "pid morto vence a porta reaproveitada por outro programa");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn head_is_parsed_and_checked_in_electron_order() {
        let head = parse_head("POST /cmd HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer abc\r\nContent-Length: 12").unwrap();
        assert_eq!((head.length, head.auth.as_deref()), (12, Some("Bearer abc")));
        assert_eq!(check(&head, "abc", 1), Ok(Route::Cmd));
        assert_eq!(check(&head, "zzz", 1), Err((401, "erro: token invalido".into())));
        let other = parse_head("GET /cmd HTTP/1.1").unwrap();
        assert_eq!(check(&other, "abc", 1), Err((404, "erro: rota desconhecida".into())));
        assert_eq!(parse_head("POST /cmd HTTP/1.1\r\nContent-Length: 999999").err(), Some((413, "erro: corpo grande demais".into())));
    }

    #[test]
    fn cdp_route_needs_token_key_and_websocket() {
        let upgrade = "\r\nAuthorization: Bearer abc\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==";
        let head = parse_head(&format!("GET /cdp?chave=srv%3A%3Anav-teste HTTP/1.1{upgrade}")).unwrap();
        assert_eq!(check(&head, "abc", 1), Ok(Route::Cdp("srv::nav-teste".into())));
        assert_eq!(check(&head, "zzz", 1), Err((401, "erro: token invalido".into())));
        let plain = parse_head("GET /cdp?chave=s%3A%3Aa HTTP/1.1\r\nAuthorization: Bearer abc").unwrap();
        assert_eq!(check(&plain, "abc", 1), Err((400, "erro: /cdp so aceita WebSocket".into())));
        let keyless = parse_head(&format!("GET /cdp HTTP/1.1{upgrade}")).unwrap();
        assert_eq!(check(&keyless, "abc", 1), Err((400, "erro: /cdp precisa de ?chave=".into())));
        let post = parse_head(&format!("POST /cdp?chave=a HTTP/1.1{upgrade}")).unwrap();
        assert_eq!(check(&post, "abc", 1), Err((404, "erro: rota desconhecida".into())));
    }

    #[test]
    fn browser_cdp_route_has_the_same_checks_and_a_tokenless_endpoint() {
        let upgrade = "\r\nAuthorization: Bearer abc\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==";
        let url = browser_endpoint(4321, "srv::nav teste");
        assert_eq!(url, "ws://127.0.0.1:4321/cdp/browser?chave=srv%3A%3Anav+teste");
        let head = parse_head(&format!("GET {} HTTP/1.1{upgrade}", url.trim_start_matches("ws://127.0.0.1:4321"))).unwrap();
        assert_eq!(check(&head, "abc", 1), Ok(Route::Browser("srv::nav teste".into())));
        assert_eq!(check(&head, "zzz", 1), Err((401, "erro: token invalido".into())));
        let plain = parse_head("GET /cdp/browser?chave=a HTTP/1.1\r\nAuthorization: Bearer abc").unwrap();
        assert_eq!(check(&plain, "abc", 1), Err((400, "erro: /cdp/browser so aceita WebSocket".into())));
    }

    #[test]
    fn tokenless_route_refuses_origin_and_foreign_host() {
        let url = local_endpoint(4000, "nav teste");
        assert_eq!(url, "ws://127.0.0.1:4000/cdp/browser/nav%20teste");
        let path = url.trim_start_matches("ws://127.0.0.1:4000");
        let ws = "\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==";
        let head = |extra: &str| parse_head(&format!("GET {path} HTTP/1.1{ws}{extra}")).unwrap();
        let code = |r: Result<Route, (u16, String)>| r.map_err(|(s, t)| (s, t.split(':').nth(1).unwrap_or("").trim().to_owned()));
        assert_eq!(check(&head("\r\nHost: 127.0.0.1:4000"), "abc", 4000), Ok(Route::Local("nav teste".into())), "sem token");
        assert_eq!(check(&head("\r\nHost: localhost:4000"), "abc", 4000), Ok(Route::Local("nav teste".into())));
        assert_eq!(code(check(&head("\r\nHost: 127.0.0.1:4000\r\nOrigin: https://evil.example"), "abc", 4000)), Err((403, "origin_refused".into())));
        assert_eq!(code(check(&head("\r\nHost: 127.0.0.1:4000\r\nOrigin: null"), "abc", 4000)), Err((403, "origin_refused".into())));
        assert_eq!(code(check(&head("\r\nHost: evil.example:4000"), "abc", 4000)), Err((403, "host_refused".into())));
        assert_eq!(code(check(&head("\r\nHost: 127.0.0.1:4001"), "abc", 4000)), Err((403, "host_refused".into())));
        assert_eq!(code(check(&head(""), "abc", 4000)), Err((403, "host_refused".into())));
        let plain = parse_head(&format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:4000")).unwrap();
        assert_eq!(code(check(&plain, "abc", 4000)), Err((400, "websocket_required".into())));
        let nameless = parse_head(&format!("GET /cdp/browser/ HTTP/1.1{ws}\r\nHost: 127.0.0.1:4000")).unwrap();
        assert_eq!(code(check(&nameless, "abc", 4000)), Err((404, "session_not_found".into())));
    }

    /// Chromium de verdade atrás do servidor real (rota, token, proxy), sem o app: o comando em `HANGAR_CDP_PROOF`
    /// (shell) recebe `HANGAR_CDP_URL` e `HANGAR_CDP_TOKEN` e precisa sair com 0. Depois dele, a sessão do "app" na
    /// mesma página continua respondendo e vê a página que o cliente deixou.
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "sobe o Chromium da máquina e roda o cliente de HANGAR_CDP_PROOF"]
    async fn real_chromium_serves_an_outside_cdp_client() {
        use super::super::chromium::pipe::Port;
        let proof = std::env::var("HANGAR_CDP_PROOF").expect("HANGAR_CDP_PROOF");
        let profile = std::env::temp_dir().join(format!("hangar-cdp-proof-{}", std::process::id()));
        let (port, mut child) = Port::launch(&profile);
        let created = port.call(None, "Target.createTarget", json!({"url": "about:blank", "newWindow": true})).await.unwrap();
        let target = created["targetId"].as_str().unwrap().to_owned();
        let app = port.call(None, "Target.attachToTarget", json!({"targetId": target, "flatten": true})).await.unwrap()["sessionId"]
            .as_str().unwrap().to_owned();
        port.call(Some(&app), "Page.enable", json!({})).await.unwrap();

        let (requests, ui) = async_channel::unbounded();
        let (pipe, visible) = (port.clone(), target.clone());
        tokio::spawn(async move {
            while let Ok(request) = ui.recv().await {
                match request {
                    Request::Pipe { reply, .. } => { let _ = reply.send(Ok((pipe.clone(), visible.clone()))); }
                    Request::Open { name, reply } => {
                        let _ = reply.send(if name == "proof" { Ok("srv::proof".into()) } else { Err((404, format!("erro: session_not_found: {name}"))) });
                    }
                    _ => {}
                }
            }
        });
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let token: Arc<str> = "t0k".into();
        tokio::spawn(async move { loop { let (s, _) = listener.accept().await.unwrap(); tokio::spawn(serve(s, token.clone(), requests.clone())); } });

        let status = tokio::process::Command::new("sh").arg("-c").arg(&proof)
            .env("HANGAR_CDP_URL", browser_endpoint(address.port(), "srv::proof")).env("HANGAR_CDP_TOKEN", "t0k")
            .env("HANGAR_CDP_LOCAL_URL", local_endpoint(address.port(), "proof"))
            .status().await.unwrap();
        let href = port.call(Some(&app), "Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true})).await;
        drop(port.call(None, "Browser.close", json!({})));
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&profile);
        assert!(status.success(), "cliente falhou: {status}");
        eprintln!("titulo visto pela sessao do app depois do cliente: {:?}", href.unwrap()["result"]["value"]);
    }

    #[test]
    fn body_with_accent_survives_split_reads() {
        let body = r#"{"chave":"s::a","verbo":"fill","args":["@e2","ação"]}"#.as_bytes();
        // O "ç" (2 bytes) cortado entre duas leituras: a decodificação é uma só, no fim.
        let cut = body.iter().position(|&b| b == 0xc3).unwrap() + 1;
        let mut joined = body[..cut].to_vec();
        joined.extend_from_slice(&body[cut..]);
        let parsed = parse_body(&joined).unwrap();
        assert_eq!((parsed.key.as_str(), parsed.args[1].as_str()), ("s::a", "ação"));
    }

    #[test]
    fn body_errors_use_the_shell_text() {
        assert_eq!(parse_body(b"{").err(), Some("erro: corpo invalido".into()));
        assert_eq!(parse_body(br#"{"verbo":"snapshot"}"#).err(), Some("erro: corpo invalido".into()));
        let with_tab = parse_body(br#"{"chave":"k","verbo":"url","aba":2}"#).unwrap();
        assert_eq!((with_tab.tab, with_tab.args.len()), (Some(2), 0));
        let external = parse_body(br#"{"cdp":"http://127.0.0.1:9222","alvo":"/agenda","verbo":"url"}"#).unwrap();
        assert_eq!((external.key.as_str(), external.cdp.as_deref(), external.alvo.as_deref()), ("", Some("http://127.0.0.1:9222"), Some("/agenda")));
    }

    #[tokio::test]
    async fn external_cdp_outside_loopback_is_refused_before_any_connection() {
        let (requests, ui) = async_channel::unbounded();
        let text = answer(br#"{"cdp":"http://192.168.77.142:9222","verbo":"snapshot"}"#, &requests).await;
        assert!(text.starts_with("erro: --cdp so aceita"), "{text}");
        assert!(ui.is_empty(), "nada vai ao navegador da sessao");
    }

    #[test]
    fn sidecar_url_never_keeps_the_token() {
        assert_eq!(super::without_token("http://h/api/sessions/s/pages/a?token=abc"), "http://h/api/sessions/s/pages/a");
        assert_eq!(super::without_token("http://h/p?raw=1&token=abc#x"), "http://h/p?raw=1#x");
        assert_eq!(super::without_token("http://h/p?q=1"), "http://h/p?q=1");
        assert_eq!(super::without_token(""), "");
    }

    #[test]
    fn only_native_sidecars_of_another_pid_are_stale() {
        assert!(is_stale_native_sidecar(&json!({"chave": "s::a", "pid": dead_pid()}), 20));
        if cfg!(target_os = "linux") {
            let other_live_app = u64::from(std::process::id());
            assert!(!is_stale_native_sidecar(&json!({"chave": "s::a", "pid": other_live_app}), 20), "app vivo ao lado guarda os seus");
        }
        assert!(!is_stale_native_sidecar(&json!({"chave": "s::a", "pid": 20}), 20));
        assert!(!is_stale_native_sidecar(&json!({"chave": "s::a", "url": "http://x/"}), 20));
    }

    #[test]
    fn same_is_constant_shape() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
    }
}
