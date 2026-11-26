//! O WhatsApp sem pedido: 2 min depois da subida e de hora em hora, troca o `hangar-whatsapp` pela release
//! nova sob os mesmos portões do hcu. O resto é do crate: mudança de formato dos dados espera o dono, e a tag
//! que já falhou não é tentada de novo sozinha.
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hangar_whatsapp::AutoUpdate;
use hangar_whatsapp::install::Outcome;
use serde_json::{Value, json};

use crate::computer_control_install::{Gate, policy};
use crate::routes::AppState;
use crate::update::periodic::INTERVAL;

/// Fora da subida: a troca para o serviço, e a subida já tem o que fazer.
const FIRST_WAIT: Duration = Duration::from_secs(120);
const PANICKED: &str = "whatsapp_update_task_panicked";
const GATE: Gate = Gate { event: "whatsapp.atualizar", config_failed: "whatsapp_update_config_failed", what: "whatsapp" };

/// O que uma passada deixa no log e no diário.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Nothing,
    /// Sem troca: só log de depuração, com o nome do resultado.
    Quiet(&'static str),
    Updated { from: String, to: String },
    Failed { from: String, to: String, code: &'static str, detail: String },
}

/// Só a subida liga; nunca segura a subida.
pub(crate) fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let diary = crate::diag::diary_dir_in(&state.accounts.env);
        tokio::time::sleep(FIRST_WAIT).await;
        loop {
            guarded(diary.clone(), pass(state.clone())).await;
            tokio::time::sleep(INTERVAL).await;
        }
    });
}

/// Cada passada na sua tarefa: um pânico vai ao diário com código e o relógio continua.
async fn guarded(diary: Option<PathBuf>, work: impl Future<Output = ()> + Send + 'static) {
    if let Err(error) = tokio::spawn(work).await {
        tracing::error!(code = PANICKED, %error, "WhatsApp: a atualização automática caiu");
        let line = json!({"evento": GATE.event, "nivel": "erro", "codigo": PANICKED, "detalhe": error.to_string()});
        crate::diag::record_blocking(diary.as_deref(), &[line], GATE.what);
    }
}

async fn pass(state: Arc<AppState>) {
    let diary = crate::diag::diary_dir_in(&state.accounts.env);
    if !policy(&state, &GATE, diary.as_deref()).allows_update() {
        return;
    }
    let whatsapp = state.whatsapp.get_or_init(|| hangar_whatsapp::WhatsApp::new(&state.accounts.env.home)).clone();
    record(diary.as_deref(), &entry(whatsapp.auto_update().await));
}

/// `None`: não instalado ou outra tarefa do WhatsApp com a vez; a próxima hora tenta de novo.
fn entry(update: Option<AutoUpdate>) -> Entry {
    let Some(AutoUpdate { from, to, result }) = update else { return Entry::Nothing };
    match result {
        Ok(Outcome::Updated) => Entry::Updated { from, to },
        Ok(Outcome::AlreadyLatest) => Entry::Quiet("already_latest"),
        Ok(Outcome::NeedsConfirm) => Entry::Quiet("needs_confirm"),
        Ok(Outcome::Skipped) => Entry::Quiet("skipped"),
        Ok(Outcome::Installed) => Entry::Quiet("installed"),
        Err(failure) => Entry::Failed { from, to, code: failure.code, detail: failure.detail },
    }
}

/// Sem novidade na tela: a troca de versão do WhatsApp fica no log e no diário.
fn record(diary: Option<&Path>, entry: &Entry) {
    match entry {
        Entry::Nothing => {}
        Entry::Quiet(outcome) => tracing::debug!(outcome, "WhatsApp: atualização automática sem troca"),
        Entry::Updated { from, to } => tracing::info!(from = %from, to = %to, "WhatsApp: atualizado de {from} para {to}"),
        Entry::Failed { from, to, code, detail } => {
            tracing::warn!(code, from = %from, to = %to, detail = %detail, "WhatsApp: a atualização automática falhou");
        }
    }
    if let Some(line) = diary_line(entry) {
        crate::diag::record_blocking(diary, &[line], GATE.what);
    }
}

/// O detalhe fica só no log: pode trazer a saída do binário.
fn diary_line(entry: &Entry) -> Option<Value> {
    match entry {
        Entry::Updated { from, to } => Some(json!({"evento": "whatsapp.atualizado", "nivel": "info", "de": from, "para": to})),
        Entry::Failed { from, to, code, .. } => {
            Some(json!({"evento": GATE.event, "nivel": "erro", "codigo": code, "de": from, "para": to}))
        }
        Entry::Nothing | Entry::Quiet(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use hangar_computer_control_setup::Policy;
    use hangar_whatsapp::install::Failure;

    use super::*;

    fn update(result: Result<Outcome, Failure>) -> Option<AutoUpdate> {
        Some(AutoUpdate { from: "v0.1.1".into(), to: "v0.2.0".into(), result })
    }

    fn diary_text(dir: &Path) -> String {
        std::fs::read_dir(dir).unwrap().flatten().map(|file| std::fs::read_to_string(file.path()).unwrap()).collect()
    }

    /// Fora do modo pacote, em canal de teste ou com automações desligadas a passada não chama o WhatsApp.
    #[test]
    fn a_closed_gate_does_not_update() {
        let latest = || Some("latest".to_owned());
        assert!(!Policy { automations: false, package_channel: latest() }.allows_update());
        assert!(!Policy { automations: true, package_channel: Some("beta".into()) }.allows_update());
        assert!(!Policy { automations: true, package_channel: None }.allows_update());
        assert!(Policy { automations: true, package_channel: latest() }.allows_update());
    }

    /// Sem o WhatsApp instalado não há troca nem registro, e nada vai à rede.
    #[tokio::test]
    async fn without_whatsapp_installed_the_pass_records_nothing() {
        let home = tempfile::tempdir().unwrap();
        let whatsapp = hangar_whatsapp::WhatsApp::new(home.path());
        let entry = entry(whatsapp.auto_update().await);
        assert_eq!(entry, Entry::Nothing);
        assert_eq!(diary_line(&entry), None);
    }

    /// Outra tarefa com a vez (`None`) não deixa registro.
    #[test]
    fn a_busy_installer_records_nothing() {
        assert_eq!(entry(None), Entry::Nothing);
    }

    /// A troca vai ao diário com as duas tags.
    #[test]
    fn an_update_goes_to_the_diary_with_both_tags() {
        let entry = entry(update(Ok(Outcome::Updated)));
        assert_eq!(entry, Entry::Updated { from: "v0.1.1".into(), to: "v0.2.0".into() });
        assert_eq!(diary_line(&entry), Some(json!({"evento": "whatsapp.atualizado", "nivel": "info", "de": "v0.1.1", "para": "v0.2.0"})));
    }

    /// A falha vai ao diário com o código, sem o detalhe.
    #[test]
    fn a_failure_goes_to_the_diary_with_its_code() {
        let entry = entry(update(Err(Failure::new("whatsapp_update_restored", "saída do binário"))));
        assert_eq!(diary_line(&entry), Some(json!({"evento": "whatsapp.atualizar", "nivel": "erro",
            "codigo": "whatsapp_update_restored", "de": "v0.1.1", "para": "v0.2.0"})));
    }

    /// Sem troca (em dia, à espera do dono ou tag que já falhou) só o log de depuração.
    #[test]
    fn no_swap_stays_out_of_the_diary() {
        for (outcome, name) in [(Outcome::AlreadyLatest, "already_latest"), (Outcome::NeedsConfirm, "needs_confirm"), (Outcome::Skipped, "skipped")] {
            let entry = entry(update(Ok(outcome)));
            assert_eq!(entry, Entry::Quiet(name));
            assert_eq!(diary_line(&entry), None);
        }
    }

    /// Uma passada que entra em pânico vai ao diário com código, e a seguinte ainda roda.
    #[tokio::test]
    async fn a_panicking_pass_goes_to_the_diary_and_the_next_one_runs() {
        let diary = tempfile::tempdir().unwrap();
        guarded(Some(diary.path().to_owned()), async { panic!("de propósito"); }).await;
        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        guarded(Some(diary.path().to_owned()), async move { flag.store(true, Ordering::SeqCst) }).await;
        assert!(ran.load(Ordering::SeqCst));
        assert!(diary_text(diary.path()).contains(PANICKED), "{}", diary_text(diary.path()));
    }
}
