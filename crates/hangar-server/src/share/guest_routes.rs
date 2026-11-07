//! Rotas do convidado que não pedem sessão (`share_guest_api.py`): a página do link, o resgate do
//! código e o `attach` de `external_pair_api.py`.
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::{Extension, Router, body::Bytes, extract::{Path, State}, http::{StatusCode, header}, response::{IntoResponse, Response}, routing::{get, post}};
use serde_json::json;

use super::ShareState;
use super::caller::Caller;
use super::owner_routes::io_failed;
use super::pair_routes::{code_error, fields, page, reason, respond};
use super::store::{Kind, PeekError};
use super::tunnel::{GUEST_PORT, TunnelError};
use crate::session_write::{detail, json_response};
use crate::workspace_routes::html_escape;

/// Endereço LOCAL da conexão na 8766: Funnel e Caddy entregam por loopback, a rede da casa pelo IP dela.
/// Quem aceita a conexão põe nas extensões do pedido.
#[derive(Clone, Copy, Debug)]
pub struct LocalAddr(pub SocketAddr);

struct GuestRoutes {
    share: Arc<ShareState>,
    owner: String,
}

type St = State<Arc<GuestRoutes>>;

/// Nome do dono na página e no resgate: `HANGAR_SERVER_ID` (o `CP_SERVER_ID`), senão o da máquina.
pub fn owner_name() -> String {
    super::machine_name(&std::env::var("HANGAR_SERVER_ID").unwrap_or_default())
}

/// `quote(s, safe='')`: só letras, dígitos e `_.-~` passam.
const QUOTE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~');

/// IP da rede por onde o convidado chegou; `None` quando veio pela internet (loopback).
fn lan_ip(local: Option<Extension<LocalAddr>>) -> Option<IpAddr> {
    let ip = local?.0.0.ip().to_canonical();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// `host:porta` com IPv6 entre colchetes.
fn lan_host(ip: IpAddr) -> String { SocketAddr::new(ip, GUEST_PORT).to_string() }

async fn public_host(share: &ShareState) -> Result<String, TunnelError> {
    Ok(share.public_guest_url().await?.trim_start_matches("https://").to_owned())
}

fn html(status: StatusCode, body: String, referrer: bool) -> Response {
    let mut resp = (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], body).into_response();
    if referrer {
        resp.headers_mut().insert(header::REFERRER_POLICY, header::HeaderValue::from_static("no-referrer"));
    }
    resp
}

async fn invite_page(State(st): St, Path(code): Path<String>, local: Option<Extension<LocalAddr>>) -> Response {
    // GET nunca gasta o código: o WhatsApp abre o link sozinho pra montar a prévia.
    let body = match st.share.store.peek(&code, Kind::Share) {
        Err(e) => format!("<h1>Convite indisponível</h1><p>{}.</p>", html_escape(reason(&e).2)),
        Ok(share) => {
            let host = match lan_ip(local) {
                Some(ip) => lan_host(ip),
                None => match public_host(&st.share).await {
                    Ok(h) => h,
                    Err(_) => return html(StatusCode::SERVICE_UNAVAILABLE, page(
                        "<h1>Convite indisponível</h1><p>O convite está indisponível por instantes. Tente de novo.</p>"), false),
                },
            };
            let app_link = format!("hangar://convite/{host}/{}", percent_encoding::utf8_percent_encode(&code, QUOTE));
            format!(
                "<h1>{} compartilhou uma sessão</h1><p>Sessão <b>{}</b>. Abra no teu Hangar para ela aparecer na tua lista.</p>\
                 <a class=\"p\" href=\"{}\">Abrir no app Hangar</a>\
                 <button onclick=\"navigator.clipboard.writeText(location.href);this.textContent='Convite copiado'\">Copiar convite</button>\
                 <p>No Hangar web ou no celular: Configurações → Máquinas → Colar convite.</p>",
                html_escape(&st.owner), html_escape(&share.session), html_escape(&app_link))
        }
    };
    html(StatusCode::OK, page(&body), true)
}

async fn redeem(State(st): St, local: Option<Extension<LocalAddr>>, body: Bytes) -> Response {
    let [code, device, token] = match fields(&body, &[("code", true, None), ("device", false, None), ("token", false, None)], true) {
        Ok((v, _)) => <[String; 3]>::try_from(v).expect("três campos"),
        Err(r) => return r,
    };
    // O endereço sai ANTES do resgate: túnel fora do ar não pode queimar um código já gasto.
    let address = match lan_ip(local) {
        Some(ip) => format!("http://{}", lan_host(ip)),
        None => match public_host(&st.share).await {
            Ok(h) => format!("https://{h}"),
            Err(_) => return detail(StatusCode::SERVICE_UNAVAILABLE, "erro_sessao_indisponivel",
                "a sessão compartilhada está indisponível por instantes", json!({})),
        },
    };
    match st.share.store.redeem(&code, &device, Some(&token), Kind::Share) {
        Ok((share, token)) => json_response(StatusCode::OK,
            json!({"token": token, "session": share.session, "owner": st.owner, "address": address})),
        Err(PeekError::Io(e)) => io_failed(e),
        Err(e) => respond(code_error(&e)),
    }
}

async fn attach(State(st): St, caller: Option<Extension<Caller>>, body: Bytes) -> Response {
    let Some(Extension(Caller::Guest(grant))) = caller else {
        return detail(StatusCode::FORBIDDEN, "erro_fora_do_convite", "só convidado", json!({}));
    };
    let [other] = match fields(&body, &[("token", true, None)], true) {
        Ok((v, _)) => <[String; 1]>::try_from(v).expect("um campo"),
        Err(r) => return r,
    };
    let store = &st.share.store;
    // O token a ligar é outro: o porteiro só resolveu o de quem chama.
    if store.lookup_token(&other).is_none_or(|g| g.sessions().is_empty()) {
        return detail(StatusCode::CONFLICT, "erro_par_attach_invalido", "token a ligar inválido ou sem sessão viva", json!({}));
    }
    match store.attach(&grant.token, &other) {
        Ok(n) => json_response(StatusCode::OK, json!({"attached": n})),
        Err(e) => io_failed(e),
    }
}

/// Rotas do convidado na 8766; o porteiro (`gate::guard`) fica na frente e deixa a página e o
/// resgate passarem sem token.
pub fn guest_router(share: Arc<ShareState>, owner: String) -> Router {
    Router::new()
        .route("/convite/{code}", get(invite_page))
        .route("/api/guest/redeem", post(redeem))
        .route("/api/guest/attach", post(attach))
        .with_state(Arc::new(GuestRoutes { share, owner }))
}
