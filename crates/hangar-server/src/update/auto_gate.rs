//! Os portões e o relógio dos componentes que o Hangar atualiza sozinho (hcu, WhatsApp): os mesmos portões
//! do automático do Hangar, lidos a cada passada, e uma passada por hora que um pânico não para.
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hangar_computer_control_setup::Policy;
use serde_json::json;

use super::periodic::INTERVAL;
use crate::config_sync::runtime::RuntimeConfig;
use crate::routes::AppState;

/// De quem é a passada: os códigos e o rótulo que o diário leva quando a configuração não lê ou a tarefa cai.
pub(crate) struct Gate {
    /// Evento da configuração ilegível.
    pub event: &'static str,
    pub config_failed: &'static str,
    pub panicked: &'static str,
    pub what: &'static str,
}

/// Os portões da atualização automática do Hangar: automações e canal do modo pacote (`None` fora dele).
pub(crate) fn policy(state: &AppState, gate: &Gate, diary: Option<&Path>) -> Policy {
    let home = &state.accounts.env.home;
    let package_channel = matches!(super::http::mode(home), super::http::Mode::Package(_))
        .then(|| super::channel(&super::Layout::new(home)));
    let automations = automations(RuntimeConfig::from_state(state), gate, diary);
    Policy { automations, package_channel }
}

/// Configuração ilegível não atualiza, e vai ao diário: o componente parado de atualizar fica com causa.
/// O arquivo que existe e não se lê também conta: lido como vazio, ligaria as automações que ele desliga.
fn automations(runtime: Result<RuntimeConfig, String>, gate: &Gate, diary: Option<&Path>) -> bool {
    match runtime.and_then(|runtime| runtime.get_checked("automations").map_err(|e| format!("{}: {}", e.kind, e.message))) {
        Ok(value) => {
            let enabled = crate::orchestration::truthy(&value);
            crate::orchestration::codec::discard(value);
            enabled
        }
        Err(error) => {
            tracing::warn!(code = gate.config_failed, component = gate.what, %error, "sem configuração; não atualiza");
            let line = json!({"evento": gate.event, "nivel": "erro", "codigo": gate.config_failed, "detalhe": error});
            crate::diag::record_blocking(diary, &[line], gate.what);
            false
        }
    }
}

/// Espera `first`, passa, e repete a cada `INTERVAL`.
pub(crate) async fn clock<F>(diary: Option<PathBuf>, gate: &Gate, event: &'static str, first: Duration, mut pass: impl FnMut() -> F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::time::sleep(first).await;
    loop {
        guarded(diary.clone(), gate, event, pass()).await;
        tokio::time::sleep(INTERVAL).await;
    }
}

/// Cada passada na sua tarefa: um pânico vai ao diário com código e o relógio continua.
pub(crate) async fn guarded(diary: Option<PathBuf>, gate: &Gate, event: &'static str, work: impl Future<Output = ()> + Send + 'static) {
    if let Err(error) = tokio::spawn(work).await {
        panicked(diary.as_deref(), gate, event, &error);
    }
}

pub(crate) fn panicked(diary: Option<&Path>, gate: &Gate, event: &str, error: &tokio::task::JoinError) {
    tracing::error!(code = gate.panicked, component = gate.what, event, %error, "a tarefa caiu");
    let line = json!({"evento": event, "nivel": "erro", "codigo": gate.panicked, "detalhe": error.to_string()});
    crate::diag::record_blocking(diary, &[line], gate.what);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    const GATE: Gate = Gate { event: "teste.atualizar", config_failed: "teste_config_failed", panicked: "teste_task_panicked", what: "teste" };

    fn diary_text(dir: &Path) -> String {
        std::fs::read_dir(dir).unwrap().map(|file| std::fs::read_to_string(file.expect("entrada do diário").path()).unwrap()).collect()
    }

    /// Uma passada que entra em pânico vai ao diário com código, e a seguinte ainda roda.
    #[tokio::test]
    async fn a_panicking_pass_goes_to_the_diary_and_the_next_one_runs() {
        let diary = tempfile::tempdir().unwrap();
        guarded(Some(diary.path().to_owned()), &GATE, "teste.passada", async { panic!("de propósito"); }).await;
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        guarded(Some(diary.path().to_owned()), &GATE, "teste.passada", async move { flag.store(true, Ordering::SeqCst) }).await;
        assert!(ran.load(Ordering::SeqCst));
        assert!(diary_text(diary.path()).contains(GATE.panicked), "{}", diary_text(diary.path()));
    }

    /// Configuração ilegível: não atualiza e diz por quê no diário, com o código de quem passou.
    #[test]
    fn an_unreadable_config_does_not_update_and_goes_to_the_diary() {
        let diary = tempfile::tempdir().unwrap();
        assert!(!automations(Err("ilegível".into()), &GATE, Some(diary.path())));
        assert!(diary_text(diary.path()).contains(GATE.config_failed), "{}", diary_text(diary.path()));
    }

    /// O `runtime-config.json` que existe e não é JSON não atualiza, mesmo que diga automações desligadas.
    #[test]
    fn a_config_file_that_does_not_parse_does_not_update() {
        let home = tempfile::tempdir().unwrap();
        let env = crate::accounts::environment::AccountEnvironment::from_map([
            ("HOME".into(), home.path().display().to_string()),
            ("USERPROFILE".into(), home.path().display().to_string()),
            ("PATH".into(), String::new()),
        ].into());
        let config = RuntimeConfig::new(env).unwrap();
        std::fs::create_dir_all(config.path.parent().unwrap()).unwrap();
        std::fs::write(&config.path, br#"{"automations": false"#).unwrap();
        let diary = tempfile::tempdir().unwrap();
        assert!(!automations(Ok(config), &GATE, Some(diary.path())));
        assert!(diary_text(diary.path()).contains(GATE.config_failed), "{}", diary_text(diary.path()));
    }

    /// Fora do modo pacote, em canal de teste ou com automações desligadas, nenhum componente atualiza.
    #[test]
    fn a_closed_gate_does_not_update() {
        let latest = || Some("latest".to_owned());
        assert!(!Policy { automations: false, package_channel: latest() }.allows_update());
        assert!(!Policy { automations: true, package_channel: Some("beta".into()) }.allows_update());
        assert!(!Policy { automations: true, package_channel: None }.allows_update());
        assert!(Policy { automations: true, package_channel: latest() }.allows_update());
    }
}
