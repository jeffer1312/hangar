//! O hcu (`hangar-computer-control-setup`) sem pedido: instala na subida se faltar e, de hora em hora,
//! instala de novo se falhou ou atualiza sob os portões do Hangar. Relógio próprio: o `tick_inner` da
//! atualização do Hangar sai cedo quando ele está em dia. Toda passada termina no registro do MCP.
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hangar_computer_control_setup as setup;
use serde_json::json;

use crate::config_sync::runtime::RuntimeConfig;
use crate::routes::AppState;
use crate::update::periodic::INTERVAL;

const PANICKED: &str = "computer_control_task_panicked";
const CONFIG_FAILED: &str = "computer_control_config_failed";

/// Só a subida liga; nunca segura a subida. Falha já foi ao diário pelo crate.
pub(crate) fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let diary = crate::diag::diary_dir_in(&state.accounts.env);
        guarded(diary.clone(), "computer_control.subida", pass(state.clone(), true)).await;
        loop {
            tokio::time::sleep(INTERVAL).await;
            guarded(diary.clone(), "computer_control.passada", pass(state.clone(), false)).await;
        }
    });
}

/// Cada passada na sua tarefa: um pânico vai ao diário com código e o relógio continua.
async fn guarded(diary: Option<PathBuf>, event: &'static str, work: impl Future<Output = ()> + Send + 'static) {
    if let Err(error) = tokio::spawn(work).await {
        panicked(diary.as_deref(), event, &error);
    }
}

/// A subida migra o Jev e só instala; o relógio também atualiza. O cliente http é refeito a cada
/// passada: uma falha dele não deixa o relógio sem instalar até o próximo reinício.
async fn pass(state: Arc<AppState>, startup: bool) {
    let diary = crate::diag::diary_dir_in(&state.accounts.env);
    if startup {
        // A migração do Jev antes do registro: a entrada nasce com o Jev já migrado.
        let migration = state.clone();
        let migrated = tokio::task::spawn_blocking(move || match RuntimeConfig::from_state(&migration) {
            Ok(runtime) => runtime.startup_jev_migration(),
            Err(error) => crate::config_sync::jev_startup::report(&migration.accounts.env, "configuracao", &error),
        })
        .await;
        if let Err(error) = migrated {
            panicked(diary.as_deref(), "computer_control.migracao_jev", &error);
        }
    }
    let paths = setup::Paths::new(state.accounts.env.home.clone(), Vec::new());
    let outcome = match crate::http_client().build() {
        Ok(http) => {
            let github = setup::Github::default();
            match setup::install_if_missing(&paths, &http, &github, diary.as_deref()).await {
                Ok(setup::Swapped::No) if !startup => {
                    setup::update_tick(&paths, &http, &github, &policy(&state, diary.as_deref()), diary.as_deref()).await
                }
                other => other,
            }
        }
        // Sem cliente http não instala nem atualiza, mas o registro segue.
        Err(error) => {
            setup::client_failed(&paths, &error.to_string(), diary.as_deref());
            Ok(setup::Swapped::No)
        }
    };
    swapped(&state, outcome).await;
}

/// Toda passada da subida e do relógio passa aqui, com binário novo ou não: o registro roda em todas
/// (o servidor pode subir antes do login gráfico) e refaz a entrada depois de cada troca.
async fn swapped(state: &Arc<AppState>, outcome: Result<setup::Swapped, setup::Error>) {
    if let Ok(setup::Swapped::Yes { tag }) = outcome {
        tracing::info!(tag = %tag, "Computer Use: binário do pacote em {tag}");
    }
    let diary = crate::diag::diary_dir_in(&state.accounts.env);
    let state = state.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || register(&state)).await {
        panicked(diary.as_deref(), "computer_control.registrar", &error);
    }
}

fn panicked(diary: Option<&Path>, event: &str, error: &tokio::task::JoinError) {
    tracing::error!(code = PANICKED, event, %error, "Computer Use: a tarefa caiu");
    let line = json!({"evento": event, "nivel": "erro", "codigo": PANICKED, "detalhe": error.to_string()});
    crate::diag::record_blocking(diary, &[line], "computer control");
}

/// O registro sozinho com as contas do catálogo. Falha vai ao diário, nunca à tela.
fn register(state: &AppState) {
    match crate::computer_control::paths(state) {
        Ok(paths) => register_in(state, &paths),
        Err(error) => {
            tracing::warn!(code = "computer_control_register_failed", msg = %error.msg, "Computer Use: sem a lista de contas");
            let line = json!({"evento": "computer_control.registrar", "nivel": "erro",
                "codigo": "computer_control_register_failed", "erro": error.code, "detalhe": error.msg});
            crate::diag::record_blocking(crate::diag::diary_dir_in(&state.accounts.env).as_deref(), &[line], "computer control");
        }
    }
}

/// Com os caminhos já resolvidos (a rota de alvos os tem); a falha o crate já mandou ao diário.
pub(crate) fn register_in(state: &AppState, paths: &setup::Paths) {
    let diary = crate::diag::diary_dir_in(&state.accounts.env);
    // O env do Jev relido sob a trava, como no PUT.
    let jev = || -> Result<_, setup::Error> { Ok(RuntimeConfig::from_state(state).map_err(setup::Error::io)?.jev_env()) };
    if let Ok(setup::Registered::Written) = setup::register(paths, setup::hyprland_found, jev, diary.as_deref()) {
        tracing::info!("Computer Use: MCP registrado");
    }
}

/// Os portões da atualização automática do Hangar, lidos a cada tique.
fn policy(state: &AppState, diary: Option<&Path>) -> setup::Policy {
    let home = &state.accounts.env.home;
    let package_channel = matches!(crate::update::http::mode(home), crate::update::http::Mode::Package(_))
        .then(|| crate::update::channel(&crate::update::Layout::new(home)));
    let automations = automations(RuntimeConfig::from_state(state), diary);
    setup::Policy { automations, package_channel }
}

/// Configuração ilegível não atualiza, e vai ao diário: o hcu parado de atualizar fica com causa.
fn automations(runtime: Result<RuntimeConfig, String>, diary: Option<&Path>) -> bool {
    match runtime {
        Ok(runtime) => {
            let value = runtime.get("automations");
            let enabled = crate::orchestration::truthy(&value);
            crate::orchestration::codec::discard(value);
            enabled
        }
        Err(error) => {
            tracing::warn!(code = CONFIG_FAILED, %error, "Computer Use: sem configuração; não atualiza");
            let line = json!({"evento": "computer_control.atualizar", "nivel": "erro", "codigo": CONFIG_FAILED, "detalhe": error});
            crate::diag::record_blocking(diary, &[line], "computer control");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};

    use serde_json::Value;

    use super::*;
    use crate::accounts::{AccountService, environment::AccountEnvironment};
    use crate::{auth::TrustedHosts, config::Config};

    fn state(home: &std::path::Path) -> Arc<AppState> {
        let address = "127.0.0.1:0".parse().unwrap();
        let mut state = AppState::new(Config { listen: address, upstream: address, internal_secret: "s".into(),
            auth_token: "owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
        state.accounts = AccountService::new(AccountEnvironment::from_map(BTreeMap::from([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
        ])));
        Arc::new(state)
    }

    fn diary_text(dir: &Path) -> String {
        std::fs::read_dir(dir).unwrap().flatten().map(|file| std::fs::read_to_string(file.path()).unwrap()).collect()
    }

    /// R3 → R4: o binário trocado passa pelo registro, que liga o MCP com o alvo cadastrado.
    #[tokio::test]
    async fn a_swapped_binary_registers_the_mcp() {
        let home = tempfile::tempdir().unwrap();
        let paths = setup::Paths::new(home.path().to_owned(), Vec::new());
        std::fs::create_dir_all(paths.targets_dir()).unwrap();
        std::fs::write(paths.binary(), b"\x7fELF").unwrap();
        std::fs::write(paths.install_json(), br#"{"tag": "v2.0.0"}"#).unwrap();
        let target = paths.targets_dir().join("vm-agent.json");
        std::fs::write(&target, br#"{"transport": "ssh", "host": "vm"}"#).unwrap();
        swapped(&state(home.path()), Ok(setup::Swapped::Yes { tag: "v2.0.0".into() })).await;
        let data: Value = serde_json::from_str(&std::fs::read_to_string(paths.main_file()).unwrap()).unwrap();
        let entry = &data["mcpServers"][setup::NAME];
        assert_eq!(entry["command"], json!(paths.binary().to_string_lossy()));
        assert_eq!(entry["env"]["HCC_AGENT_CONFIG"], json!(target.to_string_lossy()));
    }

    /// Sem binário a passada não grava nada.
    #[tokio::test]
    async fn without_the_binary_the_pass_writes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let paths = setup::Paths::new(home.path().to_owned(), Vec::new());
        std::fs::create_dir_all(paths.targets_dir()).unwrap();
        std::fs::write(paths.targets_dir().join("vm-agent.json"), b"{}").unwrap();
        swapped(&state(home.path()), Ok(setup::Swapped::No)).await;
        assert!(!paths.main_file().exists());
    }

    /// Uma passada que entra em pânico vai ao diário com código, e a seguinte ainda roda.
    #[tokio::test]
    async fn a_panicking_pass_goes_to_the_diary_and_the_next_one_runs() {
        let diary = tempfile::tempdir().unwrap();
        guarded(Some(diary.path().to_owned()), "computer_control.passada", async { panic!("de propósito"); }).await;
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        guarded(Some(diary.path().to_owned()), "computer_control.passada", async move { flag.store(true, Ordering::SeqCst) }).await;
        assert!(ran.load(Ordering::SeqCst));
        assert!(diary_text(diary.path()).contains(PANICKED), "{}", diary_text(diary.path()));
    }

    /// Configuração ilegível: não atualiza e diz por quê no diário.
    #[test]
    fn an_unreadable_config_does_not_update_and_goes_to_the_diary() {
        let diary = tempfile::tempdir().unwrap();
        assert!(!automations(Err("ilegível".into()), Some(diary.path())));
        assert!(diary_text(diary.path()).contains(CONFIG_FAILED), "{}", diary_text(diary.path()));
    }
}
