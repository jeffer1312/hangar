//! Plano de lançamento do Claude com terminal a partir do spec: o argv do agente, o `-e` da sessão,
//! a conta e o que se prepara antes (pasta confiada, modelo estrangeiro fora do `settings.json`).
//! Port do ramo Claude de `registry.create` com `_env_sessao` e `terminal_command`.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::{Map, Value, json};

use super::terminal::{PaneSpec, TerminalEnv};
use super::{Account, SessionSpec};

/// A rota da conta ChatGPT fixa do motor no `hangar-engine` (`cliproxy.engine_env`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineRoute { pub account: String, pub credential_id: String, pub home: String, pub base_url: String, pub prefix: String }

/// Endereço e segredo da ponte do plugin do Hangar.
#[derive(Clone, Debug)]
pub struct PluginBridge { pub port: u16, pub secret: String }

/// O que vem da máquina e da abertura, além do spec.
pub struct ClaudeContext<'a> {
    /// `~/.hangar/plugin-dir`, o do Hangar primeiro; vazio = mods desligados.
    pub plugin_roots: &'a [String],
    pub bridge: Option<&'a PluginBridge>,
    /// Chave nova a cada vida: o token da ponte é `<chave>.<hmac>`.
    pub bridge_key: &'a str,
    /// `runtime-config.json` da conta do servidor.
    pub runtime_config: &'a Value,
    /// Ambiente do servidor (o padrão das opções que o `runtime-config.json` não traz).
    pub server_env: &'a BTreeMap<String, String>,
    /// As escolhas de plugins e skills da conversa, já preparadas.
    pub settings: Option<&'a Value>,
    /// O envelope `bwrap … --` do só leitura.
    pub read_only: Option<&'a [String]>,
    pub windows: bool,
    /// A primeira mensagem da conversa.
    pub initial_prompt: Option<&'a str>,
    /// A conta ChatGPT fixa do motor, resolvida na abertura.
    pub engine_route: Option<&'a EngineRoute>,
    /// Reabertura de uma conversa que já tem transcript: `--resume` em vez de `--session-id`.
    pub resume: bool,
    /// Plano sobre base bypass: sem a flag a CLI não volta ao bypass ao aprovar o plano.
    pub allow_bypass: bool,
}

/// `runtime_config.get`: o `runtime-config.json` vence; senão a variável `CP_*`; senão o padrão.
fn option(runtime: &Value, env: &BTreeMap<String, String>, name: &str, default: bool) -> bool {
    let from_file = match &runtime[name] {
        Value::Null => None,
        Value::Bool(b) => Some(*b),
        Value::Number(n) => Some(n.as_f64() != Some(0.0)),
        Value::String(s) => Some(!s.is_empty()),
        Value::Array(a) => Some(!a.is_empty()),
        Value::Object(o) => Some(!o.is_empty()),
    };
    from_file.or_else(|| match env.get(&format!("CP_{}", name.to_ascii_uppercase()))?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "f" | "no" | "n" | "off" => Some(false),
        _ => None,
    }).unwrap_or(default)
}

/// O portão de plugins de function hook (`claude_function_hooks`, nasce ligado).
pub fn function_hooks(runtime: &Value, env: &BTreeMap<String, String>) -> bool { option(runtime, env, "claude_function_hooks", true) }

/// `sem_chat_completions`: a base do endereço, sem o `/chat/completions`.
fn without_chat_completions(url: &str) -> String {
    let url = url.trim();
    let (base, query) = url.split_once('?').map_or((url, None), |(b, q)| (b, Some(q)));
    let base = base.trim_end_matches('/');
    let base = base.strip_suffix("/chat/completions").unwrap_or(base);
    match query { Some(q) => format!("{base}?{q}"), None => base.to_owned() }
}

/// `runtime_config.env_jev`, na ordem do Python: o marcador sempre; ligado, chave e destino, o
/// modelo auxiliar e o comando de texto, relidos da configuração agora.
pub fn jev_pairs(on: bool, runtime: &Value) -> Vec<(String, String)> {
    let mut out = vec![("HANGAR_JEV".to_owned(), (if on { "on" } else { "off" }).to_owned())];
    if !on { return out; }
    let text = |field: &str| runtime[field].as_str().map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
    // O destino pela regra única do Rust (`codex_launch::jev_destination`), na ordem do Python.
    let destination = crate::runtime::codex_launch::jev_env(true, runtime);
    for key in ["TYPESAFE_API_KEY", "JEV_ENDPOINT", "JEV_MODEL"] {
        if let Some(value) = destination.get(key) { out.push((key.into(), value.clone())); }
    }
    let aux = ["jev_aux_base_url", "jev_aux_api_key", "jev_aux_modelo", "jev_aux_esforco"];
    let vars = ["JEV_AUX_BASE_URL", "JEV_AUX_API_KEY", "JEV_AUX_MODELO", "JEV_AUX_ESFORCO"];
    let values: Vec<Option<String>> = if aux.iter().any(|field| runtime.get(field).is_some()) {
        aux.iter().map(|field| text(field)).collect()
    } else if let Some(base) = text("jev_texto_base_url") {
        // ponytail: o auxiliar de antes da configuração única vem só do navegador; o do Computer
        // Use (`LLM_*`) fica de fora até alguém o ter sem o `jev_aux_*` gravado.
        vec![Some(without_chat_completions(&base)).filter(|v| !v.is_empty()), text("jev_texto_api_key"), text("jev_texto_modelo"), None]
    } else {
        vec![None; 4]
    };
    out.extend(vars.iter().zip(values).filter_map(|(var, value)| Some((var.to_string(), value?))));
    if let Some(command) = text("jev_texto_cmd") { out.push(("JEV_TEXTO_CMD".into(), command)); }
    out
}

/// `apply_settings` numa linha de comando sem `--settings`: as escolhas da conversa em JSON compacto.
pub(crate) fn settings_json(settings: &Value) -> String {
    let mut merged = Map::new();
    for key in ["enabledPlugins", "skillOverrides"] {
        if let Some(value) = settings.get(key) { merged.insert(key.into(), value.clone()); }
    }
    if let Some(deny) = settings.get("permissions").map(|p| p["deny"].clone()) {
        let mut seen: Vec<Value> = Vec::new();
        for rule in deny.as_array().into_iter().flatten() { if !seen.contains(rule) { seen.push(rule.clone()); } }
        merged.insert("permissions".into(), json!({"deny": seen}));
    }
    Value::Object(merged).to_string()
}

/// O pane do Claude: argv do agente (com o motor e o só leitura em volta) e o `-e` da sessão.
pub fn pane(spec: &SessionSpec, sid: &str, ctx: &ClaudeContext) -> PaneSpec {
    let mut claude: Vec<String> = vec!["claude".into(), (if ctx.resume { "--resume" } else { "--session-id" }).into(), sid.into()];
    for root in ctx.plugin_roots { claude.extend(["--plugin-dir".into(), root.clone()]); }
    if ctx.allow_bypass { claude.push("--allow-dangerously-skip-permissions".into()); }
    if let Some(model) = &spec.model { claude.extend(["--model".into(), model.clone()]); }
    if let Some(effort) = &spec.effort { claude.extend(["--effort".into(), effort.clone()]); }
    if let Some(mode) = spec.permission_mode { claude.extend(["--permission-mode".into(), mode.name().into()]); }
    if let Some(settings) = ctx.settings.filter(|s| s.as_object().is_some_and(|o| !o.is_empty())) {
        claude.extend(["--settings".into(), settings_json(settings)]);
    }
    // O prompt inicial é o posicional do `claude`, depois das opções; começando com `-`, o `--`
    // impede que vire flag.
    if let Some(prompt) = ctx.initial_prompt.filter(|p| !p.is_empty()) {
        if prompt.starts_with('-') { claude.push("--".into()); }
        claude.push(prompt.into());
    }
    let mut command = Vec::new();
    // O ambiente do motor entra dentro do pane (`hangar-engine --exec`): a chave nunca passa por `-e`.
    if let Account::Engine { name, context_window, .. } = &spec.account {
        command.extend(["hangar-engine".into(), "--exec".into(), name.clone()]);
        if let Some(route) = ctx.engine_route {
            command.extend(["--account".into(), route.account.clone(), "--account-home".into(), route.home.clone(),
                "--account-base-url".into(), route.base_url.clone()]);
        }
        if let Some(model) = &spec.model {
            command.extend(["--model".into(), model.clone()]);
            if let Some(window) = context_window { command.extend(["--context".into(), window.to_string()]); }
        }
        if let Some(tier) = spec.service_tier { command.extend(["--service-tier".into(), tier.name().into()]); }
        command.push("--".into());
    }
    command.extend(claude);
    if let Some(prefix) = ctx.read_only {
        command = super::terminal::sandboxed(prefix, &command, ctx.windows);
    }
    let mut env = jev_pairs(spec.jev, ctx.runtime_config);
    env.push(("HANGAR_CLAUDE_SETTINGS".into(), ctx.settings.filter(|s| s.as_object().is_some_and(|o| !o.is_empty()))
        .map(Value::to_string).unwrap_or_default()));
    let hooks = function_hooks(ctx.runtime_config, ctx.server_env);
    if hooks { env.push(("CLAUDE_CODE_ENABLE_FUNCTION_HOOKS".into(), "1".into())); }
    // Tela cheia sempre: só nela o Claude Code liga o mouse, por onde o app aperta os botões dos mods.
    env.push(("CLAUDE_CODE_NO_FLICKER".into(), "1".into()));
    if let Some(bridge) = ctx.bridge.filter(|_| hooks && !ctx.plugin_roots.is_empty()) {
        env.push(("HANGAR_PLUGIN_URL".into(), format!("http://127.0.0.1:{}/api/plugin", bridge.port)));
        env.push(("HANGAR_PLUGIN_TOKEN".into(), crate::mods::bridge::mint_keyed(&bridge.secret, &spec.name, ctx.bridge_key)));
    }
    if let Some(model) = &spec.subagent_model { env.push(("CLAUDE_CODE_SUBAGENT_MODEL".into(), model.clone())); }
    PaneSpec { name: spec.name.clone(), cwd: spec.cwd.clone(), provider: spec.provider, key: spec.key.clone(), env,
        config_dir: spec.account.config_dir().map(str::to_owned), command }
}

/// `~/.hangar/plugin-dir`, que o servidor publica para o terminal: o mesmo `--plugin-dir` em toda
/// abertura. Pasta que sumiu fica de fora; arquivo ausente = mods desligados.
pub fn plugin_roots(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join(".hangar").join("plugin-dir")).unwrap_or_default().lines()
        .map(str::trim_end).filter(|line| !line.is_empty() && Path::new(line).is_dir()).map(str::to_owned).collect()
}

/// O transcript da conversa nova: `projects` da conta pedida, senão o do servidor.
pub fn transcript(spec: &SessionSpec, sid: &str, server_projects: &Path) -> PathBuf {
    let base = spec.account.config_dir().map_or_else(|| server_projects.to_owned(), |dir| Path::new(dir).join("projects"));
    base.join(hangar_workspace::worktrees::sanitize_cwd(&spec.cwd)).join(format!("{sid}.jsonl"))
}

/// `tmux.claude_json_de`: o `.claude.json` que o pane vai ler, pela mesma regra do `-e` da conta.
pub fn claude_json(env: &TerminalEnv, home: &Path, config_dir: Option<&str>) -> PathBuf {
    match super::terminal::exports_config_dir(env, config_dir) {
        Some(dir) => Path::new(&dir).join(".claude.json"),
        None => home.join(".claude.json"),
    }
}

/// `contas.e_conta`: pasta de conta do Hangar de verdade (diretório real com o marcador real, sem
/// preparo pendente). Devolve o nome da conta (`.claude-<nome>`).
pub fn hangar_account(dir: &Path) -> Option<String> {
    let real = std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir());
    let marker = std::fs::symlink_metadata(dir.join(crate::accounts::storage::CLAUDE_MARKER)).is_ok_and(|m| m.is_file());
    let pending = std::fs::symlink_metadata(dir.join(crate::accounts::storage::PENDING)).is_ok();
    (real && marker && !pending).then(|| dir.file_name()?.to_str()?.strip_prefix(".claude-").map(str::to_owned)).flatten()
}

/// A reconciliação da conta do Hangar: os avisos vão à resposta e ao log; falha é recusa com o motivo
/// dela, na frase que o `hangar-conta` imprimiria.
pub fn account_result(account: &str, result: Result<Vec<String>, crate::accounts::claude_config::ConfigError>)
    -> Result<Vec<String>, super::OpenError> {
    let cause = match result {
        Ok(warnings) => {
            for warning in &warnings { tracing::warn!("conta {account}: {warning}"); }
            return Ok(warnings);
        }
        Err(error) => error.cause(),
    };
    Err(super::OpenError::new(500, "erro_conta_reconciliacao_falhou", format!("não consegui reconciliar a conta {account}: {cause}"))
        .with_params(json!({"nome_conta": account, "erro": cause})))
}

static PRETRUST: Mutex<()> = Mutex::new(());

/// Quanto a confiança espera a trava do Claude Code antes de gravar sem ela (como ele mesmo faz).
const CONFIG_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
/// Trava mais velha que isto foi largada por um processo morto (o `stale` padrão do `proper-lockfile`).
const CONFIG_LOCK_STALE: std::time::Duration = std::time::Duration::from_secs(10);

/// A trava que o Claude Code usa no `.claude.json` (`proper-lockfile`: a pasta `<arquivo real>.lock`).
/// Sem ela, uma sessão viva da conta que já tinha lido o arquivo regrava a cópia dela por cima da confiança.
pub(crate) struct ConfigLock(PathBuf, Option<std::time::SystemTime>);

impl Drop for ConfigLock {
    fn drop(&mut self) {
        // Outro processo que a julgou vencida e pegou a dele tem outra data: essa não é nossa para apagar.
        let current = std::fs::metadata(&self.0).and_then(|m| m.modified()).ok();
        if self.1.is_some() && current != self.1 { return; }
        // Trava que fica para trás segura o Claude Code da conta até ela vencer.
        if let Err(e) = std::fs::remove_dir(&self.0) && e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(lock = %self.0.display(), error = %e, "trava do .claude.json não saiu; o Claude Code espera ela vencer");
        }
    }
}

fn config_lock_path(file: &Path) -> PathBuf {
    let mut lock = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned()).into_os_string();
    lock.push(".lock");
    PathBuf::from(lock)
}

pub(crate) fn lock_config(file: &Path) -> Result<ConfigLock, String> {
    let lock = config_lock_path(file);
    let deadline = std::time::Instant::now() + CONFIG_LOCK_WAIT;
    loop {
        match std::fs::create_dir(&lock) {
            Ok(()) => {
                let created = std::fs::metadata(&lock).and_then(|m| m.modified()).ok();
                return Ok(ConfigLock(lock, created));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Vencida não tem dono vivo para disputar. Apagá-la pelo caminho é que arrisca: entre ler a
                // data e apagar, outro processo pode trocá-la pela dele. O Claude Code a recupera sozinho.
                let stale = std::fs::metadata(&lock).and_then(|m| m.modified()).ok()
                    .and_then(|t| t.elapsed().ok()).is_some_and(|age| age > CONFIG_LOCK_STALE);
                if stale { return Err("trava largada".into()); }
                if std::time::Instant::now() >= deadline { return Err("trava ocupada".into()); }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// `_pretrust_cwd`: a pasta nasce confiada no `.claude.json` que a sessão lê (senão ela nasce presa
/// no "trust this folder?", invisível para o app). Já confiada não reescreve; a falha volta a quem chamou.
pub fn pretrust(file: &Path, cwd: &str, windows: bool) -> Result<(), String> {
    let _guard = PRETRUST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let read = || -> Result<Value, String> {
        match std::fs::read(file) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
            Err(e) => Err(e.to_string()),
        }
    };
    // O Claude Code indexa com barra normal no Windows.
    let key = if windows { cwd.replace('\\', "/") } else { cwd.to_owned() };
    let trusted = |data: &Value| data.get("projects").and_then(|p| p.get(&key)).and_then(|e| e.get("hasTrustDialogAccepted")) == Some(&Value::Bool(true));
    let result = (|| -> Result<(), String> {
        // Pasta já confiada não disputa a trava com as sessões vivas. Esta leitura é sem a trava e pode
        // pegar o arquivo no meio da gravação de outra sessão: ilegível aqui segue para a leitura travada.
        if read().is_ok_and(|data| trusted(&data)) { return Ok(()); }
        let _lock = lock_config(file).inspect_err(|error| {
            tracing::warn!(file = %file.display(), error, "trava do .claude.json indisponível; a confiança grava sem ela");
        }).ok();
        let mut data = read()?;
        let projects = data.as_object_mut().ok_or("não é objeto")?.entry("projects").or_insert_with(|| json!({}));
        let entry = projects.as_object_mut().ok_or("projects não é objeto")?.entry(key.clone()).or_insert_with(|| json!({}));
        let entry = entry.as_object_mut().ok_or("projeto não é objeto")?;
        if entry.get("hasTrustDialogAccepted") == Some(&Value::Bool(true)) { return Ok(()); }
        entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
        let text = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
        crate::runtime::queue::atomic_write(file, text.as_bytes()).map_err(|e| e.to_string())
    })();
    if let Err(error) = &result { tracing::warn!(file = %file.display(), error, "pré-confiar a pasta falhou; o diálogo aparece na sessão"); }
    result
}

/// `pretrust` fora das threads do runtime: a espera pela trava dorme.
pub async fn pretrust_async(file: PathBuf, cwd: String, windows: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || pretrust(&file, &cwd, windows)).await.map_err(|e| format!("a confiança caiu: {e}"))?
}

/// O que o `.claude.json` diz da pasta quando o "trust this folder?" aparece mesmo assim: o código vai ao
/// diário, e o detalhe (marca, chaves da entrada, idade do arquivo, trava) ao log.
pub fn trust_evidence(file: &Path, cwd: &str, windows: bool) -> (&'static str, String) {
    let key = if windows { cwd.replace('\\', "/") } else { cwd.to_owned() };
    let locked = config_lock_path(file).is_dir();
    let real = std::fs::canonicalize(cwd).ok().map(|p| p.display().to_string()).filter(|real| *real != cwd);
    let age = std::fs::metadata(file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs());
    let data: Value = match std::fs::read(file).map(|bytes| serde_json::from_slice(&bytes)) {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => return ("trust_config_unreadable", format!("file={} json={e}", file.display())),
        Err(e) => return ("trust_config_unreadable", format!("file={} io={e}", file.display())),
    };
    let entry = data.get("projects").and_then(|p| p.get(&key));
    let keys: Vec<&str> = entry.and_then(Value::as_object).map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
    let code = match entry.and_then(|e| e.get("hasTrustDialogAccepted")) {
        Some(Value::Bool(true)) => "trust_flag_present",
        _ => "trust_flag_absent",
    };
    (code, format!("file={} key={key} real={real:?} entry_keys={keys:?} file_age_s={age:?} lock={locked}", file.display()))
}

/// Ids que a API da Anthropic aceita (`default_model._ANTHROPIC`).
static ANTHROPIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"(?i)\A(?:(?:[a-z]{2,}\.)?(?:anthropic\.)?claude-[\w.:@-]+|arn:aws[\w-]*:bedrock:\S+|default|best|opus|opusplan|sonnet|haiku|fable)(?:\[1m\])?\z").unwrap());

/// `default_model.drop_foreign`: tira do `settings.json` da conta e do principal um `"model"` que
/// não é da Anthropic (um `/model` digitado numa sessão de motor). Devolve os avisos da resposta.
pub fn drop_foreign_model(home: &Path, config_dir: Option<&str>) -> Vec<String> {
    let principal = home.join(".claude").join("settings.json");
    let mut files = vec![config_dir.map_or_else(|| principal.clone(), |dir| Path::new(dir).join("settings.json"))];
    if files[0] != principal { files.push(principal); }
    let mut warnings = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(Value::Object(mut data)) = serde_json::from_str::<Value>(text.trim()) else { continue };
        let Some(model) = data.get("model").and_then(Value::as_str).map(str::to_owned) else { continue };
        if ANTHROPIC.is_match(model.trim()) { continue; }
        data.shift_remove("model");
        let written = serde_json::to_string_pretty(&Value::Object(data)).map(|t| t + "\n").map_err(std::io::Error::other)
            .and_then(|t| crate::runtime::queue::atomic_write(&path, t.as_bytes()));
        match written {
            Ok(()) => warnings.push({
                tracing::warn!("modelo padrão '{model}' não é da Anthropic; removido do settings.json");
                format!("O modelo padrão '{model}' do settings.json não é da Anthropic \
                (provavelmente veio de um /model numa sessão de motor) e foi removido.")
            }),
            Err(e) => warnings.push(format!("Não consegui tirar do settings.json um modelo que não é da Anthropic: {}: {e}", path.display())),
        }
    }
    warnings
}

