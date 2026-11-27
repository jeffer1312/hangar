//! O WhatsApp sem pedido: 2 min depois da subida e de hora em hora, troca o `hangar-whatsapp` pela release
//! nova sob os mesmos portões do hcu. O resto é do crate: mudança de formato dos dados espera o dono, e a tag
//! que já falhou não é tentada de novo sozinha.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hangar_whatsapp::AutoUpdate;
use hangar_whatsapp::install::{self, Outcome};
use serde_json::{Value, json};

use crate::routes::AppState;
use crate::update::auto_gate::{Gate, clock, policy};

/// Fora da subida: a troca para o serviço, e a subida já tem o que fazer.
const FIRST_WAIT: Duration = Duration::from_secs(120);
const GATE: Gate = Gate {
    event: "whatsapp.atualizar",
    config_failed: "whatsapp_update_config_failed",
    panicked: "whatsapp_update_task_panicked",
    what: "whatsapp",
};

/// Só a subida liga; nunca segura a subida.
pub(crate) fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let diary = crate::diag::diary_dir_in(&state.accounts.env);
        clock(diary.clone(), &GATE, GATE.event, FIRST_WAIT, || pass(state.clone(), diary.clone())).await;
    });
}

/// Sem WhatsApp instalado nem lê os portões: a configuração ilegível não vai ao diário a cada hora por ele.
async fn pass(state: Arc<AppState>, diary: Option<PathBuf>) {
    let home = &state.accounts.env.home;
    if !installed(home) || !policy(&state, &GATE, diary.as_deref()).allows_update() {
        return;
    }
    let whatsapp = state.whatsapp.get_or_init(|| hangar_whatsapp::WhatsApp::new(home)).clone();
    record(diary.as_deref(), whatsapp.auto_update().await.as_ref());
}

fn installed(home: &Path) -> bool {
    install::current(&install::Paths::new(home)).is_some()
}

/// `None`: outra tarefa do WhatsApp com a vez; a próxima hora tenta de novo. Sem novidade na tela: a troca
/// de versão do WhatsApp fica no log e no diário.
fn record(diary: Option<&Path>, update: Option<&AutoUpdate>) {
    let Some(AutoUpdate { from, to, result }) = update else { return };
    match result {
        Ok(Outcome::Updated) => tracing::info!(from = %from, to = %to, "WhatsApp: atualizado de {from} para {to}"),
        Ok(outcome) => tracing::debug!(outcome = ?outcome, from = %from, to = %to, "WhatsApp: atualização automática sem troca"),
        Err(failure) => {
            tracing::warn!(code = failure.code, from = %from, to = %to, "WhatsApp: a atualização automática falhou");
        }
    }
    if let Some(line) = diary_line(update) {
        crate::diag::record_blocking(diary, &[line], GATE.what);
    }
}

/// O detalhe da falha fica só no log: pode trazer a saída do binário.
fn diary_line(update: Option<&AutoUpdate>) -> Option<Value> {
    let AutoUpdate { from, to, result } = update?;
    match result {
        Ok(Outcome::Updated) => Some(json!({"evento": "whatsapp.atualizado", "nivel": "info", "de": from, "para": to})),
        Ok(_) => None,
        Err(failure) => Some(json!({"evento": GATE.event, "nivel": "erro", "codigo": failure.code, "de": from, "para": to})),
    }
}

#[cfg(test)]
mod tests {
    use hangar_whatsapp::install::Failure;

    use super::*;

    fn update(result: Result<Outcome, Failure>) -> AutoUpdate {
        AutoUpdate { from: "v0.1.1".into(), to: "v0.2.0".into(), result }
    }

    /// Sem `install.json` a passada para antes dos portões e do WhatsApp.
    #[test]
    fn without_whatsapp_installed_the_pass_stops_first() {
        let home = tempfile::tempdir().unwrap();
        assert!(!installed(home.path()));
    }

    /// Outra tarefa com a vez (`None`) não deixa registro.
    #[test]
    fn a_busy_installer_records_nothing() {
        assert_eq!(diary_line(None), None);
    }

    /// A troca vai ao diário com as duas tags.
    #[test]
    fn an_update_goes_to_the_diary_with_both_tags() {
        assert_eq!(diary_line(Some(&update(Ok(Outcome::Updated)))),
            Some(json!({"evento": "whatsapp.atualizado", "nivel": "info", "de": "v0.1.1", "para": "v0.2.0"})));
    }

    /// A falha vai ao diário com o código, sem o detalhe.
    #[test]
    fn a_failure_goes_to_the_diary_with_its_code() {
        let failed = update(Err(Failure::new("whatsapp_update_restored", "saída do binário")));
        assert_eq!(diary_line(Some(&failed)), Some(json!({"evento": "whatsapp.atualizar", "nivel": "erro",
            "codigo": "whatsapp_update_restored", "de": "v0.1.1", "para": "v0.2.0"})));
    }

    /// Sem troca (em dia, à espera do dono ou tag que já falhou) só o log de depuração.
    #[test]
    fn no_swap_stays_out_of_the_diary() {
        for outcome in [Outcome::AlreadyLatest, Outcome::NeedsConfirm, Outcome::Skipped] {
            assert_eq!(diary_line(Some(&update(Ok(outcome)))), None, "{outcome:?}");
        }
    }
}
