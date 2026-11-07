//! Prévia de porta pelo Connect: o Caddy entrega aqui o site da prévia, e só quem abriu o link chega ao projeto.
//!
//! Nada deste listener passa por `hangar_auth`: quem erra aqui não enche o contador de tentativas do dono.
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use hangar_core::credentials;
use hyper_util::rt::TokioIo;
use subtle::ConstantTimeEq;
use tokio::net::TcpStream;
use tokio::sync::watch;

use super::config::{CADDY_HTTP_PORT, CADDY_HTTPS_PORT};
use super::devices::Clock;
use super::{CONNECT_PORT, GUEST_PORT};

pub const PREVIEW_PORT: u16 = 8769;
/// O link vai por mensagem para outro aparelho: dá tempo de copiar e abrir, e não fica valendo depois.
pub const LINK_LIFE: u64 = 10 * 60;
/// Nome próprio: `token` é do Vite, que o usa no WebSocket do projeto.
pub const LINK_PARAM: &str = "hangar_preview";
pub const COOKIE: &str = "__Host-hangar_preview";

/// Porta padrão do dono, recusada mesmo quando este servidor roda em outra.
const DEFAULT_MAIN_PORT: u16 = 8765;
const HANGAR_COOKIES: [&str; 4] = [COOKIE, "cp_token", "__Host-cp_token", "cp_sync"];
const HOP: [&str; 7] = ["connection", "keep-alive", "proxy-connection", "transfer-encoding", "te", "trailer", "upgrade"];
/// Credencial não passa ao projeto; o `Host` que ele vê é local, e o endereço público junto o confundiria.
const DROPPED: [&str; 4] = ["authorization", "proxy-authorization", "x-forwarded-host", "forwarded"];

struct Preview {
    port: u16,
    host: String,
    /// Hash e prazo do link, até o primeiro uso.
    link: Option<(String, u64)>,
    session: Option<String>,
}

impl Preview {
    fn redeem(&mut self, link: &str, now: u64) -> Option<String> {
        let (hash, expires) = self.link.as_ref()?;
        if now >= *expires || !same(hash, link) {
            return None;
        }
        self.link = None;
        let id = credentials::token();
        self.session = Some(credentials::hash(id.as_bytes()));
        Some(id)
    }
}

fn same(hash: &str, presented: &str) -> bool {
    credentials::hash(presented.as_bytes()).as_bytes().ct_eq(hash.as_bytes()).into()
}

fn same_host(presented: &str, host: &str) -> bool {
    presented.strip_suffix(":443").unwrap_or(presented).eq_ignore_ascii_case(host)
}

/// A prévia aberta e a sessão de quem entrou por ela. Só em memória: o servidor voltar encerra a prévia.
pub struct Previews {
    clock: Clock,
    reserved: Mutex<HashSet<u16>>,
    current: Mutex<Option<Preview>>,
    /// Muda a cada prévia aberta ou fechada: o que já estava passando sabe que acabou.
    generation: watch::Sender<u64>,
}

impl Default for Previews {
    fn default() -> Self { Self::new() }
}

enum Admission {
    Refused,
    OtherHost,
    /// Link gasto agora: a sessão nova, crua, sai só neste cookie.
    Entered(String),
    /// Sessão válida com um link na URL, gasto ou não: ele só precisa sair da barra de endereço.
    Clean,
    Pass(Visit),
}

struct Visit {
    port: u16,
    host: String,
    generation: watch::Receiver<u64>,
    seen: u64,
}

impl Visit {
    async fn ended(mut self) {
        let seen = self.seen;
        let _ = self.generation.wait_for(|now| *now != seen).await;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Previews {
    pub fn new() -> Self {
        let clock: Clock = Arc::new(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
        });
        let reserved = [DEFAULT_MAIN_PORT, GUEST_PORT, CONNECT_PORT, PREVIEW_PORT, CADDY_HTTP_PORT, CADDY_HTTPS_PORT].into_iter().collect();
        Self { clock, reserved: Mutex::new(reserved), current: Mutex::new(None), generation: watch::channel(0).0 }
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// As portas que só existem com o servidor de pé: a principal, a privada e as outras deste processo.
    pub fn reserve(&self, ports: impl IntoIterator<Item = u16>) {
        lock(&self.reserved).extend(ports);
    }

    pub fn refused(&self, port: u16) -> bool {
        port == 0 || lock(&self.reserved).contains(&port)
    }

    /// Devolve o link cru, que só existe nesta resposta; `None` é porta recusada. A prévia anterior acaba aqui.
    pub fn open(&self, port: u16, host: &str) -> Option<String> {
        if self.refused(port) {
            return None;
        }
        let link = credentials::token();
        let expires = (self.clock)() + LINK_LIFE;
        let mut current = lock(&self.current);
        *current = Some(Preview { port, host: host.to_owned(), link: Some((credentials::hash(link.as_bytes()), expires)), session: None });
        self.generation.send_modify(|generation| *generation += 1);
        Some(link)
    }

    pub fn close(&self) {
        let mut current = lock(&self.current);
        *current = None;
        self.generation.send_modify(|generation| *generation += 1);
    }

    fn admit(&self, host: Option<&str>, link: Option<&str>, session: Option<&str>, redeem: bool) -> Admission {
        let mut current = lock(&self.current);
        let Some(preview) = current.as_mut() else { return Admission::Refused };
        if !host.is_some_and(|host| same_host(host, &preview.host)) {
            return Admission::OtherHost;
        }
        if redeem && let Some(id) = link.and_then(|link| preview.redeem(link, (self.clock)())) {
            return Admission::Entered(id);
        }
        let known = preview.session.as_deref().zip(session).is_some_and(|(hash, session)| same(hash, session));
        if !known {
            return Admission::Refused;
        }
        if link.is_some() {
            return Admission::Clean;
        }
        // Lido com a prévia na mão: fechar depois daqui já conta como mudança para esta visita.
        let generation = self.generation.subscribe();
        let seen = *generation.borrow();
        Admission::Pass(Visit { port: preview.port, host: preview.host.clone(), generation, seen })
    }
}

/// O listener inteiro: o destino sai da prévia aberta, nunca do pedido.
pub fn router(previews: Arc<Previews>) -> Router {
    Router::new().fallback(enter).with_state(previews)
}

fn refusal(status: StatusCode) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")]).into_response()
}

/// `//x` e `/\x` seriam outro site para o navegador.
fn one_slash(path: &str) -> String {
    format!("/{}", path.trim_start_matches(['/', '\\']))
}

fn link_in(query: Option<&str>) -> Option<String> {
    form_urlencoded::parse(query?.as_bytes()).find(|(key, _)| key == LINK_PARAM).map(|(_, value)| value.into_owned())
}

fn without_link(uri: &Uri) -> String {
    let path = one_slash(uri.path());
    let kept: Vec<&str> = uri.query().unwrap_or_default().split('&')
        .filter(|pair| !pair.is_empty() && form_urlencoded::parse(pair.as_bytes()).next().is_none_or(|(key, _)| key != LINK_PARAM))
        .collect();
    if kept.is_empty() { path } else { format!("{path}?{}", kept.join("&")) }
}

fn session_in(headers: &HeaderMap) -> Option<&str> {
    headers.get_all(header::COOKIE).iter().filter_map(|value| value.to_str().ok()).flat_map(|raw| raw.split(';'))
        .find_map(|chunk| {
            let (name, value) = chunk.split_once('=')?;
            (name.trim() == COOKIE).then(|| value.trim())
        })
}

/// Sem cache: uma 303 guardada repetiria o caminho de um link já gasto.
fn see_other(target: &str, session: Option<&str>) -> Response {
    let mut response = refusal(StatusCode::SEE_OTHER);
    let headers = response.headers_mut();
    headers.insert(header::LOCATION, HeaderValue::from_str(target).unwrap_or(HeaderValue::from_static("/")));
    if let Some(id) = session && let Ok(cookie) = HeaderValue::from_str(&format!("{COOKIE}={id}; Secure; HttpOnly; SameSite=Lax; Path=/")) {
        headers.insert(header::SET_COOKIE, cookie);
    }
    response
}

fn is_websocket(req: &Request) -> bool {
    req.method() == Method::GET
        && req.headers().get(header::UPGRADE).is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"websocket"))
}

async fn enter(State(previews): State<Arc<Previews>>, req: Request) -> Response {
    // Forma absoluta e `CONNECT` trazem um destino no pedido; aqui só vale o caminho.
    if req.uri().authority().is_some() || req.method() == Method::CONNECT {
        return refusal(StatusCode::BAD_REQUEST);
    }
    let mut hosts = req.headers().get_all(header::HOST).iter();
    let host = hosts.next().and_then(|value| value.to_str().ok()).filter(|_| hosts.next().is_none());
    let link = link_in(req.uri().query());
    // Só GET gasta o link: o HEAD de quem monta miniatura em chat não pode queimá-lo.
    let admission = previews.admit(host, link.as_deref(), session_in(req.headers()), req.method() == Method::GET);
    let visit = match admission {
        Admission::Refused => return refusal(StatusCode::UNAUTHORIZED),
        Admission::OtherHost => return refusal(StatusCode::MISDIRECTED_REQUEST),
        Admission::Entered(id) => return see_other(&without_link(req.uri()), Some(&id)),
        Admission::Clean => return see_other(&without_link(req.uri()), None),
        Admission::Pass(visit) => visit,
    };
    if previews.refused(visit.port) {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY);
    }
    // As máquinas do Connect dividem o mesmo site: `SameSite` deixa o cookie ir junto no pedido de uma vizinha.
    if req.headers().get(header::ORIGIN).is_some_and(|origin| origin.as_bytes() != format!("https://{}", visit.host).as_bytes()) {
        return refusal(StatusCode::FORBIDDEN);
    }
    let websocket = is_websocket(&req);
    forward(req, visit, websocket).await
}

fn project_cookies(headers: &mut HeaderMap) {
    let kept: Vec<String> = headers.get_all(header::COOKIE).iter().filter_map(|value| value.to_str().ok())
        .flat_map(|raw| raw.split(';')).map(str::trim)
        .filter(|chunk| {
            let name = chunk.split_once('=').map_or(*chunk, |(name, _)| name).trim();
            !chunk.is_empty() && !HANGAR_COOKIES.contains(&name)
        })
        .map(str::to_owned).collect();
    headers.remove(header::COOKIE);
    if !kept.is_empty() && let Ok(value) = HeaderValue::from_str(&kept.join("; ")) {
        headers.insert(header::COOKIE, value);
    }
}

fn prepare(headers: &mut HeaderMap, visit: &Visit, websocket: bool) {
    for name in HOP.iter().chain(&DROPPED) {
        headers.remove(*name);
    }
    project_cookies(headers);
    let local = format!("127.0.0.1:{}", visit.port);
    let public_origin = format!("https://{}", visit.host);
    // O `vite` e o `next` recusam o host público; para eles o pedido é da própria máquina.
    if headers.get(header::ORIGIN).is_some_and(|origin| origin.as_bytes() == public_origin.as_bytes())
        && let Ok(origin) = HeaderValue::from_str(&format!("http://{local}")) {
        headers.insert(header::ORIGIN, origin);
    }
    if let Ok(host) = HeaderValue::from_str(&local) {
        headers.insert(header::HOST, host);
    }
    if websocket {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    }
}

/// O endereço local do projeto não abre no aparelho do visitante; destino de fora fica como veio.
fn public_location(location: &str, port: u16) -> Option<String> {
    let (scheme, rest) = location.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let (authority, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let authority = authority.to_ascii_lowercase();
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|host| authority == format!("{host}:{port}")).then(|| one_slash(tail))
}

/// Conexão própria por pedido, sempre no `127.0.0.1` da prévia, e redirecionamento nunca é seguido:
/// nada do que o visitante ou o projeto mandam escolhe outro destino.
async fn forward(mut req: Request, visit: Visit, websocket: bool) -> Response {
    let client_side = websocket.then(|| hyper::upgrade::on(&mut req));
    prepare(req.headers_mut(), &visit, websocket);
    let port = visit.port;
    let Ok(stream) = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await else {
        return refusal(StatusCode::BAD_GATEWAY);
    };
    let _ = stream.set_nodelay(true);
    let Ok((mut sender, conn)) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await else {
        return refusal(StatusCode::BAD_GATEWAY);
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });
    let Ok(mut response) = sender.send_request(req).await else {
        return refusal(StatusCode::BAD_GATEWAY);
    };
    if let Some(client_side) = client_side && response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let project_side = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            let (Ok(client), Ok(project)) = tokio::join!(client_side, project_side) else { return };
            let (mut client, mut project) = (TokioIo::new(client), TokioIo::new(project));
            tokio::select! {
                _ = tokio::io::copy_bidirectional(&mut client, &mut project) => {}
                () = visit.ended() => {}
            }
        });
        let (parts, _) = response.into_parts();
        return Response::from_parts(parts, Body::empty());
    }
    let (mut parts, body) = response.into_parts();
    for name in HOP {
        parts.headers.remove(name);
    }
    if let Some(target) = parts.headers.get(header::LOCATION).and_then(|value| value.to_str().ok()).and_then(|value| public_location(value, port))
        && let Ok(value) = HeaderValue::from_str(&target) {
        parts.headers.insert(header::LOCATION, value);
    }
    // Resposta longa, como um SSE, cai junto com a prévia.
    let stream = Body::new(body).into_data_stream().take_until(visit.ended());
    Response::from_parts(parts, Body::from_stream(stream))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_after_the_link_stays_on_this_site() {
        let target = |uri: &str| without_link(&uri.parse::<Uri>().unwrap());
        assert_eq!(target("/?hangar_preview=a"), "/");
        assert_eq!(target("/a/b?x=1&hangar_preview=a&y=2"), "/a/b?x=1&y=2");
        assert_eq!(target("/a?hangar%5Fpreview=a"), "/a", "codificado também sai");
        assert_eq!(target("//evil.example/?hangar_preview=a"), "/evil.example/");
        assert_eq!(target("/\\evil.example/?hangar_preview=a"), "/evil.example/");
    }

    #[test]
    fn location_of_the_project_itself_becomes_a_path() {
        assert_eq!(public_location("http://127.0.0.1:3000/a?b=1", 3000).as_deref(), Some("/a?b=1"));
        assert_eq!(public_location("HTTP://LOCALHOST:3000", 3000).as_deref(), Some("/"));
        assert_eq!(public_location("http://[::1]:3000//evil.example", 3000).as_deref(), Some("/evil.example"));
        for other in ["/a", "https://example.com/a", "http://127.0.0.1:8765/a", "http://127.0.0.1:30001/a", "mailto:a@b"] {
            assert_eq!(public_location(other, 3000), None, "{other}");
        }
    }

    #[test]
    fn only_project_cookies_are_kept() {
        let mut headers = HeaderMap::new();
        headers.append(header::COOKIE, HeaderValue::from_static("__Host-hangar_preview=a; app=1"));
        headers.append(header::COOKIE, HeaderValue::from_static("cp_token=b; __Host-cp_token=c; cp_sync=d; theme=dark"));
        project_cookies(&mut headers);
        assert_eq!(headers.get(header::COOKIE).unwrap(), "app=1; theme=dark");
        assert_eq!(headers.get_all(header::COOKIE).iter().count(), 1);
        headers.insert(header::COOKIE, HeaderValue::from_static("cp_token=b"));
        project_cookies(&mut headers);
        assert!(!headers.contains_key(header::COOKIE));
    }
}
