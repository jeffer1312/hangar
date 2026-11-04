//! Fatos da orquestração lidos no cofre; o transporte genérico ainda serve a entregas legadas.
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
pub struct PythonOrq {
    upstream: SocketAddr,
    secret: String,
    http: HttpClient,
    home: std::path::PathBuf,
    pair: std::path::PathBuf,
    mux: String,
}

impl PythonOrq {
    pub fn new(upstream: SocketAddr, secret: String, http: HttpClient) -> Self {
        let env = crate::accounts::environment::AccountEnvironment::capture();
        Self {
            upstream,
            secret,
            http,
            home: env.home,
            pair: env.claude_base.join(".hangar-pair"),
            mux: if cfg!(windows) { "psmux" } else { "tmux" }.into(),
        }
    }
    pub fn with_paths(mut self, home: std::path::PathBuf, pair: std::path::PathBuf) -> Self {
        self.home = home;
        self.pair = pair;
        self
    }

    pub fn with_mux(mut self, mux: String) -> Self {
        self.mux = mux;
        self
    }

    pub fn from_state(st: &AppState) -> Self {
        Self::new(
            st.cfg.upstream,
            st.cfg.internal_secret.clone(),
            st.http.clone(),
        )
        .with_mux(st.list.env().capture_program.to_string_lossy().into_owned())
        .with_paths(
            st.accounts.env.home.clone(),
            st.groups
                .as_ref()
                .map(|g| g.pair_root().to_owned())
                .unwrap_or_else(|| st.accounts.env.claude_base.join(".hangar-pair")),
        )
    }

    /// `POST /internal/{path}` → status e corpo (`Null` se não for JSON). `Err` é só código: motivo,
    /// nunca o corpo.
    pub(crate) async fn post(&self, path: &str, body: Value) -> Result<(u16, Value), String> {
        self.post_within(path, body, TIMEOUT).await
    }

    pub(crate) async fn post_within(
        &self,
        path: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<(u16, Value), String> {
        let req = axum::http::Request::post(format!("http://{}/internal/{path}", self.upstream))
            .header("x-hangar-internal", &self.secret)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .map_err(|_| "groups_internal_request".to_owned())?;
        let work = async {
            let resp = self.http.request(req).await.map_err(|e| {
                format!(
                    "groups_internal_{}",
                    if e.is_connect() {
                        "connect"
                    } else {
                        "unreachable"
                    }
                )
            })?;
            let status = resp.status().as_u16();
            let bytes = http_body_util::Limited::new(resp.into_body(), MAX_BODY)
                .collect()
                .await
                .map_err(|_| "groups_internal_body".to_owned())?
                .to_bytes();
            Ok((
                status,
                serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            ))
        };
        tokio::time::timeout(timeout, work)
            .await
            .map_err(|_| "groups_internal_timeout".to_owned())?
    }
}

/// `runs.group_phase` em JSON: `null` = não começou; ausente ou outro valor = não se sabe.
pub(crate) fn phase_of(phase: Option<&Value>) -> OrqPhase {
    match phase {
        Some(Value::Null) => OrqPhase::NotStarted,
        Some(phase) if phase == "live" => OrqPhase::Live,
        Some(phase) if phase == "ended" => OrqPhase::Ended,
        _ => OrqPhase::Unknown,
    }
}

impl OrqFacts for PythonOrq {
    fn phase<'a>(&'a self, gid: &'a str) -> BoxFuture<'a, OrqPhase> {
        Box::pin(async move {
            let home = self.home.clone();
            let gid = gid.to_owned();
            match tokio::task::spawn_blocking(move || {
                crate::orchestration::runs::group_phase(&home, &gid)
            })
            .await
            {
                Ok(Some(phase)) if phase == "live" => OrqPhase::Live,
                Ok(Some(phase)) if phase == "ended" => OrqPhase::Ended,
                Ok(None) => OrqPhase::NotStarted,
                _ => OrqPhase::Unknown,
            }
        })
    }
    fn promote<'a>(
        &'a self,
        name: &'a str,
        gid: &'a str,
    ) -> BoxFuture<'a, Result<(), PromoteError>> {
        Box::pin(async move {
            let (home, pair, name, gid, mux) = (
                self.home.clone(),
                self.pair.clone(),
                name.to_owned(),
                gid.to_owned(),
                self.mux.clone(),
            );
            tokio::task::spawn_blocking(move || {
                let _lock = crate::orchestration::context::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                use crate::orchestration::context::PromotionError;
                crate::orchestration::context::promote_classified(&home, &pair, &name, &gid, None, &mux)
                    .map_err(|error| match error {
                        PromotionError::IdentityUnavailable(message) => PromoteError::IdentityUnavailable(message),
                        PromotionError::Conflict(message) | PromotionError::Changed(message) => PromoteError::Conflict(message),
                    })
            })
            .await
            .map_err(|_| PromoteError::Unavailable("groups_orq_promote_failed".into()))?
        })
    }
}

/// Envelope de lista que não respondeu (`erro_lista_indisponivel`): nunca "ninguém".
pub(crate) fn list_unavailable(code: &str) -> Value {
    json!({"code": "erro_lista_indisponivel", "params": {"detalhe": code},
        "msg": format!("a lista de sessões está indisponível ({code}), não vazia")})
}

/// Os de `names` que são a linha do orquestrador (a recusa `_recusa_orq` do Python).
pub async fn is_orchestrator(st: &AppState, names: &[String]) -> Result<Vec<String>, Value> {
    let home = st.accounts.env.home.clone();
    let names = names.to_vec();
    tokio::task::spawn_blocking(move || {
        crate::orchestration::runs::active(&home, crate::orchestration::runs::now()).map(|rows| {
            names
                .into_iter()
                .filter(|name| rows.iter().any(|row| row["name"] == name.as_str()))
                .collect()
        })
    })
    .await
    .map_err(|_| list_unavailable("groups_orq_names_failed"))?
    .map_err(|_| list_unavailable("groups_orq_names_unavailable"))
}
