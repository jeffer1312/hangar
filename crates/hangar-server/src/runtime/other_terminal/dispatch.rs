//! O mesmo proprietário decide linha nativa, teclado, finalização e confirmação.
use super::super::{queue::Status, receipt::ReceiptIndex};
use super::*;
fn status(value: crate::terminal_input::Disposition) -> Status {
    match value {
        crate::terminal_input::Disposition::Accepted => Status::Accepted,
        crate::terminal_input::Disposition::Deferred => Status::Deferred,
        crate::terminal_input::Disposition::Rejected => Status::Rejected,
        crate::terminal_input::Disposition::Unknown => Status::Unknown,
    }
}
pub(super) async fn drain(
    target: &OtherTarget,
    queue: &QueueActor,
    options: &OtherOptions,
    io: &Arc<dyn OtherIo>,
    snapshot: &tokio::sync::watch::Sender<Value>,
    initial: Option<&OtherFrame>,
    episode: &mut deferred_episode::DeferredEpisode,
) -> Result<Value, RuntimeError> {
    let native = native::drain(target, queue, options, io, snapshot).await?;
    if native["attempted"] == true {
        return Ok(native);
    }
    if initial.is_none() {
        return Ok(native);
    }
    let state = queue.snapshot().await.map_err(|_| failed("queue_io"))?;
    if state.terminal_write_blocked(&target.binding.conversation) {
        return Ok(native);
    }
    let Some(row) = state
        .rows
        .iter()
        .find(|row| row["delivered"] == false && super::super::queue::row_waits(row))
        .cloned()
    else {
        return Ok(native);
    };
    if read_pending_interaction(target)?.is_some() {
        episode.record(target, options, crate::terminal_input::DeliveryStage::Ready, "question_open");
        return Ok(native);
    }
    let id = row["id"]
        .as_str()
        .ok_or_else(|| failed("queue_entry_invalid"))?
        .to_owned();
    let text = row["text"]
        .as_str()
        .ok_or_else(|| failed("invalid_text"))?
        .to_owned();
    let aware = EditorAwareOtherIo {
        base: io.clone(),
        target: target.clone(),
        inbox: options.inbox.clone(),
    };
    let enriched = if matches!(
        target.provider,
        hangar_transcript::Provider::Pi | hangar_transcript::Provider::Omp
    ) {
        match initial {
            Some(frame) => Some(aware.enrich(frame.clone()).await?),
            None => None,
        }
    } else {
        initial.cloned()
    };
    let path = target.transcript.clone();
    let provider = target.provider.as_str();
    let conversation = target.binding.conversation.clone();
    let cursor = tokio::task::spawn_blocking(move || {
        ReceiptIndex::new(provider, &conversation).capture(&path)
    })
    .await
    .map_err(|_| failed("receipt_worker"))?
    .map_err(|_| failed("receipt_scan"))?;
    let claimed = queue
        .exec(
            target.generation,
            &format!("physical-claim:{id}:{}", state.next_seq),
            clock(),
            Action::Claim {
                min_ts: target.created,
                limit: Some(1),
                entry_id: Some(id.clone()),
            },
        )
        .await
        .map_err(|_| failed("queue_io"))?;
    if claimed.as_array().is_none_or(Vec::is_empty) {
        return Ok(native);
    }
    let op = format!("other-send:{id}");
    let wire = format!("terminal:{}:{id}:{}", target.generation, state.next_seq);
    for (suffix, action) in [
        (
            "prepare",
            Action::Prepare {
                id: op.clone(),
                entry_id: Some(id.clone()),
                payload: json!({"kind":"input","payload":{"text":text,"conversation":target.binding.conversation,"_terminal_generation":target.generation}}),
            },
        ),
        (
            "bind",
            Action::BindDispatch {
                id: op.clone(),
                cursor: serde_json::to_value(cursor).map_err(|_| failed("receipt_cursor"))?,
            },
        ),
        (
            "begin",
            Action::BeginDispatch {
                id: op.clone(),
                wire_id: wire.clone(),
                staged: true,
            },
        ),
        (
            "writing",
            Action::MarkWriting {
                id: op.clone(),
                wire_id: wire,
            },
        ),
    ] {
        queue
            .exec(
                target.generation,
                &format!("physical:{id}:{suffix}:{}", state.next_seq),
                clock(),
                action,
            )
            .await
            .map_err(|_| failed("queue_io"))?;
    }
    let mut composer_checked = |empty| episode.composer_checked(empty);
    let result = if matches!(
        target.provider,
        hangar_transcript::Provider::Pi | hangar_transcript::Provider::Omp
    ) {
        drive::prompt_with_episode(target, &aware, &text, enriched.as_ref(), &mut composer_checked).await
    } else {
        drive::prompt_with_episode(target, &**io, &text, enriched.as_ref(), &mut composer_checked).await
    };
    episode.record(target, options, result.stage, &result.code);
    let outcome = status(result.disposition);
    let payload = serde_json::to_value(&result).map_err(|_| failed("runtime_reply"))?;
    let reply = super::super::protocol::RuntimeReply {
        operation_id: op.clone(),
        disposition: match outcome {
            Status::Accepted => Disposition::Accepted,
            Status::Deferred => Disposition::Deferred,
            Status::Rejected => Disposition::Rejected,
            _ => Disposition::Unknown,
        },
        payload,
    };
    queue
        .exec(
            target.generation,
            &format!("physical:{id}:finish:{}", state.next_seq),
            clock(),
            Action::Finish {
                id: op,
                status: outcome,
                result: serde_json::to_value(reply).map_err(|_| failed("runtime_reply"))?,
            },
        )
        .await
        .map_err(|_| failed("queue_io"))?;
    if outcome == Status::Deferred
        && queue
            .snapshot()
            .await
            .map_err(|_| failed("queue_io"))?
            .rows
            .iter()
            .any(|row| row["id"] == id && row["desistiu"] != true)
    {
        queue
            .exec(
                target.generation,
                &format!("physical:{id}:waiting:{}", state.next_seq),
                clock(),
                Action::SetDelivered {
                    entry_id: id,
                    value: false,
                    steered: false,
                },
            )
            .await
            .map_err(|_| failed("queue_io"))?;
    }
    publish_snapshot(target, queue, snapshot, None).await?;
    Ok(json!({"drained":usize::from(outcome==Status::Accepted)}))
}

pub(super) async fn direct_prompt(
    target: &OtherTarget, options: &OtherOptions, io: &Arc<dyn OtherIo>,
    operation_id: &str, text: &str, initial: Option<&OtherFrame>,
    episode: &mut deferred_episode::DeferredEpisode,
) -> Result<RuntimeReply, RuntimeError> {
    let aware = EditorAwareOtherIo {base:io.clone(), target:target.clone(), inbox:options.inbox.clone()};
    let frame = if matches!(target.provider, hangar_transcript::Provider::Pi | hangar_transcript::Provider::Omp) {
        match initial {Some(frame) => Some(aware.enrich(frame.clone()).await?), None => None}
    } else {initial.cloned()};
    let mut composer_checked = |empty| episode.composer_checked(empty);
    let result = if matches!(target.provider, hangar_transcript::Provider::Pi | hangar_transcript::Provider::Omp) {
        drive::prompt_with_episode(target, &aware, text, frame.as_ref(), &mut composer_checked).await
    } else { drive::prompt_with_episode(target, &**io, text, frame.as_ref(), &mut composer_checked).await };
    episode.record(target, options, result.stage, &result.code);
    let disposition = match result.disposition {
        crate::terminal_input::Disposition::Accepted => Disposition::Accepted,
        crate::terminal_input::Disposition::Deferred => Disposition::Deferred,
        crate::terminal_input::Disposition::Rejected => Disposition::Rejected,
        crate::terminal_input::Disposition::Unknown => Disposition::Unknown,
    };
    Ok(RuntimeReply {operation_id:operation_id.into(), disposition,
        payload:json!({"queued":false,"native":result.native,"code":result.code,"stage":result.stage,"cleanup":result.cleanup})})
}

/// Só a entrada física consulta editor; estado e prévia continuam com a captura compartilhada.
struct EditorAwareOtherIo {
    base: Arc<dyn OtherIo>,
    target: OtherTarget,
    inbox: Arc<crate::provider_models::inbox::Store>,
}
impl EditorAwareOtherIo {
    async fn editor(&self) -> Result<Option<String>, RuntimeError> {
        if !self.base.prove(&self.target).await? {
            return Err(failed("other_terminal_life_changed"));
        }
        let value = match inbox_line(&self.target, &self.inbox) {
            Some(key) => self.inbox.question(&key, "editor").await,
            None => None,
        };
        if !self.base.prove(&self.target).await? {
            return Err(failed("other_terminal_life_changed"));
        }
        Ok(value)
    }
    async fn enrich(&self, mut frame: OtherFrame) -> Result<OtherFrame, RuntimeError> {
        frame.editor = self.editor().await?;
        Ok(frame)
    }
}
impl OtherIo for EditorAwareOtherIo {
    fn prove<'a>(&'a self, target: &'a OtherTarget) -> BoxFuture<'a, Result<bool, RuntimeError>> {
        self.base.prove(target)
    }
    fn capture<'a>(&'a self) -> BoxFuture<'a, Result<OtherFrame, RuntimeError>> {
        Box::pin(async move {
            let editor = self.editor().await?;
            let mut frame = self.base.capture().await?;
            if !self.base.prove(&self.target).await? {
                return Err(failed("other_terminal_life_changed"));
            }
            frame.editor = editor;
            Ok(frame)
        })
    }
    fn write<'a>(
        &'a self,
        target: &'a OtherTarget,
        value: &'a str,
        literal: bool,
        guard: Option<&'a OtherWriteGuard>,
    ) -> BoxFuture<'a, Result<(), RuntimeError>> {
        self.base.write(target, value, literal, guard)
    }
    fn release<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

/// A CLI só cria o transcript depois do primeiro Enter; a fila já protege essa escrita.
pub(in crate::runtime) async fn send_opening(
    opening: &OtherOpening, queue: &QueueActor, options: &OtherOptions,
) -> Result<(), RuntimeError> {
    let Some(target) = opening.input_target() else {return Ok(());};
    let io: Arc<dyn OtherIo> = options.io_override.clone()
        .unwrap_or_else(|| Arc::new(ProcessOtherIo::new(&target, options)));
    let result = async {
        if !io.prove(&target).await? {return Err(failed("other_terminal_life_changed"));}
        let frame = io.capture().await?;
        let (snapshot, _) = tokio::sync::watch::channel(Value::Null);
        let mut episode = deferred_episode::DeferredEpisode::default();
        drain(&target, queue, options, &io, &snapshot, Some(&frame), &mut episode).await?;
        Ok(())
    }.await;
    io.release().await;
    result
}
