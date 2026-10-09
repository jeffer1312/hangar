//! Fatos da orquestração que continuam no Python (`/internal/orq/*`): fase da execução do grupo,
//! promoção do time, quem é a linha do orquestrador. Também leva o pedido do texto do protocolo.
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use http_body_util::BodyExt;
use serde_json::{Value, json};

use super::service::{BoxFuture, OrqFacts, OrqPhase, PromoteError};
use crate::proxy::HttpClient;
use crate::routes::AppState;

/// A promoção mexe em arquivos; o resto é leitura. Ninguém fica preso a um Python mudo.
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY: usize = 1 << 20;

#[derive(Clone)]
pub struct PythonOrq { upstream: SocketAddr, secret: String, http: HttpClient }

impl PythonOrq {
    pub fn new(upstream: SocketAddr, secret: String, http: HttpClient) -> Self { Self { upstream, secret, http } }

    pub fn from_state(st: &AppState) -> Self { Self::new(st.cfg.upstream, st.cfg.internal_secret.clone(), st.http.clone()) }

    /// `POST /internal/{path}` → status e corpo (`Null` se não for JSON). `Err` é só código: motivo,
    /// nunca o corpo.
    pub(crate) async fn post(&self, path: &str, body: Value) -> Result<(u16, Value), String> {
        self.post_within(path, body, TIMEOUT).await
    }

    pub(crate) async fn post_within(&self, path: &str, body: Value, timeout: Duration) -> Result<(u16, Value), String> {
        let req = axum::http::Request::post(format!("http://{}/internal/{path}", self.upstream))
            .header("x-hangar-internal", &self.secret).header("content-type", "application/json")
            .body(Body::from(body.to_string())).map_err(|_| "groups_internal_request".to_owned())?;
        let work = async {
            let resp = self.http.request(req).await
                .map_err(|e| format!("groups_internal_{}", if e.is_connect() { "connect" } else { "unreachable" }))?;
            let status = resp.status().as_u16();
            let bytes = http_body_util::Limited::new(resp.into_body(), MAX_BODY).collect().await
                .map_err(|_| "groups_internal_body".to_owned())?.to_bytes();
            Ok((status, serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
        };
        tokio::time::timeout(timeout, work).await.map_err(|_| "groups_internal_timeout".to_owned())?
    }
}

fn warn(code: &str, what: &'static str) {
    if crate::warn_limit::allow(None, what) {
        tracing::warn!(code, what, "groups: o Python não respondeu o fato da orquestração");
    }
}

impl OrqFacts for PythonOrq {
    fn phase<'a>(&'a self, gid: &'a str) -> BoxFuture<'a, OrqPhase> {
        Box::pin(async move {
            match self.post("orq/group-phase", json!({"gid": gid})).await {
                Ok((200, reply)) => match reply.get("phase") {
                    Some(Value::Null) => OrqPhase::NotStarted,
                    Some(phase) if phase == "live" => OrqPhase::Live,
                    Some(phase) if phase == "ended" => OrqPhase::Ended,
                    _ => OrqPhase::Unknown,
                },
                Ok((status, _)) => { warn(&format!("groups_orq_phase_status:{status}"), "groups_orq_phase"); OrqPhase::Unknown }
                Err(code) => { warn(&code, "groups_orq_phase"); OrqPhase::Unknown }
            }
        })
    }

    fn promote<'a>(&'a self, name: &'a str, gid: &'a str) -> BoxFuture<'a, Result<(), PromoteError>> {
        Box::pin(async move {
            // Sem resposta ou com 5xx o Python pode ter promovido antes de falhar; o join volta atrás
            // mesmo assim, e o diário guarda o gid para quem for conferir o time.
            let (status, reply) = match self.post("orq/promote", json!({"name": name, "gid": gid})).await {
                Ok(answer) => answer,
                Err(code) => { self.uncertain(gid, &code); return Err(PromoteError::Unavailable(code)) }
            };
            match status {
                200 => Ok(()),
                // 409 traz o texto do conflito, como a rota `/pair` do Python.
                409 => Err(PromoteError::Conflict(reply["detail"]["msg"].as_str().map_or_else(|| "groups_orq_promote_status_409".to_owned(), str::to_owned))),
                _ => {
                    let code = format!("groups_orq_promote_status_{status}");
                    if status >= 500 { self.uncertain(gid, &code); }
                    Err(PromoteError::Unavailable(code))
                }
            }
        })
    }
}

impl PythonOrq {
    fn uncertain(&self, gid: &str, code: &str) {
        crate::diag::DiagClient::new(self.upstream, self.secret.clone())
            .report("rust.groups_orq_promote_uncertain", gid, code, "a promoção do orq não confirmou; o Python pode ter promovido");
    }
}

/// Envelope de lista que não respondeu (`erro_lista_indisponivel`): nunca "ninguém".
pub(crate) fn list_unavailable(code: &str) -> Value {
    json!({"code": "erro_lista_indisponivel", "params": {"detalhe": code},
        "msg": format!("a lista de sessões está indisponível ({code}), não vazia")})
}

/// Os de `names` que são a linha do orquestrador (a recusa `_recusa_orq` do Python).
pub async fn is_orchestrator(st: &AppState, names: &[String]) -> Result<Vec<String>, Value> {
    let failed = |code: String| list_unavailable(&code);
    match PythonOrq::from_state(st).post("orq/is-orchestrator", json!({"names": names})).await {
        Ok((200, reply)) => serde_json::from_value(reply["names"].clone()).map_err(|_| failed("groups_orq_names_invalid".into())),
        Ok((status, _)) => Err(failed(format!("groups_orq_names_status:{status}"))),
        Err(code) => Err(failed(code)),
    }
}
