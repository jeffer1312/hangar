//! Conferência de que um link de convite abre pela internet: o nome público existe e o Funnel
//! entrega a página desta máquina. O DNS da máquina não serve: na tailnet ele sempre responde.
use crate::routes::AppState;
use axum::{body::to_bytes, extract::Request, http::{Method, StatusCode, header}, response::{IntoResponse, Response}};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

pub const FUNNEL_PORT: u16 = 8443;
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PAGE: usize = 256 * 1024;
const EVERY: Duration = Duration::from_secs(5);
const DEADLINE: Duration = Duration::from_secs(180);
const BODY_LIMIT: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct InviteLink { pub host: String, pub path: String }

pub struct Probe {
    pub doh: Vec<String>,
    pub funnel_scheme: &'static str,
    pub funnel_port: u16,
    pub local_base: String,
}

impl Probe {
    pub fn production() -> Self {
        Self {
            doh: vec!["https://cloudflare-dns.com/dns-query".into(), "https://dns.google/resolve".into()],
            funnel_scheme: "https",
            funnel_port: FUNNEL_PORT,
            local_base: format!("http://127.0.0.1:{}", crate::share::tunnel::GUEST_PORT),
        }
    }
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "stage", rename_all = "lowercase")]
pub enum Failure { Dns { detail: String }, Funnel { detail: String } }

fn code_ok(code: &str) -> bool {
    (1..=64).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Só o link desta máquina: a rota não pode virar um cliente HTTP para qualquer endereço.
pub fn parse_link(link: &str, own_host: &str) -> Option<InviteLink> {
    let rest = link.strip_prefix("https://")?;
    let (authority, path) = rest.split_once('/')?;
    if own_host.is_empty() || authority != format!("{own_host}:{FUNNEL_PORT}") { return None; }
    let (kind, code) = path.split_once('/')?;
    if !matches!(kind, "convite" | "par") || !code_ok(code) { return None; }
    Some(InviteLink { host: own_host.to_owned(), path: format!("/{path}") })
}

#[derive(Deserialize)]
struct DohReply { #[serde(rename = "Answer", default)] answer: Vec<DohAnswer> }
#[derive(Deserialize)]
struct DohAnswer { #[serde(rename = "type")] kind: u16, data: String }

fn client() -> reqwest::Client {
    crate::http_client().no_proxy().redirect(reqwest::redirect::Policy::none())
        .build().expect("cliente http")
}

/// Cada chamada cabe no que resta do prazo, para a falha dela chegar antes do corte geral.
fn call_timeout(end: Option<Instant>) -> Duration {
    end.map_or(CALL_TIMEOUT, |end| CALL_TIMEOUT.min(end.saturating_duration_since(Instant::now())))
}

async fn public_ips(probe: &Probe, host: &str, end: Option<Instant>) -> Result<Vec<IpAddr>, Failure> {
    let http = client();
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("name", host).append_pair("type", "A").finish();
    let mut last = String::from("nenhum provedor de DNS respondeu");
    for url in &probe.doh {
        let reply = http.get(format!("{url}?{query}")).header(header::ACCEPT, "application/dns-json")
            .timeout(call_timeout(end)).send().await;
        let parsed = match reply {
            Ok(r) if r.status().is_success() => match r.bytes().await {
                Ok(b) => serde_json::from_slice::<DohReply>(&b).map_err(|e| format!("{url}: {e}")),
                Err(e) => Err(format!("{url}: {e}")),
            },
            Ok(r) => Err(format!("{url}: HTTP {}", r.status().as_u16())),
            Err(e) => Err(format!("{url}: {e}")),
        };
        match parsed {
            Ok(reply) => {
                let ips: Vec<IpAddr> = reply.answer.iter().filter(|a| a.kind == 1)
                    .filter_map(|a| a.data.parse().ok()).collect();
                if !ips.is_empty() { return Ok(ips); }
                last = format!("{host} ainda não existe no DNS público");
            }
            Err(e) => last = e,
        }
    }
    Err(Failure::Dns { detail: last })
}

async fn page(request: reqwest::RequestBuilder) -> Result<Vec<u8>, String> {
    let r = request.send().await.map_err(|e| e.to_string())?;
    if r.status() != reqwest::StatusCode::OK { return Err(format!("HTTP {}", r.status().as_u16())); }
    let body = r.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > MAX_PAGE { return Err("página grande demais".into()); }
    Ok(body.to_vec())
}

async fn attempt(probe: &Probe, link: &InviteLink, end: Option<Instant>) -> Result<(), Failure> {
    let ips = public_ips(probe, &link.host, end).await?;
    let local = page(client().get(format!("{}{}", probe.local_base, link.path)).timeout(call_timeout(end))).await
        .map_err(|e| Failure::Funnel { detail: format!("página local: {e}") })?;
    let url = format!("{}://{}:{}{}", probe.funnel_scheme, link.host, probe.funnel_port, link.path);
    let mut last = String::new();
    // O DNS público pode trazer mais de um endereço; basta um entregar a página desta máquina.
    for ip in ips {
        let outside = crate::http_client().no_proxy().redirect(reqwest::redirect::Policy::none())
            .resolve(&link.host, SocketAddr::new(ip, probe.funnel_port))
            .build().map_err(|e| Failure::Funnel { detail: e.to_string() })?;
        match page(outside.get(&url).timeout(call_timeout(end))).await {
            // O Funnel chega por loopback como o pedido local: a página só sai igual se for desta máquina.
            Ok(remote) if remote == local => return Ok(()),
            Ok(_) => last = format!("{ip}: outra máquina respondeu no endereço do link"),
            Err(e) => last = format!("{ip}: {e}"),
        }
    }
    Err(Failure::Funnel { detail: last })
}

pub async fn check_once(probe: &Probe, link: &InviteLink) -> Result<(), Failure> {
    attempt(probe, link, None).await
}

pub async fn check_until(probe: &Probe, link: &InviteLink, every: Duration, deadline: Duration) -> Result<(), Failure> {
    let end = Instant::now() + deadline;
    let mut last = None;
    let looped = tokio::time::timeout(deadline, async {
        loop {
            match attempt(probe, link, Some(end)).await {
                Ok(()) => return,
                Err(failure) => last = Some(failure),
            }
            tokio::time::sleep(every).await;
        }
    }).await;
    match (looped, last) {
        (Ok(()), _) => Ok(()),
        (Err(_), Some(failure)) => Err(failure),
        (Err(_), None) => Err(Failure::Dns { detail: "prazo esgotado".into() }),
    }
}

pub fn matches(method: &Method, path: &str) -> bool { *method == Method::POST && path == "/api/share/reachability" }

fn refused(status: StatusCode, code: &str, msg: &str) -> (StatusCode, Value) {
    (status, json!({"detail": {"code": code, "params": {}, "msg": msg}}))
}

#[derive(Deserialize)]
struct Body { link: String }

pub async fn answer(body: &[u8], own_host: Option<String>, probe: &Probe, every: Duration, deadline: Duration) -> (StatusCode, Value) {
    let Ok(Body { link }) = serde_json::from_slice::<Body>(body) else {
        return refused(StatusCode::BAD_REQUEST, "share_reach_link_refused", "link de convite inválido");
    };
    let Some(own) = own_host else {
        return refused(StatusCode::SERVICE_UNAVAILABLE, "share_reach_no_tailscale", "não deu para ler o nome desta máquina no Tailscale");
    };
    let Some(target) = parse_link(&link, &own) else {
        return refused(StatusCode::BAD_REQUEST, "share_reach_link_refused", "o link não é um convite desta máquina");
    };
    match check_until(probe, &target, every, deadline).await {
        Ok(()) => (StatusCode::OK, json!({"ok": true})),
        Err(failure) => {
            let mut v = serde_json::to_value(&failure).unwrap_or_default();
            v["ok"] = false.into();
            (StatusCode::OK, v)
        }
    }
}

/// O mesmo nome que o Python põe no link, lido sem passar a autoridade do servidor ao `tailscale`.
async fn own_host() -> Option<String> {
    crate::share::tunnel::host(&crate::share::tunnel::Real).await.ok()
}

fn json_response(status: StatusCode, v: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response()
}

/// Não-dono: 401 aqui mesmo. O Python não tem esta rota e responderia 404, que o cliente lê
/// como "precisa do servidor Rust"; a falha conta para o bloqueio como a de qualquer rota.
pub async fn unauthorized(st: &AppState, client_ip: &str) -> Response {
    if let Err(e) = st.auth.record_failure(client_ip).await { return crate::auth::unavailable(e); }
    let (s, v) = refused(StatusCode::UNAUTHORIZED, "erro_nao_autorizado", "unauthorized");
    json_response(s, v)
}

pub async fn public(_st: Arc<AppState>, req: Request) -> Response { reply(req).await }

async fn reply(req: Request) -> Response {
    let Ok(bytes) = to_bytes(req.into_body(), BODY_LIMIT).await else {
        let (s, v) = refused(StatusCode::PAYLOAD_TOO_LARGE, "share_reach_link_refused", "corpo grande demais");
        return json_response(s, v);
    };
    let (s, v) = answer(&bytes, own_host().await, &Probe::production(), EVERY, DEADLINE).await;
    json_response(s, v)
}
