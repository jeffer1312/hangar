//! Os fatos do `resolve_open()` lidos da máquina: os mesmos arquivos que o servidor lê, por isso o
//! comando de terminal resolve igual com o servidor fora do ar. Bloqueante: o servidor chama por
//! `spawn_blocking`; as partes assíncronas (contas, catálogo do Codex, tmux) rodam no `runtime`.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::{Value, json};

use super::resolve::{CodexCatalogError, CodexLogin, CreatorFacts, OpenFacts, ProviderCatalog, ServerDefaults};
use super::{AgentProvider, OpenError, ServiceTier};
use crate::accounts::catalog::AccountError;
use crate::accounts::environment::AccountEnvironment;
use crate::accounts::{AccountService, Provider};
use crate::list::bridge::expand_home;
use crate::list::procs::{ProcessView, SystemProcs};

/// `engines._NOME_OK`: o nome entra num `$SHELL -c`.
static ENGINE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[a-z0-9][a-z0-9_-]{0,31}\z").unwrap());
static GPT_MODEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\Agpt-\d[^/]*\z").unwrap());

pub struct LocalFacts {
    accounts: AccountService,
    runtime: tokio::runtime::Handle,
    models: crate::dictation::catalog::Models,
    slots: tokio::sync::Semaphore,
    mux: crate::list::mux::Mux,
    procs: std::sync::Arc<dyn ProcessView>,
    serverless: bool,
}

impl LocalFacts {
    pub fn new(env: AccountEnvironment, runtime: tokio::runtime::Handle) -> Self {
        Self { accounts: AccountService::new(env), runtime, models: Default::default(), slots: tokio::sync::Semaphore::new(4),
            mux: Default::default(), procs: std::sync::Arc::new(SystemProcs::default()), serverless: false }
    }

    /// O comando do terminal com o servidor fora: ninguém limpa o que a sessão antiga deixou no nome.
    pub fn without_server(mut self) -> Self {
        self.serverless = true;
        self
    }

    /// Só para teste: multiplexador e `/proc` de mentira, para nunca ler o tmux de quem roda o teste.
    #[doc(hidden)]
    pub fn with_terminal(mut self, mux: crate::list::mux::Mux, procs: std::sync::Arc<dyn ProcessView>) -> Self {
        self.mux = mux;
        self.procs = procs;
        self
    }

    fn env(&self) -> &AccountEnvironment { &self.accounts.env }

    fn hangar(&self) -> PathBuf { self.env().home.join(".hangar") }

    fn headless_sidecar(&self, name: &str) -> PathBuf {
        self.hangar().join("claude-headless").join(format!("{}.json", crate::list::discover_other::sanitize_session_name(name)))
    }

    fn codex_sidecar(&self, name: &str) -> PathBuf {
        crate::runtime::codex_sidecar::path(&crate::runtime::codex_sidecar::dir(&self.env().home), name)
    }

    /// Contas Codex visíveis e o último login conhecido de cada uma (`_codex_auth_states`).
    fn codex_states(&self) -> Result<Vec<(crate::accounts::catalog::Account, String)>, OpenError> {
        // Lista que não se lê não é "nenhuma conta": trocaria a conta ou o provedor calado.
        let accounts = self.accounts.visible_codex_accounts()
            .map_err(|error| OpenError::new(error.status, error.code, error.message).with_params(error.params))?;
        Ok(accounts.into_iter().map(|account| {
            let state = self.runtime.block_on(self.accounts.read_codex_auth_fast(&account))["status"].as_str().unwrap_or("unavailable").to_owned();
            (account, state)
        }).collect())
    }

    fn panes(&self) -> Vec<crate::list::mux::Pane> {
        // Multiplexador fora é "sem sessão de terminal", como o `has_session` do Python.
        self.runtime.block_on(self.mux.list_panes()).unwrap_or_default()
    }

    fn engines_file(&self) -> Value {
        let path = self.env().base.get("CP_ENGINES_FILE").filter(|p| !p.is_empty()).map(PathBuf::from)
            .unwrap_or_else(|| self.env().home.join(".claude").join("engines.json"));
        // Ausente ou quebrado lista vazio, como o `engines.listar`: motor nenhum, nunca o de outro.
        read_json(&path).ok().flatten().filter(Value::is_object).unwrap_or(Value::Null)
    }

    /// `_engine_models`: o `/v1/models` do motor; sem resposta é 502 com o motivo, sem a chave.
    fn provider_models(&self, engine: &str) -> Result<Vec<Value>, OpenError> {
        let engines = self.engines_file();
        let (base, key) = (engines[engine]["base_url"].as_str().unwrap_or(""), engines[engine]["api_key"].as_str().unwrap_or(""));
        // O provedor pode repetir a chave no erro; ela não sai daqui.
        let redact = |error: String| if key.is_empty() { error } else { error.replace(key, "***") };
        self.runtime.block_on(engine_models(base, key)).map_err(|e| {
            let message = redact(e);
            OpenError::new(502, "erro_provedor_offline", format!("o provedor do motor '{engine}' nao respondeu: {message}"))
                .with_params(json!({"nome": engine, "erro": message}))
        })
    }

    /// `cliproxy_accounts.models_for`: só os modelos da conta, sem o prefixo dela.
    fn account_models(&self, engine: &str, prefix: &str) -> Result<Vec<Value>, OpenError> {
        let start = format!("{prefix}/");
        Ok(self.provider_models(engine)?.into_iter().filter_map(|mut m| {
            let id = m["id"].as_str()?.strip_prefix(&start)?.to_owned();
            (SAFE_MODEL.is_match(&id) && !id.starts_with("gpt-image-")).then(|| { m["id"] = Value::from(id); m })
        }).collect())
    }

    fn terminal_creator(&self, name: &str) -> Option<CreatorFacts> {
        let panes = self.panes();
        let pane = panes.iter().find(|p| p.session == name && !p.hidden)?;
        let provider = pane.provider.as_deref().and_then(AgentProvider::parse).unwrap_or(AgentProvider::Claude);
        let procs = &*self.procs;
        let pid = pane.pid.filter(|pid| *pid != 0).map(i64::from);
        // `_caller_config_dir`: pane sem pid ou ambiente ilegível não confirmam a conta.
        let config_dir = match pid.map(|pid| procs.env_var(pid, "CLAUDE_CONFIG_DIR")) {
            Some(Ok(value)) => Ok(value.map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty())),
            _ => Err(()),
        };
        let account = config_dir.clone().ok().flatten().map(PathBuf::from).unwrap_or_else(|| self.env().claude_base.clone());
        let transcript = procs.children(std::time::Duration::ZERO).ok().and_then(|children| {
            crate::list::discover::resolve_one(&panes, procs, &children, &account.join("projects"),
                &mut Default::default(), name, &pane.cwd, pid).jsonl
        });
        let cache = Mutex::new(crate::list::capped::Capped::new(1));
        // Sem fala no transcript, o modo de abertura.
        let permission_mode = transcript.and_then(|jsonl| crate::terminal_facts::transcript_mode(Path::new(&jsonl), &cache).ok().flatten()
            .or_else(|| super::record::launch_mode(&self.env().home, Path::new(&jsonl).file_stem()?.to_str()?)));
        Some(CreatorFacts { provider, headless: false, config_dir, permission_mode, codex_account: None })
    }
}

static SAFE_ACCOUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[a-z0-9][a-z0-9_-]{0,31}\z").unwrap());
static SAFE_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[A-Za-z0-9][A-Za-z0-9_.-]{0,63}\z").unwrap());
static SAFE_MODEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[A-Za-z0-9][A-Za-z0-9_.:\[\]-]*\z").unwrap());

fn cliproxy(msg: impl Into<String>) -> OpenError { OpenError::new(400, "erro_cliproxy_conta", msg) }

/// Os motores do `engines.json`, separando o ausente (nenhum motor) do ilegível, que não diz se um motor
/// sumiu: quem reabre recusa em vez de tirar a sessão do motor.
pub(crate) fn engine_names(env: &AccountEnvironment) -> Result<Vec<String>, String> {
    let path = env.base.get("CP_ENGINES_FILE").filter(|p| !p.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".claude").join("engines.json"));
    match read_json(&path)? {
        None => Ok(Vec::new()),
        Some(Value::Object(map)) => Ok(map.keys().filter(|name| ENGINE_NAME.is_match(name)).cloned().collect()),
        Some(_) => Err(format!("{}: não é um objeto", path.display())),
    }
}

fn engines_of(env: &AccountEnvironment) -> Value {
    let path = env.base.get("CP_ENGINES_FILE").filter(|p| !p.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".claude").join("engines.json"));
    read_json(&path).ok().flatten().filter(Value::is_object).unwrap_or(Value::Null)
}

/// `cliproxy_accounts._credentials`: as credenciais OAuth do CLIProxyAPI desta máquina.
fn credentials(home: &Path) -> Result<Vec<(PathBuf, Value)>, OpenError> {
    let unreadable = || cliproxy("CLIProxyAPI: não consegui ler as credenciais locais");
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(home.join(".cli-proxy-api")) {
        Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return Err(unreadable()),
    };
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        if path.symlink_metadata().map_err(|_| unreadable())?.file_type().is_symlink() {
            return Err(cliproxy("CLIProxyAPI: credencial por link não é permitida"));
        }
        let data: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|_| unreadable())?).map_err(|_| unreadable())?;
        if !data.is_object() { return Err(cliproxy("CLIProxyAPI: arquivo de credencial inválido")); }
        if data.get("disabled").is_some_and(|d| !d.is_boolean()) { return Err(cliproxy("CLIProxyAPI: campo disabled inválido")); }
        out.push((path, data));
    }
    Ok(out)
}

/// `cliproxy.account_for_engine` + `cliproxy_accounts.resolve`: a credencial ativa da conta Codex
/// pedida no CLIProxyAPI local, com prefixo exclusivo, e a rota que o `hangar-engine` recebe.
pub fn fixed_account(accounts: &AccountService, engine: &str, account: &str) -> Result<super::claude::EngineRoute, OpenError> {
    let env = &accounts.env;
    let cfg = &engines_of(env)[engine];
    let local = cliproxy_base(&env.home).filter(|local| normalize_base(cfg["base_url"].as_str().unwrap_or("")) == *local)
        .ok_or_else(|| cliproxy("a conta ChatGPT só pode ser fixada no CLIProxyAPI local"))?;
    if !SAFE_ACCOUNT.is_match(account) { return Err(cliproxy("CLIProxyAPI: identificador de conta inválido")); }
    let creds = credentials(&env.home)?;
    let visible = accounts.visible_codex_accounts().map_err(|e| OpenError::new(e.status, e.code, e.message))?;
    let mut entries: Vec<(String, String, Option<String>, usize)> = Vec::new();
    for codex in visible {
        let raw = match std::fs::read(codex.home.join("auth.json")) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(cliproxy(format!("CLIProxyAPI: login Codex ilegível na conta {}", codex.id))),
        };
        let data: Value = serde_json::from_slice(&raw).map_err(|_| cliproxy(format!("CLIProxyAPI: login Codex ilegível na conta {}", codex.id)))?;
        let Some(id) = data["tokens"]["account_id"].as_str().filter(|i| !i.is_empty()) else { continue };
        let matches: Vec<usize> = creds.iter().enumerate()
            .filter(|(_, (_, c))| c["type"] == "codex" && c["account_id"] == id && c["disabled"] != true).map(|(i, _)| i).collect();
        if matches.len() > 1 { return Err(cliproxy(format!("CLIProxyAPI: mais de uma credencial ativa para a conta {}", codex.id))); }
        let Some(&found) = matches.first() else { continue };
        if entries.iter().any(|e| e.3 == found) { return Err(cliproxy("CLIProxyAPI: a mesma credencial corresponde a mais de uma conta Hangar")); }
        let home = codex.home.canonicalize().unwrap_or(codex.home.clone()).to_string_lossy().into_owned();
        entries.push((codex.id, home, text(&creds[found].1["prefix"]), found));
    }
    for (_, _, prefix, at) in &entries {
        let Some(prefix) = prefix else { continue };
        if !SAFE_PREFIX.is_match(prefix) { return Err(cliproxy("CLIProxyAPI: conta sem prefixo válido; prepare as contas primeiro")); }
        if creds.iter().enumerate().any(|(i, (_, c))| i != *at && c["prefix"].as_str() == Some(prefix)) {
            return Err(cliproxy("CLIProxyAPI: prefixo compartilhado entre credenciais; prepare as contas primeiro"));
        }
    }
    let (id, home, prefix, _) = entries.into_iter().find(|e| e.0 == account)
        .ok_or_else(|| cliproxy(format!("CLIProxyAPI: conta {account} desconhecida, desconectada ou desativada")))?;
    let prefix = prefix.ok_or_else(|| cliproxy("CLIProxyAPI: conta sem prefixo; prepare as contas primeiro"))?;
    Ok(super::claude::EngineRoute { account: id, credential_id: format!("codex:{home}"), home, base_url: local, prefix })
}

/// `cliproxy_accounts.prefix_model` sem o prefixo: o modelo como a conta o expõe.
fn base_model(model: &str, prefix: &str) -> Result<String, OpenError> {
    let model = model.strip_prefix(&format!("{prefix}/")).unwrap_or(model);
    if !SAFE_MODEL.is_match(model) { return Err(cliproxy("CLIProxyAPI: modelo inválido ou prefixo de outra conta")); }
    Ok(model.to_owned())
}

/// `_session_provider_catalog` com os logins já lidos: conectado se algum login diz que sim (Claude
/// também com motor configurado); desconectado só se todos dizem que não. Ilegível não conta.
pub(crate) fn provider_catalog(available: Vec<AgentProvider>, claude_logins: &[Option<bool>], engines: bool,
                               codex_states: &[String], remembered: Option<String>) -> ProviderCatalog {
    let (mut connected, mut disconnected) = (Vec::new(), Vec::new());
    if available.contains(&AgentProvider::Claude) {
        if claude_logins.contains(&Some(true)) || engines {
            connected.push(AgentProvider::Claude);
        } else if !claude_logins.is_empty() && claude_logins.iter().all(|login| *login == Some(false)) {
            disconnected.push(AgentProvider::Claude);
        }
    }
    if available.contains(&AgentProvider::Codex) {
        if codex_states.iter().any(|s| s == "connected") {
            connected.push(AgentProvider::Codex);
        } else if !codex_states.is_empty() && codex_states.iter().all(|s| s == "disconnected") {
            disconnected.push(AgentProvider::Codex);
        }
    }
    ProviderCatalog { available, connected, disconnected, remembered }
}

/// `engines.catalog_model`: o `[1m]` do GPT marca a janela de 1M e não existe no catálogo.
fn catalog_model(model: &str) -> &str {
    static ONE_M: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\Agpt-\d[^/]*\[1m\]\z").unwrap());
    let last = model.rsplit('/').next().unwrap_or(model);
    if ONE_M.is_match(last) { model.strip_suffix("[1m]").unwrap_or(model) } else { model }
}

/// Número inteiro do JSON do provedor (`262144` ou `262144.0`).
fn whole(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_f64().filter(|n| n.fract() == 0.0 && *n >= 0.0).map(|n| n as u64))
}

/// `engine_probe.listar_modelos`: `GET {base}/v1/models` com a chave do motor.
async fn engine_models(base: &str, key: &str) -> Result<Vec<Value>, String> {
    if [base, key].iter().any(|v| v.contains(['\n', '\r', '\0'])) {
        return Err("endereço ou chave com quebra de linha".into());
    }
    // A URL pode vir com o `/v1` que o provedor documenta.
    let base = base.trim_end_matches('/');
    let base = base.strip_suffix("/v1").map_or(base, |b| b.trim_end_matches('/'));
    let client = crate::http_client().timeout(std::time::Duration::from_secs(15)).build().map_err(|e| e.to_string())?;
    // User-Agent explícito: o Cloudflare de alguns provedores recusa o padrão.
    let response = client.get(format!("{base}/v1/models")).bearer_auth(key).header("user-agent", "hangar/1.0").send().await
        .map_err(|e| format!("não foi possível falar com o provedor: {e}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("{} {}", status.as_u16(), body.chars().take(200).collect::<String>()));
    }
    let raw: Value = serde_json::from_str(&body).map_err(|_| format!("resposta não-JSON do provedor: {}", body.chars().take(200).collect::<String>()))?;
    let data = raw.get("data").and_then(Value::as_array).ok_or("resposta inesperada do provedor (sem lista 'data')")?;
    Ok(data.iter().filter(|m| m["id"].as_str().is_some_and(|id| !id.is_empty())).cloned().collect())
}

/// `Ok(None)` = arquivo ausente; `Err` = existe e não se lê como JSON.
fn read_json(path: &Path) -> Result<Option<Value>, String> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn text(value: &Value) -> Option<String> { value.as_str().filter(|s| !s.is_empty()).map(str::to_owned) }

/// Booleano do pydantic-settings (`CP_HEADLESS_DEFAULT`, `CP_JEV_PADRAO`).
fn env_bool(env: &BTreeMap<String, String>, name: &str) -> Option<bool> {
    match env.get(name)?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "f" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}


fn codex_error(error: AccountError) -> OpenError {
    OpenError::new(error.status, error.code, "conta Codex inválida").with_params(error.params)
}

/// `cliproxy.local()`: endereço do CLIProxyAPI desta máquina; `None` sem config utilizável.
/// `cliproxy.is_local_engine`: o motor aponta para o CLIProxyAPI desta máquina.
pub(crate) fn local_engine(env: &AccountEnvironment, engine: &str) -> bool {
    let cfg = &engines_of(env)[engine];
    cliproxy_base(&env.home).is_some_and(|local| normalize_base(cfg["base_url"].as_str().unwrap_or("")) == local)
}

fn cliproxy_base(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join(".cli-proxy-api").join("config.yaml")).ok()?;
    let config: Value = serde_saphyr::from_str(&text).ok()?;
    let keys = config["access"]["api-keys"].as_array().filter(|keys| !keys.is_empty())
        .or_else(|| config["api-keys"].as_array())?;
    keys.iter().filter_map(Value::as_str).any(|k| !k.trim().is_empty()).then_some(())?;
    let mut host = config["host"].as_str().unwrap_or("").trim().to_owned();
    if matches!(host.as_str(), "" | "0.0.0.0" | "::") { host = "127.0.0.1".into(); }
    if host.contains(':') && !host.starts_with('[') { host = format!("[{host}]"); }
    let port = match &config["port"] {
        Value::Null => 8317,
        value => value.as_u64().or_else(|| value.as_str()?.trim().parse().ok())?,
    };
    (1..65536).contains(&port).then_some(())?;
    let scheme = if config["tls"]["enable"] == true { "https" } else { "http" };
    Some(format!("{scheme}://{host}:{port}"))
}

fn normalize_base(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    url.strip_suffix("/v1").map_or(url, |u| u.trim_end_matches('/')).to_owned()
}

/// `cli_probe.sondar_providers`: o CLI que o pane acha pelo PATH do shell de login, com permissão de
/// execução. Resultado por 60 s, por shell e PATH do servidor.
pub(crate) fn installed_providers(env: &BTreeMap<String, String>) -> Vec<AgentProvider> {
    type Cache = Mutex<BTreeMap<(Option<String>, Option<String>), (std::time::Instant, Vec<AgentProvider>)>>;
    static CACHE: LazyLock<Cache> = LazyLock::new(Default::default);
    let key = (env.get("SHELL").cloned(), env.get("PATH").cloned());
    // A trava cobre a sondagem: duas aberturas com o cache vencido não sondam em paralelo.
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, found)) = cache.get(&key) && at.elapsed() < std::time::Duration::from_secs(60) { return found.clone(); }
    let path = login_path(env);
    let found: Vec<AgentProvider> = AgentProvider::ALL.into_iter().filter(|p| runnable_in(p.name(), &path)).collect();
    cache.insert(key, (std::time::Instant::now(), found.clone()));
    found
}

/// `$SHELL -l -c 'printenv PATH'` (5 s); sem shell, falha ou saída vazia, o PATH do servidor. No
/// Windows, sempre o do servidor.
fn login_path(env: &BTreeMap<String, String>) -> String {
    let own = || env.get("PATH").cloned().unwrap_or_default();
    let Some(shell) = env.get("SHELL").filter(|s| !s.is_empty() && cfg!(unix)) else { return own() };
    let mut child = match std::process::Command::new(shell).args(["-l", "-c", "printenv PATH"]).env_clear().envs(env)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn() {
        Ok(child) => child,
        Err(_) => return own(),
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while matches!(child.try_wait(), Ok(None)) && std::time::Instant::now() < deadline { std::thread::sleep(std::time::Duration::from_millis(10)); }
    let Ok(Some(status)) = child.try_wait() else {
        let _ = child.kill();
        let _ = child.wait();
        return own();
    };
    let mut out = String::new();
    let _ = std::io::Read::read_to_string(&mut child.stdout.take().unwrap(), &mut out);
    let out = out.trim();
    if status.success() && !out.is_empty() { out.to_owned() } else { own() }
}

/// Arquivo executável no PATH; no Windows, só com extensão que o pane roda como CLI.
fn runnable_in(name: &str, path: &str) -> bool {
    std::env::split_paths(path).filter(|d| !d.as_os_str().is_empty()).any(|dir| {
        if cfg!(windows) {
            return [".exe", ".cmd", ".bat", ".com"].iter().any(|ext| dir.join(format!("{name}{ext}")).is_file());
        }
        let file = dir.join(name);
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; file.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) }
        #[cfg(not(unix))]
        { file.is_file() }
    })
}

impl OpenFacts for LocalFacts {
    fn providers(&self) -> Result<ProviderCatalog, OpenError> {
        let available = installed_providers(&self.env().base);
        let claude_logins: Vec<Option<bool>> = if available.contains(&AgentProvider::Claude) {
            // Login ilegível só deixa de preferir quem está conectado, como no Python.
            let accounts = self.accounts.claude_accounts().unwrap_or_else(|error| {
                tracing::warn!(code = error.code, "catálogo de provedores: login Claude ilegível");
                Vec::new()
            });
            accounts.iter().map(|(_, account)| self.runtime.block_on(self.accounts.read_claude_auth(account))["loggedIn"].as_bool()).collect()
        } else {
            Vec::new()
        };
        let codex_states: Vec<String> = if available.contains(&AgentProvider::Codex) {
            self.codex_states()?.into_iter().map(|(_, state)| state).collect()
        } else {
            Vec::new()
        };
        let runtime = crate::runtime::codex_launch::runtime_config(self.env());
        Ok(provider_catalog(available, &claude_logins, !self.engines().is_empty(), &codex_states, text(&runtime["last_session_provider"])))
    }

    fn creator(&self, name: &str) -> Result<Option<CreatorFacts>, OpenError> {
        // Registro existente e ilegível não é "criadora não encontrada": herdar nada calado erraria conta e modo.
        let read = |path: &Path| crate::runtime::codex_sidecar::read(path).map_err(|_| OpenError::new(409, "erro_criadora_ilegivel",
            format!("não consegui ler o registro da sessão criadora '{name}'; escolha a conta e o modo")));
        if let Some(meta) = read(&self.headless_sidecar(name))?.map(Value::Object) {
            let mut mode = text(&meta["permission_mode"]);
            if mode.as_deref() == Some("plan") { mode = text(&meta["previous_non_plan"]); }
            // Sem modo no sidecar vale o último que o CLI gravou no transcript da conversa.
            let config_dir = text(&meta["config_dir"]);
            let mode = mode.or_else(|| {
                let account = config_dir.as_deref().map_or_else(|| self.env().claude_base.clone(), |dir| expand_home(dir, &self.env().home));
                let jsonl = account.join("projects").join(hangar_workspace::worktrees::sanitize_cwd(&text(&meta["cwd"])?))
                    .join(format!("{}.jsonl", text(&meta["session_id"])?));
                crate::terminal_facts::transcript_mode(&jsonl, &Mutex::new(crate::list::capped::Capped::new(1))).ok().flatten()
            });
            return Ok(Some(CreatorFacts { provider: AgentProvider::Claude, headless: true, config_dir: Ok(config_dir),
                permission_mode: mode, codex_account: None }));
        }
        if let Some(meta) = read(&self.codex_sidecar(name))? {
            return Ok(Some(CreatorFacts { provider: AgentProvider::Codex, headless: meta.get("headless") == Some(&Value::Bool(true)),
                config_dir: Ok(None), permission_mode: meta.get("permission_mode").and_then(text),
                codex_account: meta.get("codex_account").and_then(text) }));
        }
        Ok(self.terminal_creator(name))
    }

    fn defaults(&self) -> ServerDefaults {
        let runtime = crate::runtime::codex_launch::runtime_config(self.env());
        let field = |name: &str, env: &str, default: bool| Some(&runtime[name]).filter(|v| !v.is_null()).map(crate::list::discover_other::truthy).or_else(|| env_bool(&self.env().base, env)).unwrap_or(default);
        ServerDefaults { headless: field("headless_default", "CP_HEADLESS_DEFAULT", true), jev: field("jev_padrao", "CP_JEV_PADRAO", false) }
    }

    fn saved_defaults(&self, provider: AgentProvider, engine: &str, engine_account: &str) -> Result<Option<crate::session_defaults::Defaults>, String> {
        crate::session_defaults::saved(&self.env().home, provider.name(), engine, engine_account).map_err(|error| error.to_string())
    }

    fn codex_logins(&self) -> Result<Vec<CodexLogin>, OpenError> {
        Ok(self.codex_states()?.into_iter().filter(|(_, state)| state == "connected")
            .map(|(account, _)| CodexLogin { id: account.id, is_default: account.is_default }).collect())
    }

    fn claude_quotas(&self) -> Vec<Value> {
        // Sem leitura, nada muda: a conta fica como estava (como o Python sem o Rust).
        let rows = self.runtime.block_on(self.accounts.quotas(None, false, true)).unwrap_or(Value::Null);
        rows.as_array().into_iter().flatten().filter(|row| row["provedor"] == "claude").cloned().collect()
    }

    fn existing_dir(&self, cwd: &str) -> Option<String> {
        let path = expand_home(cwd, &self.env().home);
        path.is_dir().then(|| path.to_string_lossy().into_owned())
    }

    fn config_dirs(&self) -> Result<Vec<String>, OpenError> {
        let catalog = self.accounts.claude_catalog()
            .map_err(|e| OpenError::new(e.status, e.code, e.message))?;
        Ok(catalog.as_array().into_iter().flatten().filter_map(|row| text(&row["path"])).collect())
    }

    fn engines(&self) -> Vec<String> {
        self.engines_file().as_object().into_iter().flatten().map(|(name, _)| name.clone())
            .filter(|name| ENGINE_NAME.is_match(name)).collect()
    }

    fn supports_fast(&self, engine: Option<&str>, model: Option<&str>) -> bool {
        let engines = self.engines_file();
        let Some(cfg) = engine.filter(|e| self.engines().iter().any(|n| n == e)).map(|e| &engines[e]) else { return false };
        // `model or cfg.model`: texto vazio também cai no modelo do motor.
        let base = model.filter(|m| !m.is_empty()).or_else(|| cfg["model"].as_str()).unwrap_or("").rsplit('/').next().unwrap_or("");
        GPT_MODEL.is_match(base)
            && cliproxy_base(&self.env().home).is_some_and(|local| normalize_base(cfg["base_url"].as_str().unwrap_or("")) == local)
    }

    /// `create_session` com `engine_account`: a credencial da conta, o catálogo dela (o do motor só com
    /// o prefixo dela) e o modelo principal e o dos subagentes disponíveis nele. Devolve o modelo sem o
    /// prefixo da conta.
    fn fixed_engine_model(&self, engine: &str, account: &str, model: Option<&str>) -> Result<String, OpenError> {
        if !self.engines().iter().any(|e| e == engine) { return Err(OpenError::new(400, "erro_motor_invalido", "motor inválido")); }
        let route = fixed_account(&self.accounts, engine, account)?;
        let engines = self.engines_file();
        let cfg = &engines[engine];
        let models = self.account_models(engine, &route.prefix)?;
        let selected = model.filter(|m| !m.is_empty()).or(cfg["model"].as_str()).unwrap_or("");
        let base = base_model(selected, &route.prefix)?;
        let subagent = cfg["subagent_model"].as_str().filter(|m| !m.is_empty()).unwrap_or(selected);
        for (label, chosen) in [("principal", selected), ("dos subagentes", subagent)] {
            let id = base_model(chosen, &route.prefix)?;
            if !models.iter().any(|m| m["id"].as_str() == Some(catalog_model(&id))) {
                return Err(cliproxy(format!("CLIProxyAPI: modelo {label} indisponível nesta conta")));
            }
        }
        Ok(base)
    }

    /// `create_session`: o id de catálogo do modelo (sem o `[1m]` do GPT) no `/v1/models` do motor (o
    /// da conta fixa, quando há); com o `[1m]` a janela é 1M, senão a que o provedor diz.
    fn engine_window(&self, engine: &str, model: &str, account: Option<&str>) -> Result<Option<u64>, OpenError> {
        let models = match account {
            Some(account) => self.account_models(engine, &fixed_account(&self.accounts, engine, account)?.prefix)?,
            None => self.provider_models(engine)?,
        };
        let catalog_id = catalog_model(model);
        Ok(models.iter().find(|m| m["id"].as_str() == Some(catalog_id)).and_then(|m| {
            if catalog_id != model { return Some(1_000_000) }
            // `context_length or context_window`: zero e ausente caem para o próximo.
            [&m["context_length"], &m["context_window"]].into_iter().filter_map(whole).find(|n| *n != 0)
        }))
    }

    fn codex_home(&self, account_id: &str) -> Result<String, OpenError> {
        let account = self.accounts.resolve(Provider::Codex, account_id).map_err(codex_error)?;
        Ok(std::path::absolute(&account.home).unwrap_or(account.home).to_string_lossy().into_owned())
    }

    fn codex_choice(&self, home: Option<&str>, model: Option<&str>, effort: Option<&str>, tier: Option<ServiceTier>)
        -> Result<(), CodexCatalogError> {
        let models = match model {
            None => Vec::new(),
            Some(_) => {
                let home = home.map(PathBuf::from).unwrap_or_else(|| self.env().codex_default.clone());
                match self.runtime.block_on(self.models.get(&self.accounts, &self.slots, "codex", &home, false)) {
                    Ok((models, _)) => models,
                    Err(code) => {
                        let detail = format!("Não foi possível consultar o catálogo da conta selecionada ({code}).");
                        return Err(if code == "dictation_catalog_refused" { CodexCatalogError::Refused(detail) } else { CodexCatalogError::Unavailable(detail) });
                    }
                }
            }
        };
        crate::dictation::catalog::validate(&models, model, effort, tier.map(ServiceTier::name)).map_err(CodexCatalogError::Invalid)
    }

    fn account_mode(&self, config_dir: Option<&str>) -> Result<Option<String>, String> {
        // A conta padrão é a do servidor (`CLAUDE_CONFIG_DIR`), não `~/.claude` fixo.
        let dir = config_dir.map_or_else(|| self.env().claude_base.clone(), |dir| expand_home(dir, &self.env().home));
        Ok(read_json(&dir.join("settings.json"))?.and_then(|settings| text(&settings["permissions"]["defaultMode"])))
    }

    fn name_in_use(&self, name: &str) -> Result<bool, OpenError> {
        if self.headless_sidecar(name).exists() || self.codex_sidecar(name).exists() { return Ok(true); }
        // Sem resposta do multiplexador o nome não está livre: só a ausência conhecida (sem servidor ou
        // sem o programa) é "nenhum pane".
        let panes = match self.runtime.block_on(self.mux.list_panes()) {
            Ok(panes) => panes,
            Err(e) if e.code == "mux_missing" => Vec::new(),
            Err(e) => return Err(super::open::mux_unavailable(e.code)),
        };
        Ok(panes.iter().any(|p| p.session == name))
    }

    fn name_has_leftovers(&self, name: &str) -> Result<bool, OpenError> {
        let Some(path) = self.serverless.then(|| super::local::leftover(&self.env().claude_base, name)).flatten() else { return Ok(false) };
        // Ilegível conta como sobra, e o nome seguinte também não se leria: recusa em vez de procurar sem fim.
        match super::local::probe(&path) {
            Ok(_) => Ok(true),
            Err(error) => Err(OpenError::new(409, "session_open_name_needs_server", format!(
                "não deu para ler {} ({error}); com o servidor fora ninguém confere o nome — suba o servidor", path.display()))),
        }
    }

    /// `transfer_active`: registro do índice da sessão com o mesmo nome e fase não terminal.
    // ponytail: sem o `_belongs` do Python (vida da sessão); para um nome que nasce agora a vida
    // antiga não existe e o registro em curso é dele. Portar junto quando a troca vier ao Rust.
    fn transfer_busy(&self, name: &str) -> Result<bool, OpenError> {
        let base = self.hangar().join("conversation-transfers");
        let digest: String = ring::digest::digest(&ring::digest::SHA256, name.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let invalid = || OpenError::new(409, "erro_criacao_sessao", "índice de transferência inválido");
        let ids = match read_json(&base.join("indexes").join(format!("session-{digest}.json"))) {
            Ok(None) => return Ok(false),
            Ok(Some(Value::Array(ids))) => ids,
            _ => return Err(invalid()),
        };
        Ok(ids.iter().filter_map(Value::as_str).filter(|id| id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
            .filter_map(|id| read_json(&base.join(format!("{id}.json"))).ok().flatten())
            .any(|record| record["name"] == name && !matches!(record["phase"].as_str(), Some("complete" | "rejected" | "rolled_back"))))
    }

    /// Cada parte que existe é resolvida (link incluído); `..` depois do que não existe é lexical.
    fn real_path(&self, path: &str) -> PathBuf {
        let path = expand_home(path, &self.env().home);
        let path = std::path::absolute(&path).unwrap_or(path);
        let mut out = PathBuf::new();
        for part in path.components() {
            match part {
                std::path::Component::ParentDir => { out.pop(); }
                std::path::Component::CurDir => {}
                part => {
                    out.push(part);
                    if let Ok(real) = out.canonicalize() { out = real; }
                }
            }
        }
        out
    }

    fn new_key(&self) -> String { crate::mods::state::random_hex(16) }

    fn now(&self) -> f64 { crate::accounts::quotas::now() }
}
