//! Comando e ambiente do `claude -p` que o Rust sobe no cano, a partir do spec da sessão e do sidecar
//! dela: port de `_argv` e `_launch_account_cano_owned` (`adapters/claude_headless/adapter.py`).
//! Montado a cada subida (abrir, religar, reabrir) com a configuração de agora: plugins, Jev, portão de
//! function hooks e a ponte do plugin nunca são copiados do processo que morre.
//! O ambiente é segredo: sai daqui só para o `process::spawn`, nunca para log nem disco.
use super::protocol::RuntimeError;
use crate::accounts::environment::AccountEnvironment;
use crate::session_open::claude::{EngineRoute, PluginBridge};
use crate::session_open::{Account, PermissionMode, SessionSpec};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// O comando e o ambiente da subida do Claude sem terminal.
#[derive(Debug)]
pub struct Launch { pub program: Vec<String>, pub env: BTreeMap<String, String> }

/// O que vem da máquina, além do spec e do sidecar.
pub struct Context<'a> {
    pub env: &'a AccountEnvironment,
    /// `runtime-config.json` da conta do servidor.
    pub runtime_config: &'a Value,
    /// `~/.hangar/plugin-dir`, o do Hangar primeiro; vazio = mods desligados.
    pub plugin_roots: &'a [String],
    pub bridge: Option<&'a PluginBridge>,
    /// A conta ChatGPT fixa do motor, resolvida agora.
    pub engine_route: Option<&'a EngineRoute>,
    /// O motor e o modelo aceitam Fast (`cliproxy.supports_fast`).
    pub fast: bool,
}

/// `ad._SO_TERMINAL_PROTEGIDO`.
pub const READ_ONLY_MSG: &str = "sessão read-only só roda no terminal protegido; volte-a para o terminal";

const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Herdadas do backend subido de dentro de outra sessão (ou dele mesmo), dariam a esta o pane, o motor,
/// a ponte, o subagente, o Jev, o portão de hooks ou o segredo de outra.
const DROPPED: [&str; 22] = ["TMUX", "TMUX_PANE", "CP_ENGINE_ACCOUNT", "CP_ENGINE_CREDENTIAL_ID", "CP_ENGINE_ACCOUNT_BASE_URL",
    "CP_ENGINE_SERVICE_TIER", "HANGAR_PLUGIN_URL", "HANGAR_PLUGIN_TOKEN", "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_ENABLE_FUNCTION_HOOKS", "TYPESAFE_API_KEY", "JEV_ENDPOINT", "JEV_MODEL", "JEV_TEXTO_BASE_URL",
    "JEV_TEXTO_API_KEY", "JEV_TEXTO_MODELO", "JEV_TEXTO_CMD", "JEV_AUX_BASE_URL", "JEV_AUX_API_KEY", "JEV_AUX_MODELO",
    "JEV_AUX_ESFORCO", "CP_AUTH_TOKEN"];
const SECRETS: [&str; 3] = ["HANGAR_INTERNAL_SECRET", "HANGAR_RUNTIME_INSTANCE", "HANGAR_AUTH_CONTEXT"];

fn refused(code: &str, message: &str) -> RuntimeError { RuntimeError::new(code, message) }

/// Falha que nenhuma nova tentativa resolve (motor, conta fixa, modo, Fast, só leitura, cano ausente):
/// a religação para na hora e mostra o motivo, sem gastar as esperas.
pub fn permanent(code: &str) -> bool { code.starts_with("launch_") || matches!(code, "headless_read_only" | "cano_ausente") }

fn text<'a>(meta: &'a Value, key: &str) -> Option<&'a str> { meta[key].as_str().filter(|v| !v.is_empty()) }

/// `transcript_path_de`: o `.jsonl` da conversa na conta da sessão, ou onde o `EnterWorktree` o pôs.
pub fn transcript(env: &AccountEnvironment, spec: &SessionSpec, sid: &str) -> PathBuf {
    let base = spec.account.config_dir().map_or_else(|| env.claude_base.join("projects"), |dir| Path::new(dir).join("projects"));
    let expected = base.join(hangar_workspace::worktrees::sanitize_cwd(&spec.cwd)).join(format!("{sid}.jsonl"));
    if expected.exists() { return expected; }
    let moved = std::fs::read_dir(&base).into_iter().flatten().flatten()
        .map(|dir| dir.path().join(format!("{sid}.jsonl"))).find(|path| path.is_file());
    moved.unwrap_or(expected)
}

/// `permissions.defaultMode` do `settings.json` da conta (a do servidor sem pasta própria), como a
/// abertura decide: ausente ou fora da lista é bypass, `default` é manual; ilegível recusa.
pub(crate) fn account_mode(env: &AccountEnvironment, config_dir: Option<&str>) -> Result<PermissionMode, RuntimeError> {
    let dir = config_dir.map_or_else(|| env.claude_base.clone(), PathBuf::from);
    let mode = match std::fs::read(dir.join("settings.json")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(refused("launch_permission_mode", "o settings.json da conta não pôde ser lido; o modo da sessão não foi decidido")),
        Ok(raw) => serde_json::from_slice::<Value>(&raw)
            .map_err(|_| refused("launch_permission_mode", "o settings.json da conta é ilegível; o modo da sessão não foi decidido"))?
            ["permissions"]["defaultMode"].as_str().map(str::to_owned),
    };
    Ok(match mode.as_deref() {
        Some("default") => PermissionMode::Manual,
        Some(mode) => PermissionMode::claude(mode).unwrap_or(PermissionMode::BypassPermissions),
        None => PermissionMode::BypassPermissions,
    })
}

/// `pensamento.ler()`: `showThinkingSummaries` do `settings.json` compartilhado (o `~/.claude` real).
fn thinking_summaries(home: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(home.join(".claude").join("settings.json")) else { return false };
    serde_json::from_str::<Value>(raw.trim()).ok().is_some_and(|data| crate::list::discover_other::truthy(&data["showThinkingSummaries"]))
}

/// O comando e o ambiente do `claude -p` desta subida. `meta` é o sidecar (conversa, escolhas de plugins
/// e skills preparadas, base fora do plano); o resto vem do spec.
pub fn plan(spec: &SessionSpec, meta: &Value, ctx: &Context) -> Result<Launch, RuntimeError> {
    // Sem terminal não há bwrap: o Claude subiria com o código gravável.
    if spec.read_only || meta["read_only"] == true { return Err(refused("headless_read_only", READ_ONLY_MSG)); }
    let sid = text(meta, "session_id").ok_or_else(|| refused("launch_meta", "sidecar sem `session_id`"))?;
    // Matriz, item 17: modo ausente é o `defaultMode` da conta; gravado e desconhecido não vira um
    // modo inventado.
    let mode = match spec.permission_mode.filter(|m| PermissionMode::CLAUDE.contains(m)) {
        Some(mode) => mode,
        None if meta["permission_mode"].is_null() => account_mode(ctx.env, spec.account.config_dir())?,
        None => return Err(refused("launch_permission_mode", "modo de permissão da sessão desconhecido; a sessão não foi aberta")),
    };
    let engine = match &spec.account { Account::Engine { name, context_window, .. } => Some((name, context_window)), _ => None };
    let tier = match spec.service_tier {
        Some(crate::session_open::ServiceTier::Priority) if !ctx.fast =>
            return Err(refused("launch_service_tier", "service_tier exige Claude com motor GPT no CLIProxyAPI local")),
        Some(tier) if ctx.fast => Some(tier),
        _ => None,
    };
    let resume = transcript(ctx.env, spec, sid).exists();
    let mut claude: Vec<String> = ["claude", "-p", "--output-format", "stream-json", "--input-format", "stream-json", "--verbose",
        "--include-partial-messages", "--permission-prompt-tool", "stdio", "--setting-sources", "user,project,local",
        // O eco com o mesmo `uuid` é a prova de entrega (o prompt barrado por hook não ecoa).
        "--replay-user-messages"].map(str::to_owned).into();
    claude.extend([(if resume { "--resume" } else { "--session-id" }).to_owned(), sid.to_owned()]);
    for root in ctx.plugin_roots { claude.extend(["--plugin-dir".into(), root.clone()]); }
    // Com `-p` a CLI ignora `showThinkingSummaries`; só a flag traz o texto do pensamento.
    if thinking_summaries(&ctx.env.home) { claude.extend(["--thinking-display".into(), "summarized".into()]); }
    // Base bypass rodando noutro modo (o plano): sem a flag a CLI recusa voltar ao bypass.
    if meta["previous_non_plan"] == "bypassPermissions" && mode != PermissionMode::BypassPermissions {
        claude.push("--allow-dangerously-skip-permissions".into());
    }
    if let Some(model) = spec.model.as_deref().filter(|m| !m.is_empty()) { claude.extend(["--model".into(), model.into()]); }
    // `ultracode` só existe no `/effort` em voo: a flag não o aceita.
    if let Some(effort) = spec.effort.as_deref().filter(|e| EFFORTS.contains(e)) { claude.extend(["--effort".into(), effort.into()]); }
    // O modo também no `--resume`: sem a flag a CLI volta ao `defaultMode` da conta.
    claude.extend(["--permission-mode".into(), mode.name().into()]);
    let settings = meta.get("claude_settings").filter(|s| s.as_object().is_some_and(|o| !o.is_empty()));
    if let Some(settings) = settings { claude.extend(["--settings".into(), crate::session_open::claude::settings_json(settings)]); }
    let mut program = Vec::new();
    if let Some((name, context_window)) = engine {
        program.extend(["hangar-engine".into(), "--exec".into(), name.clone()]);
        if let Some(route) = ctx.engine_route {
            program.extend(["--account".into(), route.account.clone(), "--account-home".into(), route.home.clone(),
                "--account-base-url".into(), route.base_url.clone()]);
        }
        if let Some(model) = spec.model.as_deref().filter(|m| !m.is_empty()) {
            program.extend(["--model".into(), model.into()]);
            if let Some(window) = context_window { program.extend(["--context".into(), window.to_string()]); }
        }
        if let Some(tier) = tier { program.extend(["--service-tier".into(), tier.name().into()]); }
        program.push("--".into());
    }
    program.extend(claude);

    let mut env = ctx.env.base.clone();
    for name in DROPPED.iter().chain(SECRETS.iter()) { env.remove(*name); }
    let key = &spec.key;
    env.insert("CP_SESSION_NAME".into(), spec.name.clone());
    env.insert("CP_PROVIDER".into(), "claude".into());
    env.insert("HANGAR_CLAUDE_SETTINGS".into(), settings.map(Value::to_string).unwrap_or_default());
    let hooks = crate::session_open::claude::function_hooks(ctx.runtime_config, &ctx.env.base);
    // A chave da sessão vai no token: é por ela que a ponte acha a sessão, também depois de renomear.
    if let Some(bridge) = ctx.bridge.filter(|_| hooks && !ctx.plugin_roots.is_empty()) {
        env.insert("HANGAR_PLUGIN_URL".into(), format!("http://127.0.0.1:{}/api/plugin", bridge.port));
        env.insert("HANGAR_PLUGIN_TOKEN".into(), crate::mods::bridge::mint_keyed(&bridge.secret, &spec.name, key));
    }
    env.insert("CP_SESSION_KEY".into(), key.clone());
    env.insert("HANGAR_CANO_KEY".into(), key.clone());
    env.insert("HANGAR_CANO_OWNER".into(), ctx.env.home.to_string_lossy().into_owned());
    if let Some(dir) = spec.account.config_dir() { env.insert("CLAUDE_CONFIG_DIR".into(), dir.into()); }
    if let Some(model) = &spec.subagent_model { env.insert("CLAUDE_CODE_SUBAGENT_MODEL".into(), model.clone()); }
    env.extend(crate::session_open::claude::jev_pairs(spec.jev, ctx.runtime_config));
    if hooks { env.insert("CLAUDE_CODE_ENABLE_FUNCTION_HOOKS".into(), "1".into()); }
    env.insert(crate::vault::exec::CLAUDE_IDLE_VAR.into(), crate::vault::exec::CLAUDE_IDLE_MS.into());
    Ok(Launch { program, env })
}

/// O sidecar da vida nova de um relançamento: a mesma conversa, o modo pedido e o modelo e o esforço
/// que a vida viva usava (o `/model` e o `/effort` dela); o resto é o que o sidecar guarda.
pub fn relaunch_meta(mut sidecar: Value, conversation: &str, mode: &str, model: &Value, effort: &Value) -> Value {
    sidecar["session_id"] = Value::from(conversation);
    sidecar["permission_mode"] = Value::from(mode);
    for (field, value) in [("model", model), ("effort", effort)] {
        if value.as_str().is_some_and(|v| !v.is_empty()) { sidecar[field] = value.clone(); }
    }
    sidecar
}

/// A janela do motor que a abertura não leu é relida a cada subida; lida, vai ao sidecar.
pub(crate) fn refresh_window(spec: &mut SessionSpec, facts: &dyn crate::session_open::OpenFacts, sidecar_dir: &Path, name: &str) {
    if !matches!(spec.account, Account::Engine { context_window_unknown: true, .. }) { return; }
    spec.refresh_engine_window(facts);
    let Account::Engine { context_window: Some(window), context_window_unknown: false, .. } = spec.account else { return };
    let patch = [("context_window".to_owned(), Value::from(window))].into_iter().collect();
    match super::codex_sidecar::update_if(sidecar_dir, name, &[("key", Value::from(spec.key.clone()))], &patch) {
        Ok(Some(_)) => {}
        Ok(None) => tracing::warn!(session = %name, "sidecar mudou de vida; a janela do motor não foi gravada"),
        Err(error) => tracing::warn!(session = %name, kind = ?error.kind(), "a janela do motor não foi gravada no sidecar"),
    }
}

/// A subida de agora da sessão `name` (o sidecar é `meta`): o spec vivo, a configuração do servidor, a
/// conta ChatGPT fixa do motor e o Fast, relidos aqui. Bloqueante: chamar fora do runtime.
pub fn prepare(env: &AccountEnvironment, bridge: Option<&PluginBridge>, meta: &Value, name: &str,
               runtime: tokio::runtime::Handle) -> Result<Launch, RuntimeError> {
    let mut spec = crate::session_open::record::claude_live(&env.home, meta, name)
        .map_err(|error| refused("launch_meta", &format!("registro da sessão ilegível: {error}")))?;
    let facts = crate::session_open::facts::LocalFacts::new(env.clone(), runtime);
    refresh_window(&mut spec, &facts, &env.home.join(".hangar").join("claude-headless"), name);
    plan_for(env, bridge, &spec, meta, &facts)
}

/// O comando de agora para um spec já decidido, sem sessão registrada: a subida da sessão (`prepare`) e
/// a reserva pré-aquecida, que ainda não tem sidecar nem registro. Bloqueante: chamar fora do runtime.
pub fn plan_for(env: &AccountEnvironment, bridge: Option<&PluginBridge>, spec: &SessionSpec, meta: &Value,
                facts: &dyn crate::session_open::OpenFacts) -> Result<Launch, RuntimeError> {
    let config = super::codex_launch::runtime_config(env);
    let roots = crate::session_open::claude::plugin_roots(&env.home);
    let (engine, account) = match &spec.account {
        Account::Engine { name, account, .. } => (Some(name.as_str()), account.as_deref()),
        _ => (None, None),
    };
    let route = match (engine, account) {
        (Some(engine), Some(account)) => Some(crate::session_open::facts::fixed_account(
            &crate::accounts::AccountService::new(env.clone()), engine, account)
            .map_err(|error| refused("launch_engine_account", &error.msg))?),
        _ => None,
    };
    let fast = spec.service_tier.is_some() && facts.supports_fast(engine, spec.model.as_deref());
    plan(spec, meta, &Context { env, runtime_config: &config, plugin_roots: &roots, bridge, engine_route: route.as_ref(), fast })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// O cenário do fixture do Python com os marcadores trocados pela raiz desta árvore.
    struct Case { root: PathBuf, repo: PathBuf, fixture: Value, sid: String, key: String }

    fn case(name: &str) -> Case {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        let raw = std::fs::read_to_string(repo.join(format!("backend/tests/fixtures/contract/session-open/{name}.json"))).unwrap();
        let root = tempfile::tempdir().unwrap().keep();
        let (sid, key) = ("0c657c79-1745-406b-a97f-bbbf5402ff19".to_owned(), "ab".repeat(16));
        // A troca acontece dentro de strings JSON: a barra invertida do caminho no Windows vai escapada.
        let quoted = |path: &Path| path.to_string_lossy().replace('\\', "\\\\");
        let text = raw.replace("<ROOT>", &quoted(&root)).replace("<REPO>", &quoted(&repo))
            .replace("<uuid:1>", &sid).replace("<hex:2>", &key);
        Case { root, repo, fixture: serde_json::from_str(&text).unwrap(), sid, key }
    }

    impl Case {
        /// `base_env` do harness: é o ambiente do servidor que o lado Python herdou.
        fn server_env(&self) -> BTreeMap<String, String> {
            let root = self.root.to_string_lossy();
            [("HOME", format!("{root}/home")), ("PATH", format!("{root}/bin:/usr/bin:/bin")), ("LANG", "C.UTF-8".into()),
             ("CP_AUTH_TOKEN", "hso-token".into()), ("XDG_RUNTIME_DIR", format!("{root}/run"))]
                .into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
        }
        fn launched(&self) -> &Value {
            self.fixture["result"]["subprocesses"].as_array().unwrap().iter().find(|s| !s["launch"].is_null()).unwrap()
        }
        /// O argv do `claude` (ou do `hangar-engine`) depois do `--` do cano, com o executável sem a pasta.
        fn expected_program(&self) -> Vec<String> {
            let argv: Vec<String> = serde_json::from_value(self.launched()["argv"].clone()).unwrap();
            let mut program: Vec<String> = argv.into_iter().skip_while(|a| a != "--").skip(1).collect();
            program[0] = Path::new(&program[0]).file_name().unwrap().to_string_lossy().into_owned();
            program
        }
        fn spec(&self) -> SessionSpec {
            let request = crate::session_open::SessionOpenRequest::parse(self.fixture["scenario"]["steps"][0]["json"].to_string().as_bytes()).unwrap();
            let sidecar = &self.fixture["result"]["records"][format!("{}/home/.hangar/claude-headless/s1.json", self.root.display())]["created"]["json"];
            let mut spec = crate::session_open::record::claude_from_meta(sidecar, "s1", 0.0).unwrap();
            spec.key = self.key.clone();
            spec.jev = request.jev.unwrap_or(false);
            spec
        }
        fn meta(&self) -> Value {
            self.fixture["result"]["records"][format!("{}/home/.hangar/claude-headless/s1.json", self.root.display())]["created"]["json"].clone()
        }
    }

    fn launch_of(case: &Case, runtime: &Value) -> Launch {
        let env = AccountEnvironment::from_map(case.server_env());
        // Na forma do fixture (`<REPO>/plugins/...`), que o Python montou com `/`.
        let roots = ["hangar", "github-actions"].map(|p| format!("{}/plugins/{p}", case.repo.display()));
        let bridge = PluginBridge { port: 8765, secret: "hso-token".into() };
        let ctx = Context { env: &env, runtime_config: runtime, plugin_roots: &roots, bridge: Some(&bridge), engine_route: None, fast: false };
        plan(&case.spec(), &case.meta(), &ctx).unwrap()
    }

    /// O ambiente que o Python deu ao processo: o do servidor mais o `set` do fixture.
    fn expected_env(case: &Case, launch: &Launch) -> BTreeMap<String, String> {
        let mut env = case.server_env();
        for (k, v) in case.launched()["env"]["set"].as_object().unwrap() { env.insert(k.clone(), v.as_str().unwrap().to_owned()); }
        // O token é um HMAC: o fixture só guarda a forma. A chave da sessão vem antes do ponto.
        let token = env.get_mut("HANGAR_PLUGIN_TOKEN").unwrap();
        let minted = &launch.env["HANGAR_PLUGIN_TOKEN"];
        assert!(minted.starts_with(&format!("{}.", case.key)) && minted.len() == 32 + 1 + 32, "token da ponte pela chave: {minted}");
        *token = minted.clone();
        // O bearer do dono nunca chega ao `claude`.
        env.remove("CP_AUTH_TOKEN");
        // Matriz, item 1: `CP_PROVIDER` em toda sessão.
        env.insert("CP_PROVIDER".into(), "claude".into());
        // Diferença aceita: o prazo das tools do cofre não existe no lançamento do Python.
        env.insert(crate::vault::exec::CLAUDE_IDLE_VAR.into(), crate::vault::exec::CLAUDE_IDLE_MS.into());
        env
    }

    #[test]
    fn plan_matches_the_python_launch_of_a_new_conversation() {
        let case = case("create_claude_headless_then_launch");
        let launch = launch_of(&case, &json!({"jev_api_key": "tsk-fake"}));
        assert_eq!(launch.program, case.expected_program());
        assert_eq!(launch.env, expected_env(&case, &launch));
    }

    #[test]
    fn plan_matches_the_python_launch_with_engine_and_with_customizations() {
        for name in ["create_claude_headless_engine_then_launch", "create_claude_headless_customizations_then_launch"] {
            let case = case(name);
            let launch = launch_of(&case, &json!({}));
            assert_eq!(launch.program, case.expected_program(), "{name}");
            assert_eq!(launch.env, expected_env(&case, &launch), "{name}");
        }
    }

    #[test]
    fn existing_transcript_resumes_and_keeps_the_mode() {
        let case = case("create_claude_headless_then_launch");
        let projects = case.root.join("home/.claude-b/projects").join(hangar_workspace::worktrees::sanitize_cwd(&format!("{}/work/proj", case.root.display())));
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(projects.join(format!("{}.jsonl", case.sid)), "{}\n").unwrap();
        let program = launch_of(&case, &json!({})).program;
        let at = program.iter().position(|a| a == "--resume").expect("--resume com o transcript no disco");
        assert_eq!(program[at + 1], case.sid);
        assert!(!program.contains(&"--session-id".to_owned()));
        assert!(program.windows(2).any(|w| w == ["--permission-mode", "plan"]), "o modo também no --resume");
    }

    /// Relançar (queda, bypass, cura) parte do spec com a configuração de agora: o plugin e o portão de
    /// hooks entram mesmo que a vida que morre não os tivesse, e o modo pedido vai explícito.
    #[test]
    fn relaunch_uses_the_spec_with_the_requested_mode_and_the_live_choices() {
        let case = case("create_claude_headless_then_launch");
        let meta = relaunch_meta(case.meta(), &case.sid, "bypassPermissions", &json!("sonnet"), &json!(null));
        assert_eq!((meta["permission_mode"].as_str(), meta["model"].as_str(), meta["effort"].as_str()),
            (Some("bypassPermissions"), Some("sonnet"), Some("xhigh")), "o esforço sem valor vivo fica o do sidecar");
        let env = AccountEnvironment::from_map(case.server_env());
        let roots = [case.repo.join("plugins/hangar").to_string_lossy().into_owned()];
        let bridge = PluginBridge { port: 8765, secret: "hso-token".into() };
        let ctx = Context { env: &env, runtime_config: &json!({}), plugin_roots: &roots, bridge: Some(&bridge), engine_route: None, fast: false };
        let spec = crate::session_open::record::claude_live(&case.root.join("home"), &meta, "s1").unwrap();
        let launch = plan(&spec, &meta, &ctx).unwrap();
        assert!(launch.program.windows(2).any(|w| w == ["--permission-mode", "bypassPermissions"]));
        assert!(launch.program.windows(2).any(|w| w == ["--model", "sonnet"]));
        assert!(launch.program.contains(&"--plugin-dir".to_owned()) && launch.env.contains_key("HANGAR_PLUGIN_TOKEN"));
        assert!(!launch.program.contains(&"--allow-dangerously-skip-permissions".to_owned()), "em bypass a flag de volta não entra");
    }

    /// A vida nova nunca sai de uma ficha montada às pressas; do spec gravado vêm a janela do
    /// motor, a base fora do plano e o só leitura (que segue recusado sem terminal).
    #[test]
    fn relaunch_from_the_spec_keeps_engine_window_base_mode_and_read_only() {
        let case = case("create_claude_headless_engine_then_launch");
        let mut meta = case.meta();
        meta["context_window"] = json!(262144);
        meta["previous_non_plan"] = json!("bypassPermissions");
        let meta = relaunch_meta(meta, &case.sid, "plan", &json!(null), &json!(null));
        let env = AccountEnvironment::from_map(case.server_env());
        let ctx = Context { env: &env, runtime_config: &json!({}), plugin_roots: &[], bridge: None, engine_route: None, fast: false };
        let spec = crate::session_open::record::claude_live(&case.root.join("home"), &meta, "s1").unwrap();
        let program = plan(&spec, &meta, &ctx).unwrap().program;
        let engine: Vec<&str> = program.iter().take_while(|a| *a != "--").map(String::as_str).collect();
        assert_eq!(engine, ["hangar-engine", "--exec", "kimi", "--model", "k3-256k", "--context", "262144"]);
        assert!(program.contains(&"--allow-dangerously-skip-permissions".to_owned()), "base bypass no plano volta ao bypass");
        let mut read_only = meta.clone();
        read_only["read_only"] = json!(true);
        let spec = crate::session_open::record::claude_live(&case.root.join("home"), &read_only, "s1").unwrap();
        assert_eq!(plan(&spec, &read_only, &ctx).unwrap_err().code, "headless_read_only");
    }

    #[test]
    fn permanent_launch_failures_are_told_apart_from_passing_ones() {
        let case = case("create_claude_headless_then_launch");
        let env = AccountEnvironment::from_map(case.server_env());
        let ctx = Context { env: &env, runtime_config: &json!({}), plugin_roots: &[], bridge: None, engine_route: None, fast: false };
        let mut spec = case.spec();
        spec.permission_mode = None;
        assert!(permanent(&plan(&spec, &case.meta(), &ctx).unwrap_err().code));
        for passing in ["cano_nao_escutou", "cano_nao_subiu", "relaunch_initialize", "lifecycle_busy"] { assert!(!permanent(passing), "{passing}"); }
    }

    /// Matriz, item 17: sidecar sem modo (o do Python antigo) sobe no `defaultMode` da conta, explícito;
    /// modo gravado que o Hangar não conhece é recusado.
    #[test]
    fn absent_mode_uses_the_account_default_and_unknown_is_refused() {
        let case = case("create_claude_headless_then_launch");
        let env = AccountEnvironment::from_map(case.server_env());
        let ctx = Context { env: &env, runtime_config: &json!({}), plugin_roots: &[], bridge: None, engine_route: None, fast: false };
        let mut meta = case.meta();
        meta.as_object_mut().unwrap().remove("permission_mode");
        let mode_of = |meta: &Value| {
            let spec = crate::session_open::record::claude_live(&case.root.join("home"), meta, "s1").unwrap();
            plan(&spec, meta, &ctx).map(|l| l.program.windows(2).find(|w| w[0] == "--permission-mode").map(|w| w[1].clone()))
        };
        assert_eq!(mode_of(&meta).unwrap().as_deref(), Some("bypassPermissions"), "conta sem defaultMode");
        let settings = case.root.join("home/.claude-b/settings.json");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, r#"{"permissions":{"defaultMode":"acceptEdits"}}"#).unwrap();
        assert_eq!(mode_of(&meta).unwrap().as_deref(), Some("acceptEdits"));
        meta["permission_mode"] = json!("Tudo");
        assert_eq!(mode_of(&meta).unwrap_err().code, "launch_permission_mode");
    }

    #[test]
    fn unknown_mode_read_only_and_priority_without_fast_are_refused() {
        let case = case("create_claude_headless_then_launch");
        let env = AccountEnvironment::from_map(case.server_env());
        let ctx = Context { env: &env, runtime_config: &json!({}), plugin_roots: &[], bridge: None, engine_route: None, fast: false };
        let mut spec = case.spec();
        spec.permission_mode = None;
        assert_eq!(plan(&spec, &case.meta(), &ctx).unwrap_err().code, "launch_permission_mode");
        let mut spec = case.spec();
        spec.read_only = true;
        assert_eq!(plan(&spec, &case.meta(), &ctx).unwrap_err().code, "headless_read_only");
        let mut spec = case.spec();
        spec.service_tier = Some(crate::session_open::ServiceTier::Priority);
        assert_eq!(plan(&spec, &case.meta(), &ctx).unwrap_err().code, "launch_service_tier");
        spec.service_tier = Some(crate::session_open::ServiceTier::Default);
        assert!(!plan(&spec, &case.meta(), &ctx).unwrap().program.contains(&"--service-tier".to_owned()), "default sem Fast é ignorado");
    }
}
