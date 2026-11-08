//! Uma leitura de arquivos e um quadro alimentam estado, interação e prévia da mesma rodada.
use super::*;
use crate::interactions::{InteractionKind, InteractionTarget};
use crate::terminal_state::{ReducerFacts, ReducerMemory};
use hangar_api::state::StateEvent;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Clone)]
pub struct PendingInteraction {
    pub target: InteractionTarget,
    pub semantic: Value,
    pub native_id: String,
    pub questions: Vec<Value>,
    pub approval: Option<Value>,
}
pub(super) struct Observation {
    pub state: StateEvent,
    pub memory: ReducerMemory,
    pub pending: Option<PendingInteraction>,
    pub files: Vec<crate::state::preview::HookFile>,
    pub marker: Option<(String, f64)>,
    pub now: f64,
}
#[derive(Default)]
pub(super) struct FileCache {
    states: crate::list::facts_files::HookStates,
    previews: crate::state::preview::HookFiles,
}
static APPROVAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"↵\s*confirm|Type feedback").unwrap());
static PICKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"↵\s*(choose|toggle|save)|type answer").unwrap());
pub fn approval_open(pane: &str) -> bool {
    APPROVAL.is_match(pane)
}
pub fn picker_open(pane: &str) -> bool {
    PICKER.is_match(pane)
}
fn source_error(error: std::io::Error) -> RuntimeError {
    failed(if error.kind() == std::io::ErrorKind::NotFound {
        "other_terminal_transcript_missing"
    } else {
        "other_terminal_transcript_unreadable"
    })
}
pub(super) fn pending(target: &OtherTarget) -> Result<Option<PendingInteraction>, RuntimeError> {
    use hangar_transcript::interactions::*;
    let read = match target.provider {
        hangar_transcript::Provider::Pi => read_pending_pi_call(&target.transcript, "question"),
        hangar_transcript::Provider::Omp => read_pending_pi_call(&target.transcript, "ask"),
        hangar_transcript::Provider::Kimi => read_pending_kimi_call(&target.transcript),
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
        let native = crate::interactions::native_target(
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
            let native = crate::interactions::native_target(
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

pub(super) fn read(
    target: &OtherTarget,
    pane: &str,
    memory: ReducerMemory,
    cache: &mut FileCache,
    interested: bool,
) -> Result<Observation, RuntimeError> {
    let now = clock().epoch_s;
    cache.states.refresh(&target.config_dirs);
    let marker = cache
        .states
        .get_state(Some(&target.binding.conversation), |_| false)
        .filter(|marker| marker.ts >= target.agent_birth.max(target.created));
    let mut hook = marker.as_ref().map(|marker| marker.state.clone());
    let mut pending = pending(target)?;
    if pending
        .as_ref()
        .is_some_and(|pending| pending.approval.is_some())
        && !approval_open(pane)
    {
        pending = None;
    }
    let mut analysis = crate::terminal_state::analyze(pane);
    if target.provider == hangar_transcript::Provider::Kimi
        && hook.as_deref() != Some("awaiting_input")
    {
        match hangar_core::provider_state::kimi_turn_boundary(&target.transcript) {
            Ok(Some(boundary))
                if boundary
                    .timestamp_ms
                    .is_none_or(|timestamp| timestamp / 1000.0 >= target.agent_birth) =>
            {
                hook = Some(if boundary.open { "working" } else { "idle" }.into())
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                if hook.as_deref() == Some("idle")
                    && let Some(marker) = &marker
                    && kimi_session_mtime(&target.transcript)
                        .is_some_and(|mtime| mtime > marker.ts + 2.0)
                {
                    hook = Some("working".into());
                }
            }
        }
    }
    if pane.trim().is_empty() && hook.is_none() && pending.is_none() {
        return Err(failed("other_terminal_capture_unavailable"));
    }
    let status = crate::list::facts_files::published_status(
        Some(&target.binding.conversation),
        &target.config_dirs,
        now,
    );
    let (reduced, _) = crate::terminal_state::reduce_analysis(
        analysis,
        memory,
        ReducerFacts {
            hook_state: hook,
            hook_grace: None,
            status_line: status.map(|status| status.line),
            ..Default::default()
        },
    );
    analysis = reduced.analysis;
    let mut state = StateEvent {
        session: target.name.clone(),
        state: analysis.state,
        label: analysis.label,
        question: analysis.question,
        options: analysis.options,
        status_line: analysis.status_line,
        overlay: analysis.overlay,
        login: analysis.login,
        limited: analysis.limit_reset.is_some(),
        limit_reset: analysis.limit_reset,
        ..Default::default()
    };
    if let Some(pending) = &pending {
        state.state = "awaiting_input".into();
        if let Some(approval) = &pending.approval {
            let title = approval["titulo"].as_str().unwrap_or_default();
            let summary = approval["resumo"].as_str().unwrap_or_default();
            state.question = Some(if summary.is_empty() {
                title.into()
            } else {
                format!("{title}\n{summary}")
            });
            state.options = Some(
                approval["escolhas"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|choice| choice["label"].as_str().map(str::to_owned))
                    .collect(),
            );
        }
    }
    if pending.is_none()
        && state.state == "awaiting_input"
        && let Some(options) = &state.options
    {
        pending = Some(menu(state.question.clone(), options.clone()));
    }
    let files = if interested {
        cache
            .previews
            .read(&target.config_dirs, &target.binding.conversation)
    } else {
        vec![]
    };
    Ok(Observation {
        state,
        memory: reduced.memory,
        pending,
        files,
        marker: marker.map(|marker| (marker.state, marker.ts)),
        now,
    })
}

pub fn menu(question: Option<String>, options: Vec<String>) -> PendingInteraction {
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

fn kimi_session_mtime(path: &std::path::Path) -> Option<f64> {
    let mtime = |path: &std::path::Path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|mtime| mtime.as_secs_f64())
    };
    let own = mtime(path);
    let Some(agents) = path
        .parent()
        .and_then(std::path::Path::parent)
        .filter(|path| path.file_name().is_some_and(|name| name == "agents"))
    else {
        return own;
    };
    match std::fs::read_dir(agents) {
        Ok(entries) => entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| mtime(&entry.path().join("wire.jsonl")))
            .max_by(f64::total_cmp)
            .or(own),
        Err(_) => {
            if crate::warn_limit::allow(None, "other_kimi_agents_mtime_unavailable") {
                tracing::warn!(
                    code = "other_kimi_agents_mtime_unavailable",
                    "a varredura dos agentes não respondeu; usa mtime principal"
                );
            }
            own
        }
    }
}
