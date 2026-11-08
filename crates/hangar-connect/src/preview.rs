//! Prévia de porta pelo Connect: o Caddy entrega aqui o site da prévia, e só quem abriu o link chega ao projeto.
//!
//! Nada deste listener passa por `hangar_auth`: quem erra aqui não enche o contador de tentativas do dono.
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use hangar_core::credentials;
use hyper_util::rt::TokioIo;
use serde::Serialize;
use subtle::ConstantTimeEq;
use tokio::net::TcpStream;
use tokio::sync::watch;

use super::config::{CADDY_HTTP_PORT, CADDY_HTTPS_PORT};
use super::devices::{Clock, system_clock};
use super::{CONNECT_PORT, GUEST_PORT};

pub const PREVIEW_PORT: u16 = 8769;
/// O link vai por mensagem para outro aparelho: dá tempo de copiar e abrir, e não fica valendo depois.
pub const LINK_LIFE: u64 = 10 * 60;
/// Contada da entrada: prévia esquecida não fica aberta para sempre.
pub const SESSION_LIFE: u64 = 8 * 60 * 60;
/// Nome próprio: `token` é do Vite, que o usa no WebSocket do projeto.
pub const LINK_PARAM: &str = "hangar_preview";
pub const COOKIE: &str = "__Host-hangar_preview";

/// Porta padrão do dono, recusada mesmo quando este servidor roda em outra.
const DEFAULT_MAIN_PORT: u16 = 8765;
/// O CLIProxy local que a `jev_aux` usa.
const CLIPROXY_PORT: u16 = 8317;
const HANGAR_COOKIES: [&str; 4] = [COOKIE, "cp_token", "__Host-cp_token", "cp_sync"];
/// Sem script e sem envio automático: quem só busca o link para montar miniatura não o gasta.
const CONFIRM_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Hangar</title></head><body><form method=\"post\"><button type=\"submit\">Abrir prévia / Open preview</button></form></body></html>";
const HOP: [&str; 7] = ["connection", "keep-alive", "proxy-connection", "transfer-encoding", "te", "trailer", "upgrade"];
/// Credencial não passa ao projeto; o `Host` que ele vê é local, e o endereço público junto o confundiria.
const DROPPED: [&str; 4] = ["authorization", "proxy-authorization", "x-forwarded-host", "forwarded"];

struct Preview {
    port: u16,
    host: String,
    /// Hash e prazo do link, até o primeiro uso.
    link: Option<(String, u64)>,
    /// Hash e prazo da sessão de quem entrou.
    session: Option<(String, u64)>,
}

impl Preview {
    fn link_valid(&self, link: &str, now: u64) -> bool {
        self.link.as_ref().is_some_and(|(hash, expires)| now < *expires && same(hash, link))
    }

    fn redeem(&mut self, link: &str, now: u64) -> Option<String> {
        if !self.link_valid(link, now) {
            return None;
        }
        self.link = None;
        let id = credentials::token();
        self.session = Some((credentials::hash(id.as_bytes()), now + SESSION_LIFE));
        Some(id)
    }
}

/// O que `GET /api/connect/preview` mostra: prévia esquecida precisa aparecer para o dono.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Open {
    pub port: u16,
    pub host: String,
    pub entered: bool,
    pub expires_at: u64,
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
    /// Link válido num `GET`: a página pede o clique, sem gastá-lo.
    Confirm,
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
    left: Duration,
}

impl Visit {
    /// A prévia fechou, outra abriu ou a sessão venceu.
    fn ended(&self) -> impl Future<Output = ()> + Send + use<> {
        let (mut generation, seen, left) = (self.generation.clone(), self.seen, self.left);
        async move {
            tokio::select! {
                () = async { let _ = generation.wait_for(|now| *now != seen).await; } => {}
                () = tokio::time::sleep(left) => {}
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Previews {
    pub fn new() -> Self {
        let clock = system_clock();
        let reserved = [DEFAULT_MAIN_PORT, GUEST_PORT, CONNECT_PORT, PREVIEW_PORT, CADDY_HTTP_PORT, CADDY_HTTPS_PORT, CLIPROXY_PORT].into_iter().collect();
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

    /// A prévia aberta e ainda valendo.
    pub fn state(&self) -> Option<Open> {
        let now = (self.clock)();
        let current = lock(&self.current);
        let preview = current.as_ref()?;
        let (entered, expires_at) = match (&preview.session, &preview.link) {
            (Some((_, expires)), _) => (true, *expires),
            (None, Some((_, expires))) => (false, *expires),
            (None, None) => return None,
        };
        (now < expires_at).then(|| Open { port: preview.port, host: preview.host.clone(), entered, expires_at })
    }

    fn admit(&self, host: Option<&str>, link: Option<&str>, session: Option<&str>, method: &Method, origin: Option<&HeaderValue>) -> Admission {
        let now = (self.clock)();
        let mut current = lock(&self.current);
        let Some(preview) = current.as_mut() else { return Admission::Refused };
        if !host.is_some_and(|host| same_host(host, &preview.host)) {
            return Admission::OtherHost;
        }
        // Só o botão da página do próprio site gasta o link.
        let from_site = origin.is_some_and(|origin| origin.as_bytes() == format!("https://{}", preview.host).as_bytes());
        if *method == Method::POST && from_site && let Some(id) = link.and_then(|link| preview.redeem(link, now)) {
            return Admission::Entered(id);
        }
        let known = preview.session.as_ref()
            .filter(|(hash, expires)| now < *expires && session.is_some_and(|session| same(hash, session)))
            .map(|(_, expires)| *expires);
        let Some(expires) = known else {
            let valid = *method == Method::GET && link.is_some_and(|link| preview.link_valid(link, now));
            return if valid { Admission::Confirm } else { Admission::Refused };
        };
        if link.is_some() {
            return Admission::Clean;
        }
        // Lido com a prévia na mão: fechar depois daqui já conta como mudança para esta visita.
        let generation = self.generation.subscribe();
        let seen = *generation.borrow();
        Admission::Pass(Visit { port: preview.port, host: preview.host.clone(), generation, seen, left: Duration::from_secs(expires - now) })
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
    if let Some(id) = session && let Ok(cookie) = HeaderValue::from_str(&format!("{COOKIE}={id}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age={SESSION_LIFE}")) {
        headers.insert(header::SET_COOKIE, cookie);
    }
    response
}

fn confirm_page() -> Response {
    ([
        (header::CONTENT_TYPE, "text/html; charset=utf-8"),
        (header::CACHE_CONTROL, "no-store"),
        // `no-referrer` faria o navegador mandar `Origin: null` no botão, e o link não seria gasto.
        (header::REFERRER_POLICY, "same-origin"),
        (header::CONTENT_SECURITY_POLICY, "default-src 'none'; form-action 'self'; frame-ancestors 'none'"),
    ], CONFIRM_PAGE).into_response()
}

fn header_bytes<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a [u8]> {
    headers.get(name).map(HeaderValue::as_bytes)
}

/// As máquinas do Connect dividem o mesmo site: `SameSite` deixa o cookie ir junto no pedido de uma vizinha.
/// Abrir o site numa aba, vindo de qualquer lugar, é a única entrada de fora.
fn from_outside(headers: &HeaderMap, host: &str) -> bool {
    if headers.get(header::ORIGIN).is_some_and(|origin| origin.as_bytes() != format!("https://{host}").as_bytes()) {
        return true;
    }
    let inside = matches!(header_bytes(headers, "sec-fetch-site"), None | Some(b"same-origin" | b"none"));
    let opened = header_bytes(headers, "sec-fetch-mode") == Some(b"navigate".as_slice())
        && header_bytes(headers, "sec-fetch-dest") == Some(b"document".as_slice());
    !inside && !opened
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
    // O host é fixo por máquina: um service worker de prévia velha leria o link e o cookie da seguinte.
    if header_bytes(req.headers(), "service-worker") == Some(b"script".as_slice()) {
        return refusal(StatusCode::FORBIDDEN);
    }
    let mut hosts = req.headers().get_all(header::HOST).iter();
    let host = hosts.next().and_then(|value| value.to_str().ok()).filter(|_| hosts.next().is_none());
    let link = link_in(req.uri().query());
    let admission = previews.admit(host, link.as_deref(), session_in(req.headers()), req.method(), req.headers().get(header::ORIGIN));
    let visit = match admission {
        Admission::Refused => return refusal(StatusCode::UNAUTHORIZED),
        Admission::OtherHost => return refusal(StatusCode::MISDIRECTED_REQUEST),
        Admission::Confirm => return confirm_page(),
        Admission::Entered(id) => return see_other(&without_link(req.uri()), Some(&id)),
        Admission::Clean => return see_other(&without_link(req.uri()), None),
        Admission::Pass(visit) => visit,
    };
    if previews.refused(visit.port) {
        return refusal(StatusCode::UNPROCESSABLE_ENTITY);
    }
    if from_outside(req.headers(), &visit.host) {
        return refusal(StatusCode::FORBIDDEN);
    }
    let websocket = is_websocket(&req);
    // Projeto que nunca responde não segura o pedido depois do fim da prévia.
    let ended = visit.ended();
    tokio::select! {
        response = forward(req, visit, websocket) => response,
        () = ended => refusal(StatusCode::UNAUTHORIZED),
    }
}

fn is_hangar_cookie(name: &str) -> bool {
    HANGAR_COOKIES.iter().any(|known| known.eq_ignore_ascii_case(name))
}

fn cookie_name(chunk: &str) -> &str {
    chunk.split_once('=').map_or(chunk, |(name, _)| name).trim()
}

fn project_cookies(headers: &mut HeaderMap) {
    let kept: Vec<String> = headers.get_all(header::COOKIE).iter().filter_map(|value| value.to_str().ok())
        .flat_map(|raw| raw.split(';')).map(str::trim)
        .filter(|chunk| !chunk.is_empty() && !is_hangar_cookie(cookie_name(chunk)))
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
    // A página do botão manda o próprio endereço, com o link já gasto, no pedido depois da 303.
    if header_bytes(headers, "referer").is_some_and(|referer| String::from_utf8_lossy(referer).contains(LINK_PARAM)) {
        headers.remove(header::REFERER);
    }
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

/// Sem `Domain` o cookie fica neste host: com ele, o projeto encheria o `Cookie` de todas as máquinas do site.
fn project_set_cookie(value: &HeaderValue) -> Option<HeaderValue> {
    let mut parts = value.to_str().ok()?.split(';');
    let pair = parts.next()?;
    if is_hangar_cookie(cookie_name(pair)) {
        return None;
    }
    let kept: Vec<&str> = std::iter::once(pair).chain(parts.filter(|part| !cookie_name(part).eq_ignore_ascii_case("domain"))).collect();
    HeaderValue::from_str(&kept.join(";")).ok()
}

/// O navegador cumpre estes cabeçalhos para o site inteiro, não só para o projeto.
fn project_response(headers: &mut HeaderMap) {
    headers.remove("clear-site-data");
    let cookies: Vec<HeaderValue> = headers.get_all(header::SET_COOKIE).iter().filter_map(project_set_cookie).collect();
    headers.remove(header::SET_COOKIE);
    for cookie in cookies {
        headers.append(header::SET_COOKIE, cookie);
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
    project_response(response.headers_mut());
    if let Some(client_side) = client_side && response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let project_side = hyper::upgrade::on(&mut response);
        let ended = visit.ended();
        tokio::spawn(async move {
            let (Ok(client), Ok(project)) = tokio::join!(client_side, project_side) else { return };
            let (mut client, mut project) = (TokioIo::new(client), TokioIo::new(project));
            tokio::select! {
                _ = tokio::io::copy_bidirectional(&mut client, &mut project) => {}
                () = ended => {}
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
