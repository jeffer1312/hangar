pub mod cache;
pub mod claude;
pub mod codex;
pub mod other;
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

/// O cliente desiste em 8 s; aos 6 s a resposta sai do cache, e a leitura de rede segue.
pub const CLIENT_DEADLINE: Duration = Duration::from_secs(6);

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
    let (result, late) = state.accounts.quotas_within(Some(bridge), force, CLIENT_DEADLINE, None).await;
    if late {
        state.diag.report("rust.quota_sources_partial", "cotas", "quota_deadline",
            "cotas passaram do prazo; a resposta sai com o cache e a leitura segue");
    }
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
            .quotas(Some(bridge), body.force, body.cached_only)
            .await,
    )
}

#[derive(Clone, Default)]
pub struct Quotas {
    pub cache: Arc<Mutex<Option<cache::QuotaCache>>>,
    /// Um refresh de rede por vez; quem só lê o cache não entra nesta fila.
    pub refresh: Arc<Mutex<()>>,
    pub runtime: Arc<std::sync::OnceLock<Arc<crate::runtime::gateway::RuntimeRegistry>>>,
    pub diag: Arc<std::sync::OnceLock<crate::diag::DiagClient>>,
    pub endpoints: other::Endpoints,
    /// Problemas de config já levados ao diário: a lista é lida a cada poll e só a mudança conta.
    pub reported: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

/// De onde sai a leitura de uma fonte.
#[derive(Clone)]
enum Origin {
    Account(Provider, Account),
    Other(other::Reader),
}

/// A leitura curta de cota da abertura sai pela rede; os testes de integração a desligam, porque as
/// contas deles têm token falso e ela iria à Anthropic. Do processo inteiro, como o `use_cano_binary`.
static OPEN_READ_OFFLINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[doc(hidden)]
pub fn disable_open_network_read() { OPEN_READ_OFFLINE.store(true, std::sync::atomic::Ordering::Relaxed) }

pub fn open_network_read() -> bool { !OPEN_READ_OFFLINE.load(std::sync::atomic::Ordering::Relaxed) }

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

    /// A ponte só serve à renovação do token Claude: sem ela as outras fontes são lidas igual.
    pub async fn quotas(
        &self,
        bridge: Option<AccountsBridge>,
        force: bool,
        cached_only: bool,
    ) -> Result<Value, AccountError> {
        self.quotas_with(bridge, force, cached_only, None).await
    }

    /// `config` já lido por quem chama (a lista de credenciais lê os mesmos arquivos).
    pub async fn quotas_with(
        &self,
        bridge: Option<AccountsBridge>,
        force: bool,
        cached_only: bool,
        config: Option<Arc<other::Config>>,
    ) -> Result<Value, AccountError> {
        let service = self.clone();
        // O worker conserva as guardas e termina a publicação se o cliente desconectar.
        tokio::spawn(async move { service.quotas_owned(bridge, force, cached_only, config).await })
            .await
            .map_err(|_| AccountError::io())?
    }

    /// Cota com prazo: estourado, devolve o que o cache já tem (`true` no segundo campo) e a leitura
    /// de rede segue no worker, gravando o cache para a próxima. Leitura de outro pedido já em voo
    /// também responde do cache, mas não é atraso (`false`).
    pub async fn quotas_within(
        &self,
        bridge: Option<AccountsBridge>,
        force: bool,
        deadline: Duration,
        config: Option<Arc<other::Config>>,
    ) -> (Result<Value, AccountError>, bool) {
        // ponytail: leitura de rede já em voo e o pedido não é forçado → responde do cache em vez de
        // enfileirar outra (corrida curta entre o try_lock e o lock do worker: no pior caso uma leitura a mais).
        if !force && self.quotas.refresh.try_lock().is_err() {
            return (self.quotas_with(bridge, false, true, config).await, false);
        }
        match tokio::time::timeout(deadline, self.quotas_with(bridge.clone(), force, false, config.clone())).await {
            Ok(result) => (result, false),
            Err(_) => (self.quotas_with(bridge, false, true, config).await, true),
        }
    }

    /// Conta ocupada (login, renovação, redefinição) é `sessao-viva`, que as telas calam; trava que
    /// não abre é leitura que falhou e vai ao diário.
    fn lock_refused(&self, error: crate::accounts::locks::LockError) -> Value {
        match error {
            crate::accounts::locks::LockError::Busy => reading("indisponivel", vec![], Some("sessao-viva")),
            crate::accounts::locks::LockError::Io(error) => {
                tracing::warn!(code = "quota_account_lock_failed", io_kind = ?error.kind(), "trava da conta não abriu; cota sem leitura");
                if let Some(diag) = self.quotas.diag.get() {
                    diag.report("rust.quota_sources_partial", "cotas", "quota_account_lock_failed",
                        "trava da conta não abriu; cota sem leitura");
                }
                reading("indisponivel", vec![], Some("erro-trava"))
            }
        }
    }

    /// Diário e log só quando o conjunto de problemas muda: a lista é relida a cada poll.
    fn report_config_problems(&self, problems: &[&'static str]) {
        let mut sorted = problems.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        {
            let mut last = self.quotas.reported.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if *last == sorted {
                return;
            }
            *last = sorted.clone();
        }
        for code in sorted {
            tracing::warn!(code, "configuração de fonte de cota fora do formato; as outras seguem");
            if let Some(diag) = self.quotas.diag.get() {
                diag.report("rust.quota_sources_partial", "cotas", code, "configuração de fonte de cota fora do formato; as outras seguem");
            }
        }
    }

    async fn quotas_owned(
        &self,
        bridge: Option<AccountsBridge>,
        force: bool,
        cached_only: bool,
        config: Option<Arc<other::Config>>,
    ) -> Result<Value, AccountError> {
        let _refresh = if cached_only {
            None
        } else {
            Some(self.quotas.refresh.lock().await)
        };
        let config = match config {
            Some(config) => config,
            None => {
                let env = self.env.clone();
                Arc::new(tokio::task::spawn_blocking(move || other::load(&env)).await.map_err(|_| AccountError::io())?)
            }
        };
        self.report_config_problems(&config.problems);
        let aliases = &config.aliases;
        let mut sources = vec![];
        // Catálogo que falha tira só a origem dele: as outras fontes seguem com cota.
        let catalog_failed = |code: &'static str, origin: &str| {
            tracing::warn!(code, origin, "catálogo de contas ilegível; a cota das outras fontes segue");
            if let Some(diag) = self.quotas.diag.get() {
                diag.report("rust.quota_sources_partial", &format!("cotas:{origin}"), code,
                    "catálogo de contas ilegível; a cota das outras fontes segue");
            }
        };
        let claude = self.claude_accounts().unwrap_or_else(|error| {
            catalog_failed(error.code, "claude");
            vec![]
        });
        let codex = self.visible_codex_accounts().unwrap_or_else(|error| {
            catalog_failed(error.code, "codex");
            vec![]
        });
        for (row, account) in claude {
            let path = row["path"].as_str().ok_or_else(AccountError::io)?;
            if !account.is_default
                && !crate::accounts::storage::real_file(
                    &account.home.join(crate::accounts::storage::CLAUDE_MARKER),
                )
            {
                continue;
            }
            sources.push((json!({"id":format!("claude:{path}"),"label":row["label"],"provedor":"claude","ativa":row["active"]}),Origin::Account(Provider::Claude, account)));
        }
        for account in codex {
            let path = crate::accounts::catalog::resolved(&account.home);
            sources.push((json!({"id":format!("codex:{}",path.to_string_lossy()),
                "label":if account.is_default {"Codex"} else {&account.id},"provedor":"codex","ativa":false}),Origin::Account(Provider::Codex,account)));
        }
        let mut endpoints = vec![];
        for source in &config.sources {
            endpoints.push((source.id.clone(), source.reader.fingerprint(&self.quotas.endpoints)));
            sources.push((source.row(), Origin::Other(source.reader.clone())));
        }
        let signatures: std::collections::HashMap<String, Value> = sources
            .iter()
            .filter_map(|(row, origin)| {
                let Origin::Account(provider, account) = origin else { return None };
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
            for (id, endpoint) in &endpoints {
                cache.set_endpoint(id, endpoint);
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
        // Clientes só quando há rede: a leitura só do cache roda a cada poll e montar um relê os
        // certificados; montado a cada leitura, ele vê o proxy atual do sistema (registro no Windows).
        if !selected.is_empty() {
            let client = external_client(Duration::from_secs(8))?;
            let other_client = other::client()?;
            let mut jobs = futures_util::stream::iter(selected)
                .map(|(source, origin)| {
                    let (client, other_client) = (client.clone(), other_client.clone());
                    let bridge = bridge.clone();
                    let service = self.clone();
                    async move {
                        let id = source["id"].as_str().unwrap_or("");
                        let (mut value, signature, contact) = match origin {
                            Origin::Account(provider, account) => {
                                let (value, signature) = service
                                    .read_quota(provider, &account, &client, bridge.as_ref())
                                    .await;
                                (value, signature, None)
                            }
                            Origin::Other(reader) => {
                                let outcome = other::read(
                                    &other_client,
                                    &reader,
                                    &service.quotas.endpoints,
                                    now(),
                                )
                                .await;
                                (outcome.value, None, Some((outcome.contact, outcome.code, reader.kind())))
                            }
                        };
                        for field in ["id", "label", "provedor", "ativa"] {
                            value[field] = source[field].clone();
                        }
                        (id.to_owned(), value, signature, contact)
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
        for ((id, value, signature, contact), at) in readings {
            if value["motivo"] == "credencial-alterada" {
                cache.remove(&id);
            }
            cache.update(&id, value, at);
            if let Some(signature) = signature {
                cache.set_credential(&id, signature);
            }
            if let Some((_, endpoint)) = endpoints.iter().find(|(other, _)| *other == id) {
                cache.set_endpoint(&id, endpoint);
            }
            // Tipo de leitor e degrau da espera vão no campo de sessão: id e nome do motor ficam fora.
            let Some((contact, code, kind)) = contact else { continue };
            match contact {
                other::Contact::Answered => cache.clear_unreachable(&id),
                other::Contact::Refused => {
                    let attempt = cache.mark_unreachable(&id, at, force);
                    tracing::warn!(code = "quota_source_unreachable", kind, attempt, "fonte de cota recusou a conexão");
                    if let Some(diag) = self.quotas.diag.get() {
                        diag.report("rust.quota_source_unreachable", &format!("cotas:{kind}:tentativa-{attempt}"),
                            "quota_source_unreachable", "fonte de cota recusou a conexão; nova tentativa com espera crescente");
                    }
                    continue;
                }
                other::Contact::Silent => {}
            }
            if let Some(code) = code {
                tracing::warn!(code, kind, "leitura de cota sem número; a fonte segue na lista");
                if let Some(diag) = self.quotas.diag.get() {
                    diag.report("rust.quota_source_problem", &format!("cotas:{kind}"), code,
                        "leitura de cota sem número; a fonte segue na lista");
                }
            }
        }
        if !cached_only && cache.dirty() && cache.save(&self.quota_path()).is_err() {
            tracing::warn!(
                code = "quota_cache_write_failed",
                "cache de cotas não gravado"
            );
        }
        let mut output = vec![];
        for (source, origin) in sources {
            let id = source["id"].as_str().ok_or_else(AccountError::io)?;
            let Some(mut value) = cache.get(id) else {
                continue;
            };
            for field in ["id", "label", "provedor", "ativa"] {
                value[field] = source[field].clone();
            }
            if let Some(alias) = aliases.get(id).and_then(Value::as_str) {
                value["label"] = json!(alias);
            }
            value["idade_s"] = value["ts"]
                .as_f64()
                .map(|at| json!(now() - at))
                .unwrap_or(Value::Null);
            value["refresh_expires_at"] = if let Origin::Account(Provider::Claude, account) = origin {
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

    /// Janelas da conta Claude de `config_dir` (vazio = a padrão) para a linha de status, sem as por
    /// modelo. Só o cache em disco e a API da Anthropic: nenhuma chamada ao Python. Token vencido sai
    /// sem janelas até o ciclo de renovação de fundo trocá-lo.
    pub async fn claude_windows(&self, config_dir: &str) -> Result<Vec<Value>, &'static str> {
        let wanted = if config_dir.is_empty() {
            None
        } else {
            Some(AccountKey::new(Provider::Claude, std::path::Path::new(config_dir)).map_err(|_| "quota_account")?)
        };
        let (path, account) = self
            .claude_accounts()
            .map_err(|_| "quota_catalog")?
            .into_iter()
            .filter(|(_, account)| {
                account.is_default
                    || crate::accounts::storage::real_file(
                        &account.home.join(crate::accounts::storage::CLAUDE_MARKER),
                    )
            })
            .find(|(_, account)| match &wanted {
                None => account.is_default,
                Some(key) => AccountKey::new(Provider::Claude, &account.home).ok().as_ref() == Some(key),
            })
            .and_then(|(row, account)| Some((row["path"].as_str()?.to_owned(), account)))
            .ok_or("quota_account")?;
        let id = format!("claude:{path}");
        let signature = std::fs::read(account.home.join(".credentials.json"))
            .ok()
            .map(|bytes| json!(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref()))
            .unwrap_or(Value::Null);
        let stale = {
            let mut held = self.quotas.cache.lock().await;
            let cache = held.get_or_insert_with(|| cache::QuotaCache::load(&self.quota_path()));
            if cache.credential_changed(&id, &signature) {
                cache.remove(&id);
            }
            cache.set_credential(&id, signature);
            cache.needs_refresh(&id, now(), false)
        };
        // Com a listagem já lendo a rede, ela grava o cache: a linha usa o que já houver.
        if stale && let Ok(_refresh) = self.quotas.refresh.try_lock() {
            let client = external_client(Duration::from_secs(8)).map_err(|_| "quota_client")?;
            let (value, signature) = self.read_quota(Provider::Claude, &account, &client, None).await;
            let mut held = self.quotas.cache.lock().await;
            let cache = held.get_or_insert_with(|| cache::QuotaCache::load(&self.quota_path()));
            if value["motivo"] == "credencial-alterada" {
                cache.remove(&id);
            }
            cache.update(&id, value, now());
            if let Some(signature) = signature {
                cache.set_credential(&id, signature);
            }
            if cache.save(&self.quota_path()).is_err() {
                tracing::warn!(code = "quota_cache_write_failed", "cache de cotas não gravado");
            }
        }
        let held = self.quotas.cache.lock().await;
        let quota = held.as_ref().and_then(|cache| cache.get(&id)).unwrap_or(Value::Null);
        // Leitura que falhou vira código no log do ator, não linha calada sem janelas.
        match quota["estado"].as_str() {
            None | Some("lida") => {}
            Some("expirada") => return Err("quota_expired"),
            Some("sem_credencial") => return Err("quota_no_credential"),
            Some(_) => return Err("quota_unavailable"),
        }
        Ok(quota["janelas"]
            .as_array()
            .map(|windows| windows.iter().filter(|w| w["por_modelo"] != true).cloned().collect())
            .unwrap_or_default())
    }

    async fn read_quota(
        &self,
        provider: Provider,
        account: &Account,
        client: &reqwest::Client,
        bridge: Option<&AccountsBridge>,
    ) -> (Value, Option<Value>) {
        let Ok(key) = AccountKey::new(provider, &account.home) else {
            return (
                reading("indisponivel", vec![], Some("credencial-ilegivel")),
                None,
            );
        };
        let mut guard = match self.locks.try_acquire(&key, GuardMode::Shared) {
            Ok(guard) => guard,
            Err(error) => return (self.lock_refused(error), None),
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
                // Só quem lê com a ponte renova a conta expirada.
                if value["estado"] == "expirada" && bridge.is_some() {
                    drop(guard);
                    if let Err(reason) = self
                        .refresh_claude(
                            account,
                            self.claude_windows.clone(),
                            self.quotas.runtime.get().cloned(),
                            true,
                        )
                        .await
                    {
                        return (reading("expirada", vec![], Some(reason)), None);
                    }
                    guard = match self.locks.try_acquire(&key, GuardMode::Shared) {
                        Ok(guard) => guard,
                        Err(error) => return (self.lock_refused(error), None),
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

/// Cliente para os serviços de fora (cotas, OAuth): respeita o proxy do ambiente, como o
/// `urllib` do Python fazia; quem só sai por proxy não pode perder cota nem login.
pub(crate) fn external_client(timeout: Duration) -> Result<reqwest::Client, AccountError> {
    crate::http_client()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|error| {
            tracing::warn!(code = "quota_client_build_failed", %error, "cliente HTTP das cotas não montou");
            AccountError::io()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn external_reads_go_through_the_environment_proxy() {
        if std::env::var_os("QUOTA_PROXY_PROBE").is_none() {
            // Processo filho: o proxy vem só do ambiente dele, sem tocar no deste teste.
            let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = proxy.local_addr().unwrap();
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "accounts::quotas::tests::external_reads_go_through_the_environment_proxy"])
                .env("QUOTA_PROXY_PROBE", "1")
                .env("HTTPS_PROXY", format!("http://{address}"))
                .env_remove("NO_PROXY")
                .env_remove("no_proxy")
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            proxy.set_nonblocking(true).unwrap();
            let mut child = child;
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let stream = loop {
                match proxy.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if child.try_wait().unwrap().is_some() || std::time::Instant::now() > deadline {
                            break None;
                        }
                        std::thread::yield_now();
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            let mut stream = stream.expect("a leitura externa não passou pelo proxy do ambiente");
            stream.set_nonblocking(false).unwrap();
            let mut first = [0u8; 64];
            let read = std::io::Read::read(&mut stream, &mut first).unwrap();
            drop(stream);
            let _ = child.kill();
            let _ = child.wait();
            assert!(
                String::from_utf8_lossy(&first[..read]).starts_with("CONNECT quota.invalid:443"),
                "{}",
                String::from_utf8_lossy(&first[..read])
            );
            return;
        }
        let client = external_client(Duration::from_secs(3)).unwrap();
        let _ = client.get("https://quota.invalid/").send().await;
    }
    /// Conta temporária com um motor de chave apontando para `base`, apelido incluído.
    fn service_with_engine(base: &str) -> (tempfile::TempDir, AccountService) {
        let temp = tempfile::tempdir().unwrap();
        let claude = temp.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("engines.json"),
            json!({"test":{"base_url":base,"api_key":"k","model":"m"}}).to_string()).unwrap();
        std::fs::write(claude.join(".hangar-apelidos.json"), json!({"chave:test":"Chave"}).to_string()).unwrap();
        let mut env =
            crate::accounts::environment::AccountEnvironment::from_map(Default::default());
        env.home = temp.path().to_owned();
        env.claude_base = claude;
        env.codex_default = temp.path().join(".codex");
        (temp, AccountService::new(env))
    }

    fn bridge() -> AccountsBridge {
        // Nenhuma volta ao Python nesta leitura: a ponte aponta para porta nenhuma.
        AccountsBridge::new("127.0.0.1:9".parse().unwrap(), "synthetic".into(), "synthetic".into()).unwrap()
    }

    fn row<'a>(rows: &'a Value, id: &str) -> Option<&'a Value> {
        rows.as_array().unwrap().iter().find(|v| v["id"] == id)
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_writer_and_keep_other_provider() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = reads.clone();
        let handler = move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Json(json!({"limits":[{"window":{"duration":300},"detail":{"limit":"100","remaining":"75"}}]}))
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/v1/usages", axum::routing::get(handler));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (_temp, service) = service_with_engine(&format!("http://{address}/v1"));
        let (first, second) = tokio::join!(
            service.quotas(Some(bridge()), false, false),
            service.quotas(Some(bridge()), false, false)
        );
        for rows in [&first.unwrap(), &second.unwrap()] {
            let row = row(rows, "chave:test").unwrap();
            assert_eq!(row["label"], "Chave");
            assert_eq!(row["provedor"], "kimi");
            assert_eq!(row["janelas"][0]["pct"], 25.0);
            assert!(row["idade_s"].as_f64().is_some_and(|age| age >= 0.0));
        }
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        let stored: Value = serde_json::from_slice(&std::fs::read(service.quota_path()).unwrap()).unwrap();
        assert_eq!(stored["chave:test"]["cota"]["id"], "chave:test");
        let restarted = AccountService::new(service.env.clone());
        let rows = restarted.quotas(Some(bridge()), false, false).await.unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(row(&rows, "chave:test").unwrap()["label"], "Chave");
        server.abort();
    }

    /// Catálogo de contas ilegível derruba só aquela origem: as chaves de API seguem com cota.
    #[tokio::test]
    async fn unreadable_account_catalog_keeps_the_other_sources() {
        let handler = || async { Json(json!({"usage":{"limit":"10","remaining":"5"}})) };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/v1/usages", axum::routing::get(handler));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (temp, service) = service_with_engine(&format!("http://{address}/v1"));
        let mut env = service.env.clone();
        // HOME que é arquivo: listar as pastas das contas falha com erro de E/S.
        env.home = temp.path().join("home-arquivo");
        std::fs::write(&env.home, "x").unwrap();
        env.codex_default = env.home.join(".codex");
        env.base.insert("CP_ENGINES_FILE".into(), temp.path().join(".claude/engines.json").to_string_lossy().into_owned());
        let service = AccountService::new(env);
        assert!(service.claude_accounts().is_err() || service.visible_codex_accounts().is_err(), "o caso precisa de um catálogo que falha");
        let rows = service.quotas(None, false, false).await.expect("a cota das outras fontes sai");
        assert_eq!(row(&rows, "chave:test").unwrap()["estado"], "lida");
        server.abort();
    }

    /// Pasta das travas que não abre não é conta ocupada: `sessao-viva` as telas calam de propósito.
    #[tokio::test]
    async fn unreadable_lock_folder_is_a_failed_read_not_a_busy_account() {
        let (temp, service) = service_with_engine("http://127.0.0.1:9/v1");
        let locks = temp.path().join(".hangar/account-locks");
        std::fs::create_dir_all(locks.parent().unwrap()).unwrap();
        std::fs::write(&locks, "não é pasta").unwrap();
        let rows = service.quotas(None, false, false).await.unwrap();
        let claude = rows.as_array().unwrap().iter().find(|row| row["provedor"] == "claude").expect("conta padrão");
        assert_eq!(claude["estado"], "indisponivel");
        assert_ne!(claude["motivo"], "sessao-viva", "{claude}");
    }

    /// Leitura em voo não é prazo estourado; a forçada espera a vez e só ela pode estourar.
    #[tokio::test]
    async fn in_flight_read_is_not_late_and_a_forced_one_still_waits() {
        let (_temp, service) = service_with_engine("http://127.0.0.1:9/v1");
        let config = Arc::new(other::Config::default());
        let held = service.quotas.refresh.lock().await;
        let started = std::time::Instant::now();
        let (rows, late) = service.quotas_within(None, false, Duration::from_secs(5), Some(config.clone())).await;
        assert!(rows.is_ok());
        assert!(!late, "em voo não conta como prazo");
        assert!(started.elapsed() < Duration::from_secs(1));
        let (_, late) = service.quotas_within(None, true, Duration::from_millis(100), Some(config)).await;
        assert!(late, "forçada espera o lock e estoura o prazo");
        drop(held);
    }

    #[tokio::test]
    async fn cached_read_does_not_wait_for_refresh_in_flight() {
        let (entered, mut in_flight) = tokio::sync::mpsc::unbounded_channel::<()>();
        let release = Arc::new(tokio::sync::Notify::new());
        let gate = release.clone();
        let handler = move || {
            let (entered, gate) = (entered.clone(), gate.clone());
            async move {
                entered.send(()).unwrap();
                gate.notified().await;
                Json(json!({"usage":{"limit":"10","remaining":"5"}}))
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/v1/usages", axum::routing::get(handler));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (_temp, service) = service_with_engine(&format!("http://{address}/v1"));
        let refreshing = tokio::spawn({
            let service = service.clone();
            async move { service.quotas(Some(bridge()), false, false).await }
        });
        in_flight.recv().await.unwrap();
        // Quem escolhe conta ao abrir sessão lê só o cache; a rede das outras fontes não é dele.
        let cached = tokio::time::timeout(Duration::from_secs(2), service.quotas(Some(bridge()), false, true))
            .await
            .expect("a leitura do cache esperou o refresh em voo");
        assert!(row(&cached.unwrap(), "chave:test").is_none(), "o cache ainda não tinha a leitura em voo");
        release.notify_one();
        assert!(row(&refreshing.await.unwrap().unwrap(), "chave:test").is_some());
        server.abort();
    }

    /// Fonte lenta não segura quem tem prazo: sai o cache, e a leitura em voo grava para a próxima.
    #[tokio::test]
    async fn slow_source_past_the_deadline_returns_the_cache_and_keeps_reading() {
        let release = Arc::new(tokio::sync::Notify::new());
        let gate = release.clone();
        let handler = move || {
            let gate = gate.clone();
            async move {
                gate.notified().await;
                Json(json!({"usage":{"limit":"10","remaining":"5"}}))
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/v1/usages", axum::routing::get(handler));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (_temp, service) = service_with_engine(&format!("http://{address}/v1"));
        let config = Arc::new(other::load(&service.env));
        let started = std::time::Instant::now();
        let (rows, late) = service.quotas_within(None, false, Duration::from_millis(300), Some(config)).await;
        assert!(late && started.elapsed() < Duration::from_secs(3));
        assert!(row(&rows.unwrap(), "chave:test").is_none(), "o cache ainda não tinha a leitura");
        release.notify_one();
        let mut stored = Value::Null;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if let Ok(bytes) = std::fs::read(service.quota_path()) {
                stored = serde_json::from_slice(&bytes).unwrap();
                break;
            }
        }
        assert_eq!(stored["chave:test"]["cota"]["estado"], "lida", "a leitura em voo terminou e gravou");
        server.abort();
    }

    #[tokio::test]
    async fn refused_source_waits_before_paying_the_connection_again() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let (_temp, service) = service_with_engine(&format!("http://{closed}"));
        let rows = service.quotas(Some(bridge()), false, false).await.unwrap();
        let first = row(&rows, "chave:test").unwrap();
        assert_eq!((&first["estado"], &first["motivo"]), (&json!("indisponivel"), &json!("sem-resposta")));
        let stored: Value = serde_json::from_slice(&std::fs::read(service.quota_path()).unwrap()).unwrap();
        let entry = &stored["chave:test"];
        assert_eq!(entry["unreachable_count"], 1);
        let wait = entry["unreachable_until"].as_f64().unwrap() - entry["gravado_em"].as_f64().unwrap();
        assert!((wait - 600.0).abs() < 1.0, "{wait}");
        assert!(entry.get("endpoint").is_none() && entry["endpoint_sha"].is_string(), "endereço só como hash");
        // Forçar fura a espera, mas não sobe degrau: a próxima automática segue em 10 min.
        service.quotas(Some(bridge()), true, false).await.unwrap();
        let stored: Value = serde_json::from_slice(&std::fs::read(service.quota_path()).unwrap()).unwrap();
        assert_eq!(stored["chave:test"]["unreachable_count"], 1);
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
