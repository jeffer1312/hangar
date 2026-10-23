//! Pedido do agente para abrir o navegador embutido da própria sessão (`hangar-preview open`, tool
//! `browser_open`): `POST`/`DELETE /api/sessions/{name}/nav`. O app desktop cria o view ao receber o
//! evento `nav` (pela lista, que ele mantém aberta, ou pela conversa) e confirma com o `DELETE`.
//! Um marcador por sessão, só a última url; vence em 10 min e sobrevive ao reinício em
//! `~/.hangar/nav/_pendentes.json`, que o Python lê para o que ele ainda serve (8768, modo `python`).
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::routes::AppState;
use crate::session_write::json_response;

/// Prazo do pedido que ninguém confirmou: não reabre a aba esquecida depois.
pub const TTL_SECS: f64 = 600.0;
const BODY_LIMIT: usize = 64 * 1024;
/// `urllib.parse.quote(token, safe='')`.
const TOKEN_SET: &AsciiSet = &NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~');

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker { pub url: String, pub ts: f64 }

pub struct NavStore {
    /// `None`: só em memória (bridge de teste sem pastas).
    file: Option<PathBuf>,
    map: Mutex<BTreeMap<String, Marker>>,
    /// Uma gravação por vez, sempre do mapa mais novo.
    writing: Mutex<()>,
    /// Acorda a lista do dono para publicar já, sem esperar o tique.
    pub changed: Notify,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

pub fn file_in(home: &Path) -> PathBuf { home.join(".hangar").join("nav").join("_pendentes.json") }

/// Entradas válidas do arquivo (`_nav_carregar`): `url` texto e `ts` número; o resto some calado.
pub fn parse(raw: &[u8]) -> BTreeMap<String, Marker> {
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(raw) else { return BTreeMap::new() };
    map.into_iter().filter_map(|(name, m)| {
        let url = m.get("url")?.as_str()?.to_owned();
        // `bool` também é `int` no Python: `true` vale 1.
        let ts = m.get("ts").and_then(|t| t.as_f64().or_else(|| t.as_bool().map(f64::from)))?;
        Some((name, Marker { url, ts }))
    }).collect()
}

impl NavStore {
    pub fn load(home: Option<&Path>) -> Self {
        let file = home.map(file_in);
        let map = file.as_deref().and_then(|f| std::fs::read(f).ok()).map(|raw| parse(&raw)).unwrap_or_default();
        Self { file, map: Mutex::new(map), writing: Mutex::new(()), changed: Notify::new() }
    }

    /// Dentro do prazo, por nome; vencer só filtra (a leitura não grava).
    pub fn live(&self, at: f64) -> Vec<(String, Marker)> {
        lock(&self.map).iter().filter(|(_, m)| at - m.ts <= TTL_SECS).map(|(n, m)| (n.clone(), m.clone())).collect()
    }

    pub fn get(&self, name: &str, at: f64) -> Option<Marker> {
        lock(&self.map).get(name).filter(|m| at - m.ts <= TTL_SECS).cloned()
    }

    pub fn put(self: &Arc<Self>, name: &str, url: String, at: f64) -> Marker {
        let marker = Marker { url, ts: at };
        lock(&self.map).insert(name.to_owned(), marker.clone());
        self.saved();
        marker
    }

    /// `true` se havia marcador.
    pub fn confirm(self: &Arc<Self>, name: &str) -> bool {
        let had = lock(&self.map).remove(name).is_some();
        if had { self.saved(); }
        had
    }

    /// Sessão que acabou (fechada, ou outra nasce no nome): a nova não herda o pedido.
    pub fn forget(self: &Arc<Self>, name: &str) { self.confirm(name); }

    pub fn rename(self: &Arc<Self>, old: &str, new: &str) {
        let mut map = lock(&self.map);
        let Some(m) = map.remove(old) else { return };
        map.insert(new.to_owned(), m);
        drop(map);
        self.saved();
    }

    /// Mudou: avisa a lista e grava fora das threads do runtime (sem runtime, aqui mesmo).
    fn saved(self: &Arc<Self>) {
        self.changed.notify_waiters();
        if self.file.is_none() { return; }
        let me = self.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(h) => { h.spawn_blocking(move || me.write()); }
            Err(_) => me.write(),
        }
    }

    fn write(&self) {
        let Some(file) = &self.file else { return };
        let _one = lock(&self.writing);
        let at = now();
        // O vencido sai do arquivo na próxima gravação.
        let body = {
            let mut map = lock(&self.map);
            map.retain(|_, m| at - m.ts <= TTL_SECS);
            serde_json::to_vec(&*map)
        };
        let Ok(body) = body else { return };
        if let Err(e) = write_private(file, &body) {
            tracing::warn!(code = "nav_persist_failed", kind = ?e.kind(), "navegador: pedido não gravado em disco");
        }
    }
}

/// Troca atômica com permissão 600: a url da página de rascunho leva o token do dono.
fn write_private(file: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = file.parent() { std::fs::create_dir_all(dir)?; }
    let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
    let mut f = options.open(&tmp)?;
    f.write_all(body)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, file)
}

static PAGE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^/api/sessions/([^/?#]+)/pages/([A-Za-z0-9_-]{1,128})$").unwrap());
static SCHEME: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^https?://").unwrap());

/// O `str.strip()` do Python também tira os separadores `\x1c`–`\x1f`.
fn py_space(c: char) -> bool { c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c) }

/// A url que o app abre, ou `(status, detalhe)` da recusa. Página de rascunho da própria sessão vem
/// como caminho sem token e é completada com o endereço local do servidor e o token do dono.
pub fn resolve_url(name: &str, raw: &str, listen: SocketAddr, token: &str) -> Result<String, (StatusCode, &'static str)> {
    let u = raw.trim_matches(py_space);
    if u.is_empty() { return Err((StatusCode::BAD_REQUEST, "url vazia")); }
    if u.starts_with('/') {
        let own = PAGE.captures(u).is_some_and(|c| percent_decode_str(&c[1]).decode_utf8_lossy() == name);
        if !own { return Err((StatusCode::BAD_REQUEST, "caminho relativo só vale para página desta sessão")); }
        // Bind em toda interface inclui o loopback; IP de LAN só escuta nele.
        let host = if listen.ip().is_unspecified() { std::net::IpAddr::from([127, 0, 0, 1]) } else { listen.ip() };
        let addr = SocketAddr::new(host, listen.port());
        return Ok(format!("http://{addr}{u}?token={}", utf8_percent_encode(token, TOKEN_SET)));
    }
    Ok(if SCHEME.is_match(u) { u.to_owned() } else { format!("http://{u}") })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenBody { url: String }

fn detail(status: StatusCode, msg: &str) -> Response { json_response(status, json!({"detail": msg})) }

pub async fn open(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    owner_route(st, peer, req).await
}

pub async fn confirm(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    owner_route(st, peer, req).await
}

/// Só o dono; o resto segue ao Python, que recusa (o convidado nunca pede navegador).
async fn owner_route(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    if !owner { return crate::routes::pass(&st, req, &fwd).await; }
    let headers = req.headers().clone();
    let mut r = serve(&st, req).await;
    crate::routes::cors(&headers, r.headers_mut());
    r
}

/// As duas rotas no caminho que a ponte do Python repassa (segmentos escapados).
pub(crate) fn matches(method: &Method, path: &str) -> bool {
    matches!(*method, Method::POST | Method::DELETE) && session_of(path).is_some()
}

fn session_of(path: &str) -> Option<String> {
    let seg = path.strip_prefix("/api/sessions/")?.strip_suffix("/nav")?;
    if seg.is_empty() || seg.contains('/') { return None; }
    percent_decode_str(seg).decode_utf8().ok().map(|n| n.into_owned())
}

/// O dono, pela 8765 ou pela ponte do Connect.
pub(crate) async fn serve(st: &Arc<AppState>, req: Request) -> Response {
    let Some(name) = session_of(req.uri().path()) else { return detail(StatusCode::NOT_FOUND, "Not Found") };
    // A trava vem antes do corpo, como a dependência `_transfer_guard` antes da validação.
    let _pass = match crate::plans::routes::enter(st, &name).await { Ok(p) => p, Err(r) => return r };
    if req.method() == Method::DELETE {
        st.list.nav.confirm(&name);
        return json_response(StatusCode::OK, json!({"ok": true}));
    }
    let Ok(bytes) = to_bytes(req.into_body(), BODY_LIMIT).await else { return crate::plans::routes::unprocessable_body("corpo grande demais") };
    let body = match serde_json::from_slice::<OpenBody>(&bytes) {
        Ok(b) if !b.url.is_empty() => b,
        Ok(_) => return crate::plans::routes::unprocessable_body("url vazia"),
        Err(e) => return crate::plans::routes::unprocessable_body(&e.to_string()),
    };
    let home = st.list.env().dirs.as_ref().map(|d| d.home.clone()).or_else(std::env::home_dir).unwrap_or_default();
    let local = crate::mcp::identity::Local { home: &home, mux: &st.list.env().mux };
    if !crate::mcp::identity::exists(&local, &name).await {
        return detail(StatusCode::NOT_FOUND, "sessão não encontrada");
    }
    let url = match resolve_url(&name, &body.url, st.cfg.listen, &st.cfg.auth_token) {
        Ok(u) => u,
        Err((status, msg)) => return detail(status, msg),
    };
    st.list.nav.put(&name, url.clone(), now());
    // Conversa aberta: o dono recebe já (convidado nunca, `side::owner_only`).
    st.side.hubs.deliver(&name, "nav", &json!({"url": url}).to_string());
    json_response(StatusCode::OK, json!({"ok": true}))
}

/// O pedido vivo da sessão para quem abre a conversa agora; só o dono.
pub(crate) fn chat_frame(st: &AppState, name: &str) -> Option<bytes::Bytes> {
    let m = st.list.nav.get(name, now())?;
    Some(crate::tail::sse_frame("nav", &json!({"url": m.url}).to_string(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listen(s: &str) -> SocketAddr { s.parse().unwrap() }

    #[test]
    fn parse_keeps_only_url_text_and_numeric_ts() {
        let got = parse(br#"{"a":{"url":"u","ts":5},"b":{"url":1,"ts":5},"c":{"url":"u"},"d":{"url":"u","ts":true},"e":"x"}"#);
        assert_eq!(got.keys().collect::<Vec<_>>(), ["a", "d"]);
        assert_eq!(got["d"].ts, 1.0);
        assert!(parse(b"[1]").is_empty());
        assert!(parse(b"nao json").is_empty());
    }

    fn golden() -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/nav/nav.json");
        serde_json::from_slice(&std::fs::read(path).expect("golden do Python (gen_nav.py)")).unwrap()
    }

    /// Cada url do golden sai igual à do Python, menos a só de espaços (N3: o Rust recusa).
    #[test]
    fn urls_match_the_python_route() {
        let g = golden();
        let port = g["port"].as_u64().unwrap() as u16;
        for c in g["urls"].as_array().unwrap() {
            let ip: std::net::IpAddr = c["bind"].as_str().unwrap().parse().unwrap();
            let got = resolve_url(c["name"].as_str().unwrap(), c["input"].as_str().unwrap(), SocketAddr::new(ip, port), c["token"].as_str().unwrap());
            if c["input"].as_str().unwrap().trim().is_empty() {
                assert_eq!(c["out"], "http://", "o Python manda http:// vazio");
                assert_eq!(got, Err((StatusCode::BAD_REQUEST, "url vazia")));
                continue;
            }
            match c["status"].as_u64().unwrap() {
                200 => assert_eq!(got.as_deref(), Ok(c["out"].as_str().unwrap()), "{c}"),
                s => assert_eq!(got.map_err(|(st, d)| (st.as_u16() as u64, d.to_owned())), Err((s, c["detail"].as_str().unwrap().to_owned())), "{c}"),
            }
        }
    }

    #[test]
    fn pending_files_load_like_python() {
        for c in golden()["files"].as_array().unwrap() {
            let mut live: Vec<String> = parse(c["raw"].as_str().unwrap().as_bytes()).into_iter()
                .filter(|(_, m)| now() - m.ts <= TTL_SECS).map(|(n, _)| n).collect();
            live.sort();
            assert_eq!(json!(live), c["live"], "{c}");
        }
    }

    #[test]
    fn blank_url_is_refused_instead_of_becoming_http() {
        assert_eq!(resolve_url("s", "   ", listen("0.0.0.0:8765"), "t"), Err((StatusCode::BAD_REQUEST, "url vazia")));
    }

    #[test]
    fn expired_marker_is_hidden_without_rewriting() {
        let store = Arc::new(NavStore::load(None));
        store.put("a", "http://x".into(), 1000.0);
        assert!(store.get("a", 1000.0 + TTL_SECS).is_some());
        assert!(store.get("a", 1000.0 + TTL_SECS + 0.5).is_none());
        assert!(store.live(1000.0 + TTL_SECS + 0.5).is_empty());
    }

    #[test]
    fn forget_and_rename_follow_the_session() {
        let store = Arc::new(NavStore::load(None));
        store.put("a", "http://x".into(), now());
        store.rename("a", "b");
        assert!(store.get("a", now()).is_none());
        assert_eq!(store.get("b", now()).unwrap().url, "http://x");
        store.forget("b");
        assert!(store.live(now()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private_and_reloads() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(NavStore::load(Some(dir.path())));
        store.put("a", "http://127.0.0.1:1/p?token=s".into(), now());
        let file = file_in(dir.path());
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        let again = NavStore::load(Some(dir.path()));
        assert_eq!(again.get("a", now()).unwrap().url, "http://127.0.0.1:1/p?token=s");
    }

    proptest::proptest! {
        #[test]
        fn parse_and_resolve_never_panic(raw in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256), url in ".{0,64}") {
            let _ = parse(&raw);
            let _ = resolve_url("s", &url, listen("[::1]:8765"), "tok en");
        }
    }
}
