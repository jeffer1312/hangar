//! Operações pela CLI `claude plugin`. Nenhuma relê o estado depois de mudar: quem chama faz um
//! [`Plugins::state`] em seguida, porque a releitura precisa do `settings.json` que o servidor lê.
use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::cli::{AVAILABLE, CliError, CliOutput, CliRunner, DOWNLOAD, SHORT};
use crate::details;
use crate::disk::{CliInstalled, read_state};
use crate::model::{
    AvailablePlugin, InputError, MarketplaceName, MarketplaceSource, OperationResult, PluginDetails, PluginId,
    PluginsState,
};

use crate::disk::BUILTIN;

/// Um erro por código do contrato HTTP; `message` vai em `params.message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginsError {
    Input(InputError),
    NotFound(String),
    UnknownMarketplace(String),
    BuiltinLocked,
    CliMissing,
    CliFailed(String),
    CliTimeout,
    /// A instalação pediu `--accept-command`; leva o comando para a tela mostrar.
    NeedsReview(String),
}

pub struct Plugins<R> {
    runner: R,
}

#[derive(Deserialize)]
struct Listing {
    available: Vec<CliAvailable>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliAvailable {
    plugin_id: String,
    name: String,
    marketplace_name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

impl PluginsError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Input(error) => error.code(),
            Self::NotFound(_) => "plugins_not_found",
            Self::UnknownMarketplace(_) => "plugins_unknown_marketplace",
            Self::BuiltinLocked => "plugins_builtin_locked",
            Self::CliMissing => "plugins_cli_missing",
            Self::CliFailed(_) => "plugins_cli_failed",
            Self::CliTimeout => "plugins_cli_timeout",
            Self::NeedsReview(_) => "plugins_command_needs_review",
        }
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Self::NotFound(text) | Self::UnknownMarketplace(text) | Self::CliFailed(text) | Self::NeedsReview(text) => {
                Some(text)
            }
            Self::Input(_) | Self::BuiltinLocked | Self::CliMissing | Self::CliTimeout => None,
        }
    }
}

impl<R: CliRunner> Plugins<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    /// `plugin list --json` junto ao disco da pasta principal; `checked_at` sai `None` (é do servidor).
    ///
    /// # Errors
    ///
    /// Falha da CLI ou lista ilegível.
    pub async fn state(&self, main_dir: &Path, settings: &Value) -> Result<PluginsState, PluginsError> {
        let output = self.runner.run(&["plugin", "list", "--json"], SHORT).await?;
        if !output.success {
            return Err(PluginsError::CliFailed(failure_text(&output)));
        }
        let installed: Vec<CliInstalled> = serde_json::from_str(output.stdout.trim())
            .map_err(|error| PluginsError::CliFailed(format!("plugin list --json: {error}")))?;
        Ok(read_state(main_dir, settings, &installed))
    }

    /// Atualiza todos os marketplaces. Sem nome a CLI recusa `--json`, então vale o código de saída.
    /// Quem guarda `checked_at` é o servidor, depois do `Ok`.
    ///
    /// # Errors
    ///
    /// Falha da CLI, com a stderr.
    pub async fn check(&self) -> Result<(), PluginsError> {
        let output = self.runner.run(&["plugin", "marketplace", "update"], DOWNLOAD).await?;
        if output.success { Ok(()) } else { Err(PluginsError::CliFailed(failure_text(&output))) }
    }

    /// # Errors
    ///
    /// Falha da CLI ou lista ilegível.
    pub async fn available(&self) -> Result<Vec<AvailablePlugin>, PluginsError> {
        let output = self.runner.run(&["plugin", "list", "--json", "--available"], AVAILABLE).await?;
        if !output.success {
            return Err(PluginsError::CliFailed(failure_text(&output)));
        }
        let listing: Listing = serde_json::from_str(output.stdout.trim())
            .map_err(|error| PluginsError::CliFailed(format!("plugin list --json --available: {error}")))?;
        Ok(listing
            .available
            .into_iter()
            .map(|entry| AvailablePlugin {
                id: entry.plugin_id,
                name: entry.name,
                marketplace: entry.marketplace_name,
                version: entry.version,
                description: entry.description,
            })
            .collect())
    }

    /// # Errors
    ///
    /// [`PluginsError::NotFound`] para plugin não instalado; falha da CLI.
    pub async fn details(&self, id: &PluginId) -> Result<PluginDetails, PluginsError> {
        let output = self.runner.run(&["plugin", "details", "--", id.as_str()], SHORT).await?;
        if output.success {
            return Ok(details::parse(id, &output.stdout));
        }
        // `details` não tem `--json`: o "não instalado" só aparece no texto.
        let text = failure_text(&output);
        Err(if text.contains("not found") { PluginsError::NotFound(text) } else { PluginsError::CliFailed(text) })
    }

    /// # Errors
    ///
    /// Ver [`Plugins::update`].
    pub async fn install(&self, id: &PluginId) -> Result<OperationResult, PluginsError> {
        self.plugin_op("install", id, DOWNLOAD).await
    }

    /// # Errors
    ///
    /// [`PluginsError::NotFound`], [`PluginsError::NeedsReview`] ou falha da CLI, lidos da última
    /// linha JSON da stdout.
    pub async fn update(&self, id: &PluginId) -> Result<OperationResult, PluginsError> {
        self.plugin_op("update", id, DOWNLOAD).await
    }

    /// # Errors
    ///
    /// Ver [`Plugins::update`].
    pub async fn enable(&self, id: &PluginId) -> Result<OperationResult, PluginsError> {
        self.plugin_op("enable", id, SHORT).await
    }

    /// # Errors
    ///
    /// Ver [`Plugins::update`].
    pub async fn disable(&self, id: &PluginId) -> Result<OperationResult, PluginsError> {
        self.plugin_op("disable", id, SHORT).await
    }

    /// # Errors
    ///
    /// Ver [`Plugins::update`].
    pub async fn uninstall(&self, id: &PluginId) -> Result<OperationResult, PluginsError> {
        self.plugin_op("uninstall", id, SHORT).await
    }

    /// # Errors
    ///
    /// Ver [`Plugins::update`].
    pub async fn marketplace_add(&self, source: &MarketplaceSource) -> Result<OperationResult, PluginsError> {
        self.mutate(&["plugin", "marketplace", "add", "--json", "--", source.as_str()], DOWNLOAD).await
    }

    /// # Errors
    ///
    /// [`PluginsError::BuiltinLocked`] no embutido; [`PluginsError::UnknownMarketplace`]; falha da CLI.
    pub async fn marketplace_update(&self, name: &MarketplaceName) -> Result<OperationResult, PluginsError> {
        self.marketplace_op("update", name, DOWNLOAD).await
    }

    /// # Errors
    ///
    /// Ver [`Plugins::marketplace_update`].
    pub async fn marketplace_remove(&self, name: &MarketplaceName) -> Result<OperationResult, PluginsError> {
        self.marketplace_op("remove", name, SHORT).await
    }

    async fn plugin_op(&self, op: &str, id: &PluginId, timeout: Duration) -> Result<OperationResult, PluginsError> {
        self.mutate(&["plugin", op, "-s", "user", "--json", "--", id.as_str()], timeout).await
    }

    async fn marketplace_op(
        &self,
        op: &str,
        name: &MarketplaceName,
        timeout: Duration,
    ) -> Result<OperationResult, PluginsError> {
        if name.as_str() == BUILTIN {
            return Err(PluginsError::BuiltinLocked);
        }
        self.mutate(&["plugin", "marketplace", op, "--json", "--", name.as_str()], timeout).await
    }

    async fn mutate(&self, args: &[&str], timeout: Duration) -> Result<OperationResult, PluginsError> {
        let output = self.runner.run(args, timeout).await?;
        operation_result(&output)
    }
}

impl From<InputError> for PluginsError {
    fn from(error: InputError) -> Self {
        Self::Input(error)
    }
}

impl From<CliError> for PluginsError {
    fn from(error: CliError) -> Self {
        match error {
            CliError::Missing => Self::CliMissing,
            CliError::Timeout => Self::CliTimeout,
            CliError::Spawn(text) => Self::CliFailed(text),
        }
    }
}

impl fmt::Display for PluginsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.message() {
            Some(message) => write!(f, "{}: {message}", self.code()),
            None => f.write_str(self.code()),
        }
    }
}

impl std::error::Error for PluginsError {}

/// A CLI imprime o resultado na última linha JSON da stdout, com o mesmo código de saída.
fn operation_result(output: &CliOutput) -> Result<OperationResult, PluginsError> {
    let Some(line) = last_json_line(&output.stdout) else {
        return Err(PluginsError::CliFailed(failure_text(output)));
    };
    if let Some(shown) = line.get("shownCommand").filter(|shown| !shown.is_null()) {
        let command = shown.get("command").and_then(Value::as_str).map_or_else(|| shown.to_string(), str::to_owned);
        return Err(PluginsError::NeedsReview(command));
    }
    let message = text(&line, "message").filter(|message| !message.is_empty()).unwrap_or_else(|| failure_text(output));
    // Ligar o que já está ligado vem como falha da CLI; para a tela, com estado de um instante atrás, é sucesso.
    let already = line.get("failureCode").and_then(Value::as_str) == Some("already_in_goal_state");
    if already || line.get("outcome").and_then(Value::as_str) == Some("ok") {
        return Ok(OperationResult {
            outcome: text(&line, "updateOutcome").unwrap_or_else(|| "ok".to_owned()),
            message,
            old_version: text(&line, "oldVersion"),
            new_version: text(&line, "newVersion"),
        });
    }
    Err(match line.get("failureCode").and_then(Value::as_str) {
        Some("not_found" | "not_installed" | "not_installed_at_scope") => PluginsError::NotFound(message),
        Some("not_configured") => PluginsError::UnknownMarketplace(message),
        _ => PluginsError::CliFailed(message),
    })
}

fn last_json_line(stdout: &str) -> Option<Value> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .find_map(|line| serde_json::from_str(line).ok())
}

/// Stderr sem o `✘` da CLI; vazia, a stdout.
fn failure_text(output: &CliOutput) -> String {
    let stderr = output.stderr.trim();
    let stderr = stderr.strip_prefix('✘').map_or(stderr, str::trim_start);
    if stderr.is_empty() { output.stdout.trim().to_owned() } else { stderr.to_owned() }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::future::{Future, ready};
    use std::pin::pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Waker};

    use super::*;

    /// Saídas reais da CLI 2.1.319 (pasta de config descartável, 09/11/2026).
    const INSTALL_OK: &str = r#"{"command":"install","outcome":"ok","plugin":"caveman@caveman","pluginId":"caveman@caveman","scope":"user","message":"Successfully installed plugin: caveman@caveman (scope: user)"}"#;
    const UPDATE_UP_TO_DATE: &str = r#"{"command":"update","outcome":"ok","plugin":"caveman@caveman","pluginId":"caveman@caveman","scope":"user","message":"caveman is already at the latest version (4.1.0).","updateOutcome":"up_to_date","oldVersion":"4.1.0","newVersion":"4.1.0"}"#;
    const UPDATE_UPDATED: &str = r#"{"command":"update","outcome":"ok","plugin":"caveman@caveman","pluginId":"caveman@caveman","scope":"user","message":"Plugin \"caveman\" updated from 1.0.0 to 1.1.0","updateOutcome":"updated","oldVersion":"1.0.0","newVersion":"1.1.0"}"#;
    const INSTALL_NOT_FOUND: &str = r#"{"command":"install","outcome":"failed","plugin":"comver@teste-mkt","scope":"user","message":"Plugin \"comver\" not found in marketplace \"teste-mkt\".","failureCode":"not_found"}"#;
    const UNINSTALL_NOT_INSTALLED: &str = r#"{"command":"uninstall","outcome":"failed","plugin":"caveman@caveman","scope":"user","message":"Plugin \"caveman@caveman\" not found in installed plugins","failureCode":"not_installed"}"#;
    const DISABLE_ALREADY: &str = r#"{"command":"disable","outcome":"failed","plugin":"caveman@caveman","scope":"user","message":"Plugin \"caveman@caveman\" is already disabled at user scope","failureCode":"already_in_goal_state","alreadyInGoalState":true}"#;
    const MARKETPLACE_ADD_OK: &str = r#"{"command":"marketplace-add","outcome":"ok","marketplace":"caveman","message":"SSH not configured, cloning via HTTPS: https://github.com/JuliusBrussee/caveman.git\nSuccessfully added marketplace: caveman (declared in user settings)"}"#;
    const MARKETPLACE_UPDATE_OK: &str = r#"{"command":"marketplace-update","outcome":"ok","marketplace":"caveman","message":"Refreshing marketplace cache (timeout: 120s)…\nSuccessfully updated marketplace: caveman"}"#;
    const MARKETPLACE_REMOVE_OK: &str = r#"{"command":"marketplace-remove","outcome":"ok","marketplace":"caveman","message":"Successfully removed marketplace: caveman"}"#;
    const MARKETPLACE_REMOVE_MISSING: &str = r#"{"command":"marketplace-remove","marketplace":"caveman","outcome":"failed","message":"Marketplace 'caveman' not found","failureCode":"not_configured"}"#;
    /// Forma tirada do `mrt`/`sMo` do pacote 2.1.319; não reproduzida (precisa de fonte `command`).
    const INSTALL_SHOWN_COMMAND: &str = r#"{"command":"install","outcome":"failed","plugin":"x@y","scope":"user","message":"needs review","shownCommand":{"kind":"command_source","pluginId":"x@y","command":"npx -y gerar-plugin","mode":"build","catalogRevision":"r1","sha256":"abc"}}"#;
    const LIST: &str = r#"[
  {"id": "caveman@caveman", "version": "4.1.0", "scope": "user", "enabled": true,
   "installPath": "/home/u/.claude/plugins/cache/caveman/caveman/4.1.0",
   "installedAt": "2026-04-23T13:07:40.388Z", "lastUpdated": "2026-11-07T04:43:45.891Z",
   "notes": ["The packages it lists were not installed, because it uses a yarn or pnpm lockfile, which plugin installs do not support"],
   "noteDetails": [{"type": "dependencies-refused", "plugin": "caveman"}], "projectEnabled": true},
  {"id": "caveman@caveman", "version": "4.1.0", "scope": "project", "enabled": true,
   "installPath": "/home/u/.claude/plugins/cache/caveman/caveman/4.1.0",
   "installedAt": "2026-08-22T12:12:02.261Z", "lastUpdated": "2026-11-07T14:19:19.203Z",
   "projectPath": "/home/u", "projectEnabled": true},
  {"id": "hangar@inline", "version": "0.1.0", "scope": "session", "enabled": true,
   "installPath": "/home/u/.hangar/current/plugins/hangar"}
]"#;
    const AVAILABLE_LIST: &str = r#"{
  "installed": [{"id": "hangar@inline", "version": "0.1.0", "scope": "session", "enabled": true}],
  "available": [
    {"pluginId": "superpowers@anthropic-plugin-directory", "name": "superpowers",
     "description": "Core skills library for Claude Code: TDD, debugging, collaboration patterns, and proven techniques",
     "marketplaceName": "anthropic-plugin-directory", "version": "6.4.2",
     "source": {"source": "url", "url": "https://github.com/obra/superpowers.git", "sha": "8ca22dba9a94f28898bbce59f2537ff4d87c747d"}},
    {"pluginId": "frontend-design@anthropic-plugin-directory", "name": "frontend-design",
     "marketplaceName": "anthropic-plugin-directory",
     "source": {"source": "git-subdir", "url": "https://github.com/anthropics/claude-plugins-official", "path": "plugins/frontend-design"}}
  ]
}"#;

    type Call = (Vec<String>, Duration);

    struct Fake {
        calls: Mutex<Vec<Call>>,
        replies: Mutex<VecDeque<Result<CliOutput, CliError>>>,
    }

    impl CliRunner for Fake {
        fn run(&self, args: &[&str], timeout: Duration) -> impl Future<Output = Result<CliOutput, CliError>> + Send {
            self.calls.lock().expect("lock").push((args.iter().map(|arg| (*arg).to_owned()).collect(), timeout));
            ready(self.replies.lock().expect("lock").pop_front().expect("resposta preparada"))
        }
    }

    fn plugins(replies: Vec<Result<CliOutput, CliError>>) -> Plugins<Fake> {
        Plugins::new(Fake { calls: Mutex::new(Vec::new()), replies: Mutex::new(replies.into()) })
    }

    fn out(success: bool, stdout: &str, stderr: &str) -> Result<CliOutput, CliError> {
        Ok(CliOutput { success, stdout: stdout.to_owned(), stderr: stderr.to_owned() })
    }

    fn calls(plugins: &Plugins<Fake>) -> Vec<Call> {
        plugins.runner.calls.lock().expect("lock").clone()
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    /// O falso responde na hora; basta um poll sem runtime.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
    }

    fn caveman() -> PluginId {
        PluginId::parse("caveman@caveman").expect("id real")
    }

    fn name(raw: &str) -> MarketplaceName {
        MarketplaceName::parse(raw).expect("nome real")
    }

    #[test]
    fn install_ok_sends_user_scope_json_and_separator() {
        let plugins = plugins(vec![out(true, &format!("aviso qualquer\n{INSTALL_OK}\n"), "")]);
        let result = block_on(plugins.install(&caveman())).expect("ok");
        assert_eq!(
            result,
            OperationResult {
                outcome: "ok".into(),
                message: "Successfully installed plugin: caveman@caveman (scope: user)".into(),
                old_version: None,
                new_version: None,
            }
        );
        assert_eq!(
            calls(&plugins),
            vec![(argv(&["plugin", "install", "-s", "user", "--json", "--", "caveman@caveman"]), DOWNLOAD)]
        );
    }

    #[test]
    fn update_maps_update_outcome_and_versions() {
        let plugins = plugins(vec![out(true, UPDATE_UP_TO_DATE, ""), out(true, UPDATE_UPDATED, "")]);
        let same = block_on(plugins.update(&caveman())).expect("up_to_date");
        assert_eq!((same.outcome.as_str(), same.old_version.as_deref(), same.new_version.as_deref()), ("up_to_date", Some("4.1.0"), Some("4.1.0")));
        let newer = block_on(plugins.update(&caveman())).expect("updated");
        assert_eq!((newer.outcome.as_str(), newer.old_version.as_deref(), newer.new_version.as_deref()), ("updated", Some("1.0.0"), Some("1.1.0")));
        assert_eq!(calls(&plugins)[0], (argv(&["plugin", "update", "-s", "user", "--json", "--", "caveman@caveman"]), DOWNLOAD));
    }

    #[test]
    fn short_plugin_ops_use_short_timeout() {
        let ok = r#"{"command":"x","outcome":"ok","message":"m"}"#;
        let plugins = plugins(vec![out(true, ok, ""), out(true, ok, ""), out(true, ok, "")]);
        block_on(plugins.enable(&caveman())).expect("enable");
        block_on(plugins.disable(&caveman())).expect("disable");
        block_on(plugins.uninstall(&caveman())).expect("uninstall");
        let ops: Vec<Call> = ["enable", "disable", "uninstall"]
            .iter()
            .map(|op| (argv(&["plugin", op, "-s", "user", "--json", "--", "caveman@caveman"]), SHORT))
            .collect();
        assert_eq!(calls(&plugins), ops);
    }

    #[test]
    fn failed_not_found_and_not_installed_are_not_found() {
        let plugins = plugins(vec![
            out(false, INSTALL_NOT_FOUND, "✘ Failed to install plugin \"comver@teste-mkt\": Plugin \"comver\" not found"),
            out(false, UNINSTALL_NOT_INSTALLED, "✘ Failed to uninstall plugin"),
        ]);
        let missing = PluginId::parse("comver@teste-mkt").expect("id");
        let error = block_on(plugins.install(&missing)).expect_err("not_found");
        assert_eq!(error, PluginsError::NotFound("Plugin \"comver\" not found in marketplace \"teste-mkt\".".into()));
        assert_eq!(error.code(), "plugins_not_found");
        let error = block_on(plugins.uninstall(&caveman())).expect_err("not_installed");
        assert_eq!(error.code(), "plugins_not_found");
    }

    #[test]
    fn already_in_goal_state_is_success() {
        let plugins = plugins(vec![out(false, DISABLE_ALREADY, "✘ Failed to disable plugin")]);
        let result = block_on(plugins.disable(&caveman())).expect("tela velha pedindo o que já vale");
        assert_eq!(result.outcome, "ok");
    }

    #[test]
    fn other_failure_is_cli_failed_with_message() {
        let failed = r#"{"command":"install","outcome":"failed","plugin":"caveman@caveman","scope":"user","message":"Failed to clone repository","failureCode":"clone_failed"}"#;
        let plugins = plugins(vec![out(false, failed, "✘ Failed to install plugin")]);
        let error = block_on(plugins.install(&caveman())).expect_err("failed");
        assert_eq!(error, PluginsError::CliFailed("Failed to clone repository".into()));
        assert_eq!(error.message(), Some("Failed to clone repository"));
    }

    #[test]
    fn shown_command_needs_review() {
        let plugins = plugins(vec![out(false, INSTALL_SHOWN_COMMAND, "")]);
        let error = block_on(plugins.install(&PluginId::parse("x@y").expect("id"))).expect_err("review");
        assert_eq!(error, PluginsError::NeedsReview("npx -y gerar-plugin".into()));
        assert_eq!(error.code(), "plugins_command_needs_review");
    }

    #[test]
    fn no_json_line_uses_stderr_without_mark_then_stdout() {
        let plugins = plugins(vec![out(false, "", "✘ --json needs a marketplace name\n"), out(false, "saída solta\n", "")]);
        let error = block_on(plugins.enable(&caveman())).expect_err("stderr");
        assert_eq!(error, PluginsError::CliFailed("--json needs a marketplace name".into()));
        let error = block_on(plugins.enable(&caveman())).expect_err("stdout");
        assert_eq!(error, PluginsError::CliFailed("saída solta".into()));
    }

    #[test]
    fn runner_errors_map_to_contract_codes() {
        let plugins = plugins(vec![Err(CliError::Missing), Err(CliError::Timeout), Err(CliError::Spawn("EACCES".into()))]);
        assert_eq!(block_on(plugins.enable(&caveman())).expect_err("missing").code(), "plugins_cli_missing");
        assert_eq!(block_on(plugins.enable(&caveman())).expect_err("timeout").code(), "plugins_cli_timeout");
        assert_eq!(block_on(plugins.enable(&caveman())).expect_err("spawn"), PluginsError::CliFailed("EACCES".into()));
    }

    #[test]
    fn marketplace_ops_send_json_and_separator() {
        let plugins = plugins(vec![
            out(true, MARKETPLACE_ADD_OK, ""),
            out(true, MARKETPLACE_UPDATE_OK, ""),
            out(true, MARKETPLACE_REMOVE_OK, ""),
        ]);
        let source = MarketplaceSource::parse("JuliusBrussee/caveman").expect("fonte");
        let added = block_on(plugins.marketplace_add(&source)).expect("add");
        assert!(added.message.ends_with("Successfully added marketplace: caveman (declared in user settings)"));
        block_on(plugins.marketplace_update(&name("caveman"))).expect("update");
        block_on(plugins.marketplace_remove(&name("caveman"))).expect("remove");
        assert_eq!(
            calls(&plugins),
            vec![
                (argv(&["plugin", "marketplace", "add", "--json", "--", "JuliusBrussee/caveman"]), DOWNLOAD),
                (argv(&["plugin", "marketplace", "update", "--json", "--", "caveman"]), DOWNLOAD),
                (argv(&["plugin", "marketplace", "remove", "--json", "--", "caveman"]), SHORT),
            ]
        );
    }

    #[test]
    fn missing_marketplace_is_unknown_marketplace() {
        let plugins = plugins(vec![out(false, MARKETPLACE_REMOVE_MISSING, "✘ Failed to remove marketplace")]);
        let error = block_on(plugins.marketplace_remove(&name("caveman"))).expect_err("not_configured");
        assert_eq!(error, PluginsError::UnknownMarketplace("Marketplace 'caveman' not found".into()));
        assert_eq!(error.code(), "plugins_unknown_marketplace");
    }

    #[test]
    fn builtin_marketplace_is_locked_before_cli() {
        let plugins = plugins(Vec::new());
        let builtin = name("anthropic-plugin-directory");
        assert_eq!(block_on(plugins.marketplace_update(&builtin)), Err(PluginsError::BuiltinLocked));
        assert_eq!(block_on(plugins.marketplace_remove(&builtin)), Err(PluginsError::BuiltinLocked));
        assert!(calls(&plugins).is_empty());
    }

    #[test]
    fn check_updates_all_marketplaces_without_json() {
        let plugins = plugins(vec![
            out(true, "Updating marketplaces...✔ Successfully updated 1 marketplace\n", ""),
            out(false, "", "✘ Failed to update marketplace(s): rede fora\n"),
        ]);
        assert_eq!(block_on(plugins.check()), Ok(()));
        assert_eq!(block_on(plugins.check()), Err(PluginsError::CliFailed("Failed to update marketplace(s): rede fora".into())));
        assert_eq!(calls(&plugins)[0], (argv(&["plugin", "marketplace", "update"]), DOWNLOAD));
    }

    #[test]
    fn available_maps_cli_entries() {
        let plugins = plugins(vec![out(true, AVAILABLE_LIST, "")]);
        let available = block_on(plugins.available()).expect("lista");
        assert_eq!(
            available,
            vec![
                AvailablePlugin {
                    id: "superpowers@anthropic-plugin-directory".into(),
                    name: "superpowers".into(),
                    marketplace: "anthropic-plugin-directory".into(),
                    version: Some("6.4.2".into()),
                    description: Some("Core skills library for Claude Code: TDD, debugging, collaboration patterns, and proven techniques".into()),
                },
                AvailablePlugin {
                    id: "frontend-design@anthropic-plugin-directory".into(),
                    name: "frontend-design".into(),
                    marketplace: "anthropic-plugin-directory".into(),
                    version: None,
                    description: None,
                },
            ]
        );
        assert_eq!(calls(&plugins), vec![(argv(&["plugin", "list", "--json", "--available"]), AVAILABLE)]);
    }

    #[test]
    fn state_reads_cli_list_and_keeps_user_scope() {
        let dir = tempfile::tempdir().expect("pasta temporária");
        let plugins = plugins(vec![out(true, LIST, ""), out(false, "", "✘ quebrou")]);
        let state = block_on(plugins.state(dir.path(), &serde_json::json!({}))).expect("estado");
        assert_eq!(state.plugins.len(), 1);
        assert_eq!((state.plugins[0].id.as_str(), state.plugins[0].last_updated.as_str()), ("caveman@caveman", "2026-11-07T04:43:45.891Z"));
        assert_eq!(state.checked_at, None);
        assert_eq!(calls(&plugins), vec![(argv(&["plugin", "list", "--json"]), SHORT)]);
        assert_eq!(block_on(plugins.state(dir.path(), &serde_json::json!({}))), Err(PluginsError::CliFailed("quebrou".into())));
    }

    #[test]
    fn details_runs_cli_and_maps_not_found() {
        let not_found = "Plugin \"nada@caveman\" not found. Run `claude plugin list` to see installed plugins, or pass --plugin-dir <path> to load one from disk.\n";
        let plugins = plugins(vec![out(true, "caveman 4.1.0\n  Skills (23)  a\n  Always-on:   ~1,991 tok\n", ""), out(false, not_found, "")]);
        let details = block_on(plugins.details(&caveman())).expect("detalhe");
        assert_eq!((details.inventory.skills, details.always_on_tokens), (Some(23), Some(1991)));
        let missing = PluginId::parse("nada@caveman").expect("id");
        assert_eq!(block_on(plugins.details(&missing)).expect_err("ausente").code(), "plugins_not_found");
        assert_eq!(calls(&plugins)[0], (argv(&["plugin", "details", "--", "caveman@caveman"]), SHORT));
    }
}
