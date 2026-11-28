//! Pendências nativas e identidade dos menus do terminal.
use hangar_core::runtime_reply::RuntimeError;
use hangar_interactions::{InteractionKind, InteractionTarget, native_target};
use serde_json::{Value, json};

use crate::{OtherTarget, failed};

#[derive(Clone)]
pub struct PendingInteraction {
    pub target: InteractionTarget,
    pub semantic: Value,
    pub native_id: String,
    pub questions: Vec<Value>,
    pub approval: Option<Value>,
}

fn source_error(error: std::io::Error) -> RuntimeError {
    failed(if error.kind() == std::io::ErrorKind::NotFound {
        "other_terminal_transcript_missing"
    } else {
        "other_terminal_transcript_unreadable"
    })
}
pub fn read_pending_interaction(target: &OtherTarget) -> Result<Option<PendingInteraction>, RuntimeError> {
    use hangar_transcript::interactions::*;
    let read = match target.provider {
        hangar_transcript::Provider::Pi => read_pending_pi_call(&target.transcript, "question"),
        hangar_transcript::Provider::Omp => read_pending_pi_call(&target.transcript, "ask"),
        hangar_transcript::Provider::Kimi => read_pending_kimi_call(&target.transcript),
        // O `agy` não grava pergunta pendente no transcript: a pergunta é lida da tela (menu).
        hangar_transcript::Provider::Antigravity => return Ok(None),
        _ => return Err(failed("other_terminal_provider")),
    };
    let call = match read {
        Ok(call) => call,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(source_error(error)),
    };
    if let Some(call) = call {
        let questions = if target.provider == hangar_transcript::Provider::Kimi {
            call.args
                .get("questions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        } else {
            vec![Value::Object(call.args)]
        };
        let native = native_target(
            InteractionKind::Question,
            &target.binding.conversation,
            &json!(call.id),
        );
        let semantic =
            json!({"questions":questions,"request_id":call.id,"provider":target.provider.as_str()});
        return Ok(Some(PendingInteraction {
            target: native,
            semantic,
            native_id: call.id,
            questions,
            approval: None,
        }));
    }
    if target.provider == hangar_transcript::Provider::Kimi {
        let approval = match read_pending_kimi_interaction(&target.transcript) {
            Ok(approval) => approval,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(source_error(error)),
        };
        if let Some(approval) = approval {
            let id = approval["id"]
                .as_str()
                .ok_or_else(|| failed("other_terminal_interaction_invalid"))?
                .to_owned();
            let native = native_target(
                InteractionKind::Approval,
                &target.binding.conversation,
                &json!(id),
            );
            return Ok(Some(PendingInteraction {
                target: native,
                semantic: approval.clone(),
                native_id: id,
                questions: vec![],
                approval: Some(approval),
            }));
        }
    }
    Ok(None)
}

pub fn menu_interaction(question: Option<String>, options: Vec<String>) -> PendingInteraction {
    let semantic = json!({"question":question,"options":options});
    PendingInteraction {
        target: InteractionTarget {
            kind: InteractionKind::Menu,
            request_key: format!(
                "other-menu:{}",
                sha1_smol::Sha1::from(semantic.to_string()).digest()
            ),
        },
        semantic,
        native_id: String::new(),
        questions: vec![],
        approval: None,
    }
}
