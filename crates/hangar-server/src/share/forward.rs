//! Convidado repassado ao Python: só o Rust decide quem ele é, e diz isso num cabeçalho assinado.
use std::collections::BTreeMap;

use axum::extract::Request;
use axum::http::{HeaderValue, Uri, header};
use base64::Engine;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use super::caller::GuestGrant;
use super::gate::CLOSE_ENDED;
use super::store::Kind;

pub const HEADER: &str = "x-hangar-guest";
/// Curto: o cabeçalho vale para um pedido só, não é credencial que se guarde.
pub const VALID_SECS: u64 = 30;

/// `<base64url(json {s, exp})>.<hex hmac>` sobre `método\ncaminho cru\npayload`.
pub fn sign(secret: &str, method: &str, raw_path: &str, grant: &GuestGrant, now: u64) -> HeaderValue {
    let sessions: BTreeMap<&str, Kind> = grant.sessions.iter().map(|(n, k)| (n.as_str(), *k)).collect();
    let json = serde_json::json!({"s": sessions, "exp": now + VALID_SECS}).to_string();
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let mac: String = ring::hmac::sign(&key, format!("{method}\n{raw_path}\n{payload}").as_bytes())
        .as_ref().iter().map(|b| format!("{b:02x}")).collect();
    HeaderValue::from_str(&format!("{payload}.{mac}")).expect("base64url e hex são ASCII")
}

/// Tira o token de convite (Authorization e `?token=`) e põe o cabeçalho assinado: com o token, o
/// `GuestUserGate` do Python o confundiria com convidado de login e daria 401.
pub fn prepare_guest_request(req: &mut Request, secret: &str, grant: &GuestGrant, now: u64) {
    req.headers_mut().remove(header::AUTHORIZATION);
    let path = req.uri().path().to_owned();
    if let Some(query) = req.uri().query() {
        // Recorta a query crua: decodificar e recodificar mudaria os bytes dos outros parâmetros.
        let kept: Vec<&str> = query.split('&')
            .filter(|p| p.split('=').next().is_none_or(|k| k != "token")).collect();
        let path_and_query = if kept.is_empty() { path.clone() } else { format!("{path}?{}", kept.join("&")) };
        let mut parts = req.uri().clone().into_parts();
        parts.path_and_query = path_and_query.parse().ok();
        if let Ok(uri) = Uri::from_parts(parts) {
            *req.uri_mut() = uri;
        }
    }
    let value = sign(secret, req.method().as_str(), &path, grant, now);
    req.headers_mut().insert(HEADER, value);
}

/// Cano de bytes do WebSocket repassado: quando o acesso acaba, o cliente recebe o fechamento 4410
/// (o Python não sabe da revogação) e o cano cai.
pub async fn pipe_watched<C, U>(mut client: C, mut upstream: U, ended: impl Future<Output = ()>)
where C: AsyncRead + AsyncWrite + Unpin, U: AsyncRead + AsyncWrite + Unpin {
    // ponytail: o quadro de fechamento pode cair no meio de um quadro do Python já cortado; o
    // cliente vê protocolo quebrado em vez de 4410 nesse caso raro, e a conexão cai do mesmo jeito.
    let done = tokio::select! {
        _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => false,
        _ = ended => true,
    };
    if done {
        let [hi, lo] = CLOSE_ENDED.to_be_bytes();
        let _ = client.write_all(&[0x88, 0x02, hi, lo]).await;
        let _ = client.shutdown().await;
        let _ = upstream.shutdown().await;
    }
}
