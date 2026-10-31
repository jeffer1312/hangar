//! Plano de lançamento do Codex a partir do spec: o comando do `hangar-codex-tui` (o mesmo na
//! criação e no relançamento), o `-e` da sessão, o sidecar sem terminal e a pasta pré-confiada no
//! `config.toml` da conta. Port do ramo Codex de `registry.create`, `_create_codex_headless`,
//! `lancador.comando_do_lancador` e `sessions.pretrust_cwd`.
use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};

use super::terminal::PaneSpec;
use super::{AgentProvider, OpenError, PermissionMode, ServiceTier, SessionSpec};

/// `lancador.EXECUTAVEL`: o mesmo nome no PATH (no Windows, o `.cmd` do `install.ps1`).
pub const LAUNCHER: &str = "hangar-codex-tui";

/// O que o lançador recebe, venha do spec (criação) ou do sidecar (relançar a conversa aberta).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub cwd: String,
    pub codex_home: String,
    /// Id da conta (`default` na padrão).
    pub account: String,
    pub service_tier: Option<String>,
    pub resume: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Nome do modo do app; vira `--approval-policy`/`--sandbox`, nunca o padrão calado do lançador.
    pub mode: String,
    pub tool_output_token_limit: Option<u64>,
    pub prompt: Option<String>,
    /// Caminho do lançador da instalação (`launcher_program`); `None` = o nome no PATH.
    pub launcher: Option<String>,
}

/// O lançador da instalação que roda este servidor. O do PATH pode ser de outra versão (link que
/// ficou no checkout) e recusar os argumentos de agora, e o pane morre ao nascer. No Windows o
/// PATH tem o `.cmd` que o `install.ps1` escreve.
pub fn launcher_program(home: &Path, windows: bool) -> Option<String> {
    let path = crate::install_root::current(home).join("scripts").join(LAUNCHER);
    (!windows && path.is_file()).then(|| path.to_string_lossy().into_owned())
}

/// Pasta absoluta da conta Codex pelo id (`resolve_account`), a mesma que a resolução conferiu.
pub fn home(server_env: &BTreeMap<String, String>, account_id: &str) -> Result<String, OpenError> {
    let service = crate::accounts::AccountService::new(crate::accounts::environment::AccountEnvironment::from_map(server_env.clone()));
    let account = service.resolve(crate::accounts::Provider::Codex, account_id)
        .map_err(|error| OpenError::new(error.status, error.code, "conta Codex inválida").with_params(error.params))?;
    Ok(std::path::absolute(&account.home).unwrap_or(account.home).to_string_lossy().into_owned())
}

/// `PERMISSION_POLICIES`: modo do app → (aprovação, sandbox). Fora da lista não abre.
pub fn policy(mode: &str) -> Result<(&'static str, &'static str), OpenError> {
    crate::runtime::codex_launch::policy_of(mode).ok_or_else(|| OpenError::new(409, "erro_permissao_invalida",
        format!("modo de permissão desconhecido: {mode:?}; a sessão não foi aberta")))
}

/// `comando_do_lancador`, na ordem dele, com a política sempre explícita (matriz, item 17).
pub fn launcher_argv(launch: &Launch) -> Result<Vec<String>, OpenError> {
    let (approval, sandbox) = policy(&launch.mode)?;
    let program = launch.launcher.clone().unwrap_or_else(|| LAUNCHER.into());
    let mut argv: Vec<String> = vec![program, "--cwd".into(), launch.cwd.clone()];
    let mut flag = |name: &str, value: Option<&String>| if let Some(value) = value.filter(|v| !v.is_empty()) {
        argv.extend([name.to_owned(), value.clone()]);
    };
    flag("--service-tier", launch.service_tier.as_ref());
    flag("--codex-home", Some(&launch.codex_home));
    flag("--codex-account", Some(&launch.account));
    flag("--resume", launch.resume.as_ref());
    flag("--model", launch.model.as_ref());
    flag("--effort", launch.effort.as_ref());
    argv.extend(["--approval-policy".into(), approval.into(), "--sandbox".into(), sandbox.into()]);
    if let Some(limit) = launch.tool_output_token_limit {
        argv.extend(["--tool-output-token-limit".into(), limit.to_string()]);
    }
    if let Some(prompt) = launch.prompt.as_ref().filter(|p| !p.is_empty()) {
        argv.extend(["--prompt".into(), prompt.clone()]);
    }
    Ok(argv)
}

/// O lançador da criação: a conta resolvida e a escolha do spec; o modo do spec vale.
pub fn creation(spec: &SessionSpec, codex_home: &str, prompt: Option<&str>) -> Launch {
    let account = match &spec.account { super::Account::Codex { account_id } => account_id.clone(), _ => "default".into() };
    Launch {
        cwd: spec.cwd.clone(), codex_home: codex_home.into(), account,
        service_tier: spec.service_tier.map(|t| t.name().to_owned()), resume: None,
        model: spec.model.clone(), effort: spec.effort.clone(),
        mode: spec.permission_mode.unwrap_or(PermissionMode::FullAccess).name().into(),
        tool_output_token_limit: None, prompt: prompt.map(str::to_owned), launcher: None,
    }
}

/// O pane do Codex: o lançador (no envelope do só leitura, se houver) e o `-e` da sessão.
pub fn pane(name: &str, key: &str, launch: &Launch, jev: bool, runtime_config: &Value, read_only: Option<&[String]>, windows: bool)
    -> Result<PaneSpec, OpenError> {
    let mut command = launcher_argv(launch)?;
    let context = crate::runtime::codex_launch::context_overrides(Path::new(&launch.codex_home), runtime_config);
    command.extend(context.into_iter().flat_map(|value| ["--config".to_owned(), value]));
    if let Some(prefix) = read_only {
        command = super::terminal::sandboxed(prefix, &command, windows);
    }
    let mut env = super::claude::jev_pairs(jev, runtime_config);
    env.push(("HANGAR_CLAUDE_SETTINGS".into(), String::new()));
    Ok(PaneSpec { name: name.into(), cwd: launch.cwd.clone(), provider: AgentProvider::Codex, key: key.into(), env, config_dir: None, command })
}

/// `_exigir_lancador_codex`: sem o lançador ou o `codex` no PATH o pane morre ao nascer e o
/// `new-session` devolve sucesso. Recusa antes de criar nada.
pub fn require_launcher(server_env: &BTreeMap<String, String>, launcher: Option<&str>) -> Result<(), OpenError> {
    let missing = |program: &str| crate::runtime::process::resolve_program(program, server_env).is_none();
    if missing(launcher.unwrap_or(LAUNCHER)) {
        return Err(OpenError::new(409, "erro_criacao_sessao", format!("{LAUNCHER} nao esta no PATH deste servidor — sem ele a sessao \
            Codex nasce e morre na hora, sem erro. Instale o lancador: no Linux, scripts/install-claude-wrapper.sh; no Windows, install.ps1.")));
    }
    if missing("codex") {
        return Err(OpenError::new(409, "erro_criacao_sessao", "codex nao esta no PATH deste servidor — o lancador sobe o app-server com \
            ele, e sem isso a sessao Codex nasce e morre na hora, sem erro. Instale o CLI do Codex."));
    }
    Ok(())
}

/// O sidecar da sessão sem terminal, como o `sessions.save` do `_create_codex_headless` grava:
/// conta padrão sem `codex_account`, sem `service_tier` quando não escolhido.
pub fn headless_sidecar(spec: &SessionSpec, codex_home: &str) -> Map<String, Value> {
    let mut meta = json!({"name": spec.name, "provider": "codex", "thread_id": null, "rollout_path": "", "cwd": spec.cwd,
        "model": spec.model, "effort": spec.effort, "endpoint": null, "app_pid": null, "codex_home": codex_home, "key": spec.key,
        "permission_mode": spec.permission_mode.unwrap_or(PermissionMode::FullAccess).name(), "headless": true, "cano": null, "jev": spec.jev});
    if let super::Account::Codex { account_id } = &spec.account && account_id != "default" { meta["codex_account"] = json!(account_id); }
    if let Some(tier) = spec.service_tier { meta["service_tier"] = json!(ServiceTier::name(tier)); }
    meta.as_object().cloned().unwrap_or_default()
}

/// Onde a ponte de skills começa o bloco dela no `config.toml`: a confiança entra antes.
const MANAGED_BLOCK: &str = "# >>> hangar:";

/// `sessions.pretrust_cwd`: sem a pasta confiada a TUI nasce presa no "Do you trust…?", sem
/// thread e sem sidecar. Lê o TOML de verdade (tabela escrita de outro jeito também conta) e só
/// insere a que falta; nunca impede a abertura.
pub fn pretrust(codex_home: &Path, cwd: &str) {
    // `_pretrust_lock`: ler e regravar sem ela, duas aberturas juntas perdem a confiança de uma.
    static PRETRUST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = PRETRUST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let config = codex_home.join("config.toml");
    let result = (|| -> std::io::Result<()> {
        let raw = match std::fs::read_to_string(&config) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let parsed: toml::Table = toml::from_str(&raw).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        if parsed.get("projects").and_then(|p| p.as_table()).is_some_and(|p| p.contains_key(cwd)) { return Ok(()); }
        let entry = format!("[projects.{}]\ntrust_level = \"trusted\"\n\n", serde_json::to_string(cwd).unwrap_or_default());
        let text = match raw.find(MANAGED_BLOCK) {
            Some(at) => format!("{}{entry}{}", &raw[..at], &raw[at..]),
            None => format!("{raw}\n{entry}"),
        };
        std::fs::create_dir_all(codex_home)?;
        write_keeping_mode(&config, text.as_bytes())
    })();
    if let Err(error) = result {
        tracing::warn!(config = %config.display(), %error, "pré-confiar a pasta no Codex falhou; a TUI pergunta na abertura");
    }
}

/// `_gravar_preservando`: troca atômica que mantém o modo do arquivo de antes.
fn write_keeping_mode(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mode = std::fs::metadata(path).ok().map(|m| m.permissions());
    crate::runtime::queue::atomic_write(path, bytes)?;
    if let Some(mode) = mode { std::fs::set_permissions(path, mode)?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> Launch {
        Launch { cwd: "/w".into(), codex_home: "/h/.codex".into(), account: "default".into(), mode: "Full Access".into(), ..Launch::default() }
    }

    /// Aberturas ao mesmo tempo na mesma conta não perdem a confiança uma da outra.
    #[test]
    fn simultaneous_pretrusts_keep_every_folder() {
        let home = tempfile::tempdir().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(24));
        let threads: Vec<_> = (0..24).map(|i| {
            let (home, barrier) = (home.path().to_owned(), barrier.clone());
            std::thread::spawn(move || { barrier.wait(); pretrust(&home, &format!("/w/{i}")); })
        }).collect();
        for thread in threads { thread.join().unwrap(); }
        let parsed: toml::Table = toml::from_str(&std::fs::read_to_string(home.path().join("config.toml")).unwrap()).unwrap();
        assert_eq!(parsed["projects"].as_table().unwrap().len(), 24);
    }

    /// A ordem e os nomes do `comando_do_lancador`; a política vai sempre explícita.
    #[test]
    fn the_launcher_command_follows_the_python_order_with_an_explicit_policy() {
        assert_eq!(launcher_argv(&launch()).unwrap(), ["hangar-codex-tui", "--cwd", "/w", "--codex-home", "/h/.codex", "--codex-account", "default",
            "--approval-policy", "never", "--sandbox", "danger-full-access"]);
        let full = Launch { service_tier: Some("priority".into()), resume: Some("t-1".into()), model: Some("gpt-5.5".into()), effort: Some("high".into()),
            mode: "Ask for approval".into(), tool_output_token_limit: Some(9000), prompt: Some("começa".into()), account: "b".into(), ..launch() };
        assert_eq!(launcher_argv(&full).unwrap(), ["hangar-codex-tui", "--cwd", "/w", "--service-tier", "priority", "--codex-home", "/h/.codex",
            "--codex-account", "b", "--resume", "t-1", "--model", "gpt-5.5", "--effort", "high", "--approval-policy", "on-request",
            "--sandbox", "read-only", "--tool-output-token-limit", "9000", "--prompt", "começa"]);
        assert_eq!(policy("Approve for me").unwrap(), ("on-request", "workspace-write"));
    }

    /// O lançador do `current` vence o do PATH, que pode ser de outra versão; no Windows e sem o
    /// arquivo no `current`, o nome no PATH.
    #[test]
    fn the_launcher_comes_from_the_install_root_when_it_has_one() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(launcher_program(home.path(), false), None);
        let scripts = home.path().join(".hangar").join("current").join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join(LAUNCHER), "#!/bin/sh\n").unwrap();
        let own = scripts.join(LAUNCHER).to_string_lossy().into_owned();
        assert_eq!(launcher_program(home.path(), false), Some(own.clone()));
        assert_eq!(launcher_program(home.path(), true), None);
        let argv = launcher_argv(&Launch { launcher: Some(own.clone()), ..launch() }).unwrap();
        assert_eq!(argv[..3], [own, "--cwd".into(), "/w".into()]);
    }

    /// Modo fora da lista nunca vira Full Access.
    #[test]
    fn an_unknown_mode_refuses_instead_of_full_access() {
        for mode in ["Read Only", "", "yolo"] {
            assert_eq!(launcher_argv(&Launch { mode: mode.into(), ..launch() }).unwrap_err().code, Some("erro_permissao_invalida"), "{mode:?}");
        }
    }

    #[test]
    fn the_pane_carries_jev_and_the_read_only_envelope() {
        let pane = pane("cx", "k", &launch(), false, &json!({}), Some(&["bwrap".into(), "--".into()]), false).unwrap();
        assert_eq!(pane.command[..4], ["bwrap", "--", "/bin/sh", "-c"].map(str::to_owned));
        assert!(pane.command[4].starts_with("exec hangar-codex-tui --cwd /w"), "{:?}", pane.command);
        assert_eq!(pane.env, [("HANGAR_JEV".to_owned(), "off".to_owned()), ("HANGAR_CLAUDE_SETTINGS".to_owned(), String::new())]);
        assert_eq!((pane.provider, pane.key.as_str(), pane.config_dir), (AgentProvider::Codex, "k", None));
    }

    /// Sem nada salvo a TUI sobe com 1M; o desligado gravado no Hangar não leva nada ao lançador.
    #[test]
    fn the_pane_asks_the_launcher_for_one_million_unless_turned_off() {
        let home = tempfile::tempdir().unwrap();
        let launch = Launch { codex_home: home.path().to_string_lossy().into_owned(), ..launch() };
        let command = pane("cx", "k", &launch, false, &json!({}), None, false).unwrap().command;
        assert!(command.windows(2).any(|w| w == ["--config", "model_context_window=1000000"]), "{command:?}");
        assert!(command.windows(2).any(|w| w == ["--config", "model_auto_compact_token_limit=900000"]), "{command:?}");
        let off = pane("cx", "k", &launch, false, &json!({"codex_extended_context": false}), None, false).unwrap().command;
        assert!(!off.iter().any(|a| a == "--config"), "{off:?}");
    }

    #[cfg(unix)]
    #[test]
    fn pretrust_inserts_before_the_managed_block_once_and_keeps_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        std::fs::write(&config, "model = \"m\"\n# >>> hangar: skills\n[x]\n").unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o640)).unwrap();
        pretrust(home.path(), "/w/proj");
        let text = std::fs::read_to_string(&config).unwrap();
        assert_eq!(text, "model = \"m\"\n[projects.\"/w/proj\"]\ntrust_level = \"trusted\"\n\n# >>> hangar: skills\n[x]\n");
        assert_eq!(std::fs::metadata(&config).unwrap().permissions().mode() & 0o777, 0o640);
        pretrust(home.path(), "/w/proj");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), text, "idempotente");
        // Tabela escrita de outro jeito também é a mesma pasta: nunca redefinir.
        std::fs::write(&config, "[projects]\n\"/w/b\" = { trust_level = \"trusted\" }\n").unwrap();
        pretrust(home.path(), "/w/b");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "[projects]\n\"/w/b\" = { trust_level = \"trusted\" }\n");
        // Config quebrado não é sobrescrito e não derruba a abertura.
        std::fs::write(&config, "[[[").unwrap();
        pretrust(home.path(), "/w/c");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "[[[");
        let fresh = tempfile::tempdir().unwrap();
        pretrust(&fresh.path().join(".codex"), "/w/d");
        assert_eq!(std::fs::read_to_string(fresh.path().join(".codex/config.toml")).unwrap(), "\n[projects.\"/w/d\"]\ntrust_level = \"trusted\"\n\n");
    }
}
