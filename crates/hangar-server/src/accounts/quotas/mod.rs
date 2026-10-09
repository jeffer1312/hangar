pub mod cache;
pub mod claude;
pub mod codex;
pub mod reset;

use super::http::Json;
use crate::accounts::{
    AccountKey, GuardMode, Provider,
    bridge::AccountsBridge,
    catalog::{Account, AccountError, AccountService},
};
use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

fn response(result: Result<Value, AccountError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => super::http::error(error),
    }
}

pub async fn public(state: Arc<crate::routes::AppState>, request: Request) -> Response {
    let suggestion_only = request.uri().path().ends_with("/sugestao");
    let query: std::collections::HashMap<String, String> =
        form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let force = match crate::query::bool_param(&query, "forcar") {
        Ok(force) => force,
        Err(response) => return *response,
    };
    let bridge = match super::http::bridge(&state) {
        Ok(bridge) => bridge,
        Err(error) => return response(Err(error)),
    };
    let result = state.accounts.quotas(bridge, force, false).await;
    if suggestion_only {
        return match result {
            Ok(rows) => match rows.as_array().and_then(|rows| suggestion(rows)) {
                Some(value) => Json(value).into_response(),
                None => (
                    StatusCode::NOT_FOUND,
                    Json(json!({"detail":"sem-conta-legivel"})),
                )
                    .into_response(),
            },
            Err(error) => response(Err(error)),
        };
    }
    response(result)
}

pub async fn private(
    axum::extract::State(state): axum::extract::State<Arc<crate::routes::AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    request: Request,
) -> Response {
    if !crate::workspace_routes::private_ok(&state, peer, request.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        force: bool,
        cached_only: bool,
        invalidate: Option<String>,
    }
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 16384).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(body) = serde_json::from_slice::<Input>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if let Some(id) = body.invalidate {
        let mut cache = state.accounts.quotas.cache.lock().await;
        let cache =
            cache.get_or_insert_with(|| cache::QuotaCache::load(&state.accounts.quota_path()));
        cache.remove(&id);
        return match cache.save(&state.accounts.quota_path()) {
            Ok(()) => Json(json!({"ok":true})).into_response(),
            Err(_) => response(Err(AccountError::io())),
        };
    }
    let bridge = match super::http::bridge(&state) {
        Ok(bridge) => bridge,
        Err(error) => return response(Err(error)),
    };
    response(
        state
            .accounts
            .quotas(bridge, body.force, body.cached_only)
            .await,
    )
}

#[derive(Clone, Default)]
pub struct Quotas {
    pub cache: Arc<Mutex<Option<cache::QuotaCache>>>,
    /// Um refresh de rede por vez; quem só lê o cache não entra nesta fila.
    pub refresh: Arc<Mutex<()>>,
    pub runtime: Arc<std::sync::OnceLock<Arc<crate::runtime::gateway::RuntimeRegistry>>>,
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub fn timestamp(value: &Value) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|dt| dt.timestamp() as f64 + dt.timestamp_subsec_nanos() as f64 / 1_000_000_000.0)
}

pub fn window(
    value: &Value,
    label: &str,
    percent: &str,
    reset: &str,
    scoped: bool,
) -> Option<Value> {
    Some(json!({"rotulo":label,"pct":value[percent].as_f64()?,
        "reset_ts":timestamp(&value[reset]),"por_modelo":scoped}))
}

pub fn reading(state: &str, windows: Vec<Value>, reason: Option<&str>) -> Value {
    json!({"estado":state,"janelas":windows,"motivo":reason,"ts":if state == "lida" {Some(now())} else {None},
        "idade_s":null,"reset_credits":null,"refresh_expires_at":null})
}

pub fn suggestion(rows: &[Value]) -> Option<Value> {
    rows.iter()
        .filter(|row| row["provedor"] == "claude" && row["estado"] == "lida")
        .filter_map(|row| {
            let usage = row["janelas"]
                .as_array()?
                .iter()
                .filter_map(|w| w["pct"].as_f64())
                .reduce(f64::max)?;
            Some((100.0 - usage, row["ativa"] == true, row))
        })
        .reduce(|best, next| {
            if (next.0, next.1) > (best.0, best.1) {
                next
            } else {
                best
            }
        })
        .map(|(headroom, active, row)| {
            json!({"id":row["id"],"label":row["label"],
            "path":row["id"].as_str().unwrap_or("").strip_prefix("claude:").unwrap_or(""),
            "ativa":active,"folga":headroom})
        })
}

impl AccountService {
    pub fn quota_path(&self) -> PathBuf {
        if cfg!(windows) {
            self.env
                .base
                .get("LOCALAPPDATA")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| self.env.home.join("AppData/Local"))
                .join("hangar/cotas-cache.json")
        } else {
            self.env.home.join(".hangar/cotas-cache.json")
        }
    }

    pub async fn quotas(
        &self,
        bridge: AccountsBridge,
        force: bool,
        cached_only: bool,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        // O worker conserva as guardas e termina a publicação se o cliente desconectar.
        tokio::spawn(async move { service.quotas_owned(bridge, force, cached_only).await })
            .await
            .map_err(|_| AccountError::io())?
    }

    async fn quotas_owned(
        &self,
        bridge: AccountsBridge,
        force: bool,
        cached_only: bool,
    ) -> Result<Value, AccountError> {
        let _refresh = if cached_only {
            None
        } else {
            Some(self.quotas.refresh.lock().await)
        };
        let facts = bridge
            .quotas("sources", &[])
            .await
            .map_err(|_| AccountError::io())?;
        let mut sources = vec![];
        for (row, account) in self.claude_accounts()? {
            let path = row["path"].as_str().ok_or_else(AccountError::io)?;
            if !account.is_default
                && !crate::accounts::storage::real_file(
                    &account.home.join(crate::accounts::storage::CLAUDE_MARKER),
                )
            {
                continue;
            }
            sources.push((json!({"id":format!("claude:{path}"),"label":row["label"],"provedor":"claude","ativa":row["active"]}),Some((Provider::Claude, account))));
        }
        for account in self.visible_codex_accounts()? {
            let path = crate::accounts::catalog::resolved(&account.home);
            sources.push((json!({"id":format!("codex:{}",path.to_string_lossy()),
                "label":if account.is_default {"Codex"} else {&account.id},"provedor":"codex","ativa":false}),Some((Provider::Codex,account))));
        }
        for row in facts["sources"].as_array().ok_or_else(AccountError::io)? {
            if row["provedor"] == "claude" || row["provedor"] == "codex" {
                return Err(AccountError::io());
            }
            sources.push((row.clone(), None));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| AccountError::io())?;
        let signatures: std::collections::HashMap<String, Value> = sources
            .iter()
            .filter_map(|(row, account)| {
                let (provider, account) = account.as_ref()?;
                let file = if *provider == Provider::Claude {
                    ".credentials.json"
                } else {
                    "auth.json"
                };
                let signature = std::fs::read(account.home.join(file)).ok().map(|bytes| {
                    json!(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref())
                });
                Some((
                    row["id"].as_str()?.to_owned(),
                    signature.unwrap_or(Value::Null),
                ))
            })
            .collect();
        let selected: Vec<_> = {
            let mut held = self.quotas.cache.lock().await;
            let cache = held.get_or_insert_with(|| cache::QuotaCache::load(&self.quota_path()));
            for (id, signature) in &signatures {
                // Um logout ou troca de token não pode conservar a cota da identidade anterior.
                if cache.credential_changed(id, signature) {
                    cache.remove(id);
                }
                cache.set_credential(id, signature.clone());
            }
            let at = now();
            sources
                .iter()
                .filter(|(row, _)| {
                    !cached_only
                        && row["id"]
                            .as_str()
                            .is_some_and(|id| cache.needs_refresh(id, at, force))
                })
                .cloned()
                .collect()
        };
        // A rede roda sem o cache trancado: a leitura só do cache não espera as outras fontes.
        let mut readings = vec![];
        {
            let mut jobs = futures_util::stream::iter(selected)
                .map(|(source, account)| {
                    let client = client.clone();
                    let bridge = bridge.clone();
                    let service = self.clone();
                    async move {
                        let id = source["id"].as_str().unwrap_or("");
                        let (mut value, signature) = if let Some((provider, account)) = account {
                            service
                                .read_quota(provider, &account, &client, &bridge)
                                .await
                        } else {
                            (
                                bridge
                                    .quotas("read", &[id.to_owned()])
                                    .await
                                    .ok()
                                    .and_then(|body| body["readings"][id].as_object().cloned())
                                    .map(Value::Object)
                                    .unwrap_or_else(|| {
                                        reading("indisponivel", vec![], Some("sem-resposta"))
                                    }),
                                None,
                            )
                        };
                        for field in ["id", "label", "provedor", "ativa"] {
                            value[field] = source[field].clone();
                        }
                        (id.to_owned(), value, signature)
                    }
                })
                .buffer_unordered(8);
            use futures_util::StreamExt;
            while let Some(reading) = jobs.next().await {
                readings.push((reading, now()));
            }
        }
        let mut held = self.quotas.cache.lock().await;
        let cache = held.get_or_insert_with(|| cache::QuotaCache::load(&self.quota_path()));
        for ((id, value, signature), at) in readings {
            if value["motivo"] == "credencial-alterada" {
                cache.remove(&id);
            }
            cache.update(&id, value, at);
            if let Some(signature) = signature {
                cache.set_credential(&id, signature);
            }
        }
        if !cached_only && cache.dirty() && cache.save(&self.quota_path()).is_err() {
            tracing::warn!(
                code = "quota_cache_write_failed",
                "cache de cotas não gravado"
            );
        }
        let mut output = vec![];
        for (source, account) in sources {
            let id = source["id"].as_str().ok_or_else(AccountError::io)?;
            let Some(mut value) = cache.get(id) else {
                continue;
            };
            for field in ["id", "label", "provedor", "ativa"] {
                value[field] = source[field].clone();
            }
            if let Some(alias) = facts["aliases"][id].as_str().filter(|s| !s.is_empty()) {
                value["label"] = json!(alias);
            }
            value["idade_s"] = value["ts"]
                .as_f64()
                .map(|at| json!(now() - at))
                .unwrap_or(Value::Null);
            value["refresh_expires_at"] = if let Some((Provider::Claude, account)) = account {
                std::fs::read(account.home.join(".credentials.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                    .and_then(|v| v["claudeAiOauth"]["refreshTokenExpiresAt"].as_f64())
                    .map(|ms| json!(ms / 1000.0))
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            output.push(value);
        }
        Ok(json!(output))
    }

    async fn read_quota(
        &self,
        provider: Provider,
        account: &Account,
        client: &reqwest::Client,
        bridge: &AccountsBridge,
    ) -> (Value, Option<Value>) {
        let Ok(key) = AccountKey::new(provider, &account.home) else {
            return (
                reading("indisponivel", vec![], Some("credencial-ilegivel")),
                None,
            );
        };
        let Ok(mut guard) = self.locks.try_acquire(&key, GuardMode::Shared) else {
            return (reading("indisponivel", vec![], Some("sessao-viva")), None);
        };
        let valid = match provider {
            Provider::Claude => self.validate_claude(account, &key).is_ok(),
            Provider::Codex => {
                self.resolve(provider, &account.id)
                    .ok()
                    .and_then(|row| AccountKey::new(provider, &row.home).ok())
                    .as_ref()
                    == Some(&key)
            }
        };
        if !valid {
            return (
                reading("indisponivel", vec![], Some("credencial-ilegivel")),
                None,
            );
        }
        let signature = || {
            let filename = if provider == Provider::Claude {
                ".credentials.json"
            } else {
                "auth.json"
            };
            std::fs::read(account.home.join(filename))
                .ok()
                .map(|bytes| json!(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref()))
                .unwrap_or(Value::Null)
        };
        let mut before = signature();
        let value = match provider {
            Provider::Claude => {
                let value = claude::read(client, &account.home, now()).await;
                if value["estado"] == "expirada" {
                    drop(guard);
                    if let Err(reason) = self
                        .refresh_claude(
                            account,
                            bridge.clone(),
                            self.quotas.runtime.get().cloned(),
                            true,
                        )
                        .await
                    {
                        return (reading("expirada", vec![], Some(reason)), None);
                    }
                    guard = match self.locks.try_acquire(&key, GuardMode::Shared) {
                        Ok(guard) => guard,
                        Err(_) => {
                            return (reading("indisponivel", vec![], Some("sessao-viva")), None);
                        }
                    };
                    if self.validate_claude(account, &key).is_err() {
                        return (
                            reading("indisponivel", vec![], Some("credencial-ilegivel")),
                            None,
                        );
                    }
                    before = signature();
                    claude::read(client, &account.home, now()).await
                } else {
                    value
                }
            }
            Provider::Codex => codex::read(self, client, account, now()).await,
        };
        let after = signature();
        drop(guard);
        if before != after {
            return (
                reading("indisponivel", vec![], Some("credencial-alterada")),
                None,
            );
        }
        (value, Some(after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn concurrent_requests_share_one_writer_and_keep_other_provider() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = reads.clone();
        let handler = move |body: bytes::Bytes| {
            let counter = counter.clone();
            async move {
                let body: Value = serde_json::from_slice(&body).unwrap();
                if body["action"] == "sources" {
                    Json(
                        json!({"sources":[{"id":"kimi:test","label":"Kimi","provedor":"kimi","ativa":false}],"aliases":{"kimi:test":"Chave"}}),
                    )
                } else {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(
                        json!({"readings":{"kimi:test":reading("lida",vec![json!({"rotulo":"5h","pct":25,"reset_ts":null,"por_modelo":false})],None)}}),
                    )
                }
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app =
            axum::Router::new().route("/internal/accounts/quotas", axum::routing::post(handler));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let mut env =
            crate::accounts::environment::AccountEnvironment::from_map(Default::default());
        env.home = temp.path().to_owned();
        env.claude_base = temp.path().join(".claude");
        env.codex_default = temp.path().join(".codex");
        let service = AccountService::new(env.clone());
        let bridge = AccountsBridge::new(address, "synthetic".into(), "synthetic".into()).unwrap();
        let (first, second) = tokio::join!(
            service.quotas(bridge.clone(), false, false),
            service.quotas(bridge.clone(), false, false)
        );
        let first = first.unwrap();
        let second = second.unwrap();
        for rows in [&first, &second] {
            let row = rows
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["id"] == "kimi:test")
                .unwrap();
            assert_eq!(row["label"], "Chave");
            assert_eq!(row["janelas"][0]["pct"], 25);
            assert!(row["idade_s"].as_f64().is_some_and(|age| age >= 0.0));
        }
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        let bytes = std::fs::read(service.quota_path()).unwrap();
        let stored: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stored["kimi:test"]["cota"]["id"], "kimi:test");
        let restarted = AccountService::new(env);
        let rows = restarted.quotas(bridge, false, false).await.unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .find(|v| v["id"] == "kimi:test")
                .unwrap()["label"],
            "Chave"
        );
        server.abort();
    }
    #[tokio::test]
    async fn cached_read_does_not_wait_for_refresh_in_flight() {
        let (entered, mut in_flight) = tokio::sync::mpsc::unbounded_channel::<()>();
        let release = Arc::new(tokio::sync::Notify::new());
        let gate = release.clone();
        let handler = move |body: bytes::Bytes| {
            let (entered, gate) = (entered.clone(), gate.clone());
            async move {
                let body: Value = serde_json::from_slice(&body).unwrap();
                if body["action"] == "sources" {
                    return Json(json!({"sources":[{"id":"kimi:test","label":"Kimi",
                        "provedor":"kimi","ativa":false}],"aliases":{}}));
                }
                entered.send(()).unwrap();
                gate.notified().await;
                Json(json!({"readings":{"kimi:test":reading("lida",vec![],None)}}))
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app =
            axum::Router::new().route("/internal/accounts/quotas", axum::routing::post(handler));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let temp = tempfile::tempdir().unwrap();
        let mut env =
            crate::accounts::environment::AccountEnvironment::from_map(Default::default());
        env.home = temp.path().to_owned();
        env.claude_base = temp.path().join(".claude");
        env.codex_default = temp.path().join(".codex");
        let service = AccountService::new(env);
        let bridge = AccountsBridge::new(address, "synthetic".into(), "synthetic".into()).unwrap();
        let refreshing = tokio::spawn({
            let (service, bridge) = (service.clone(), bridge.clone());
            async move { service.quotas(bridge, false, false).await }
        });
        in_flight.recv().await.unwrap();
        // Quem escolhe conta ao abrir sessão lê só o cache; a rede das outras fontes não é dele.
        let cached = tokio::time::timeout(
            Duration::from_secs(2),
            service.quotas(bridge.clone(), false, true),
        )
        .await
        .expect("a leitura do cache esperou o refresh em voo");
        let kimi = |rows: &Value| {
            rows.as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == "kimi:test")
        };
        assert!(!kimi(&cached.unwrap()), "o cache ainda não tinha a leitura em voo");
        release.notify_one();
        assert!(kimi(&refreshing.await.unwrap().unwrap()));
        server.abort();
    }
    #[test]
    fn suggestion_uses_tightest_window_and_prefers_active_on_tie() {
        let rows = vec![
            json!({"id":"claude:first","label":"first","provedor":"claude","estado":"lida","ativa":false,"janelas":[{"pct":10},{"pct":90}]}),
            json!({"id":"claude:second","label":"second","provedor":"claude","estado":"lida","ativa":true,"janelas":[{"pct":90}]}),
            json!({"id":"claude:unavailable","provedor":"claude","estado":"expirada","janelas":[{"pct":0}]}),
        ];
        let result = suggestion(&rows).unwrap();
        assert_eq!(result["id"], "claude:second");
        assert_eq!(result["folga"], 10.0);
    }
}
