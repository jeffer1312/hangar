// Entrada física da vida adotada; fila e confirmação durável pertencem ao ator.
use super::{
    OtherFrame, OtherIo, OtherTarget, OtherWriteGuard, PendingInteraction, kimi_approval_open,
    kimi_picker_open, menu_interaction, read_pending_interaction,
};
use crate::interactions::{
    ExpectedInteraction, InteractionBinding, InteractionError, InteractionKind, InteractionStore,
};
use crate::runtime::protocol::{Disposition as RuntimeDisposition, RuntimeError, RuntimeReply};
use crate::terminal_input::{
    Cleanup, ComposerSnapshot, DeliveryResult, DeliveryStage, Disposition, Proof,
};
use regex::Regex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashSet},
    sync::{LazyLock, Mutex},
    time::Duration,
};
use tokio::time::Instant;

const SETTLE: Duration = Duration::from_millis(300);
const OPEN_SETTLE: Duration = Duration::from_millis(700);
const INPUT_SETTLE: Duration = Duration::from_millis(200);
const PROOF_INTERVAL: Duration = Duration::from_millis(50);
static QUESTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*\?\s+(\S.*?)\s*$").unwrap());
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(?:Pasted\s*text|paste)\s*#(\d+)|\[Image\s*#(\d+)\]").unwrap());
static PI_CURSOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*>\s*(\d+)\.").unwrap());
static AGENT_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:❯\s+)?[●◯]\s+(\S+)").unwrap());
static PI_NOTICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Subagentasyncgroupedresultintercomdeliverywasnotacknowledgedfor.*?/tmp/pi-subagents-[^']*'?\.?").unwrap()
});
static MULTIPLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*[❯>]?\s*\d+\.\s*\[.?\]\s").unwrap());
static CURSOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*[>❯›]\s*(\d+)\.").unwrap());
static READY_WARNED: LazyLock<Mutex<HashSet<(String, u64, String)>>> =
    LazyLock::new(Default::default);
/// Marcas de TUI pronta do `_wait_input_ready` do Python, procuradas só no rodapé.
const CODEX_READY: [&str; 5] = ["⏵⏵", "⏸", "bypass permissions", "? for shortcuts", "for shortcuts"];
const CODEX_READY_LINES: usize = 12;
const CODEX_READY_WAIT: Duration = Duration::from_secs(12);
const CODEX_PLACEHOLDER: &str = "Ask Codex to do anything";

pub struct OtherControlOutcome {
    pub reply: RuntimeReply,
    pub fallback_text: Option<String>,
}

pub fn forget(target: &OtherTarget) {
    READY_WARNED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(
            target.key.clone(),
            target.generation,
            target.binding.conversation.clone(),
        ));
}
fn failure(code: &str) -> RuntimeError {
    RuntimeError::new(
        code,
        "não foi possível comprovar a operação nesta vida do terminal",
    )
}
fn interaction_error(error: InteractionError) -> RuntimeError {
    crate::interactions::runtime_error(error)
}
fn binding(target: &OtherTarget) -> InteractionBinding {
    InteractionBinding {
        key: target.key.clone(),
        conversation: target.binding.conversation.clone(),
        generation: target.generation,
    }
}
fn valid_text(text: &str, newlines: bool) -> bool {
    text.chars()
        .all(|c| u32::from(c) >= 32 || c == '\t' || newlines && c == '\n')
}
fn compact(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !"│┃║".contains(*c))
        .collect()
}
fn result(
    disposition: Disposition,
    stage: DeliveryStage,
    cleanup: Cleanup,
    code: &str,
) -> DeliveryResult {
    DeliveryResult {
        disposition,
        stage,
        cleanup,
        native: false,
        message_id: None,
        code: code.into(),
        draft: None,
        consumed: false,
    }
}
async fn life(target: &OtherTarget, io: &dyn OtherIo) -> Result<(), RuntimeError> {
    if io.prove(target).await? {
        Ok(())
    } else {
        Err(failure("other_terminal_life_changed"))
    }
}
async fn capture(target: &OtherTarget, io: &dyn OtherIo) -> Result<OtherFrame, RuntimeError> {
    let frame = io.capture().await;
    life(target, io).await?;
    frame
}
async fn pending(
    target: &OtherTarget,
    io: &dyn OtherIo,
) -> Result<Option<PendingInteraction>, RuntimeError> {
    // Codex cru (sem sidecar): não há conversa de onde ler pergunta nativa.
    if codex(target) {
        life(target, io).await?;
        return Ok(None);
    }
    let owned = target.clone();
    let value = tokio::task::spawn_blocking(move || read_pending_interaction(&owned))
        .await
        .map_err(|_| failure("other_terminal_interaction_worker"))?;
    life(target, io).await?;
    value
}
async fn write_io(
    target: &OtherTarget,
    io: &dyn OtherIo,
    value: &str,
    literal: bool,
    guard: Option<&OtherWriteGuard>,
) -> Result<(), RuntimeError> {
    io.write(target, value, literal, guard).await
}
fn blocked(frame: &OtherFrame) -> bool {
    let analysis = crate::terminal_state::analyze(&frame.text);
    analysis.overlay || analysis.state == "awaiting_input"
}
fn codex(target: &OtherTarget) -> bool {
    target.provider == hangar_transcript::Provider::Codex
}
fn ready(target: &OtherTarget, frame: &OtherFrame) -> bool {
    if codex(target) {
        // No pane inteiro a marca casaria com o texto da conversa.
        return frame
            .text
            .trim_end_matches('\n')
            .split('\n')
            .rev()
            .take(CODEX_READY_LINES)
            .any(|line| CODEX_READY.iter().any(|mark| line.contains(mark)));
    }
    frame
        .text
        .trim_end_matches('\n')
        .lines()
        .rev()
        .take(20)
        .any(|line| line.chars().any(|c| "─━═╰│".contains(c)))
}
fn composer(target: &OtherTarget, frame: &OtherFrame) -> Option<ComposerSnapshot> {
    if codex(target) {
        return codex_composer(frame);
    }
    if matches!(
        target.provider,
        hangar_transcript::Provider::Pi | hangar_transcript::Provider::Omp
    ) && let Some(editor) = &frame.editor
    {
        return Some(ComposerSnapshot {
            content: editor.clone(),
            placeholders: BTreeSet::new(),
            stashed: false,
        });
    }
    let styled;
    let text = if let Some(typed) = &frame.typed {
        typed.as_str()
    } else if let Some(value) = &frame.styled {
        styled = crate::terminal_input::unstyle(value, false);
        &styled
    } else {
        &frame.text
    };
    let mut lines: Vec<_> = text.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let panel = lines
        .iter()
        .rev()
        .take_while(|line| AGENT_ROW.is_match(line))
        .count();
    let rows = &lines[lines.len() - panel..];
    if rows.iter().any(|line| line.contains('◯'))
        && rows
            .iter()
            .any(|line| AGENT_ROW.captures(line).is_some_and(|c| &c[1] == "main"))
    {
        lines.truncate(lines.len() - panel);
        while lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
    }
    let rules: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.matches('─').count() >= 20)
        .map(|(n, _)| n)
        .collect();
    let bottom = *rules.last()?;
    let top = *rules.get(rules.len().checked_sub(2)?)?;
    if lines.len() - bottom > 8 || bottom - top > 15 {
        return None;
    }
    let mut content = lines[top + 1..bottom]
        .iter()
        .map(|line| {
            let line = line
                .trim()
                .trim_start_matches(['│', '┃', '║'])
                .trim_end_matches(['│', '┃', '║'])
                .trim();
            line.strip_prefix('>')
                .or_else(|| line.strip_prefix('❯'))
                .unwrap_or(line)
                .trim()
        })
        .collect::<Vec<_>>()
        .join("\n");
    if matches!(
        target.provider,
        hangar_transcript::Provider::Pi | hangar_transcript::Provider::Omp
    ) {
        content = PI_NOTICE.replace_all(&compact(&content), "").into_owned();
    }
    let placeholders = PLACEHOLDER
        .captures_iter(&content)
        .map(|captures| match captures.get(1) {
            Some(paste) => paste.as_str().to_owned(),
            None => format!("img{}", &captures[2]),
        })
        .collect();
    Some(ComposerSnapshot {
        content,
        placeholders,
        stashed: false,
    })
}
/// A caixa de digitar do Codex: a última linha `›` e as que continuam o texto até a linha em branco.
/// A sugestão vem esmaecida e sai do `typed`; a captura sem estilo traz a sugestão padrão por extenso.
fn codex_composer(frame: &OtherFrame) -> Option<ComposerSnapshot> {
    let text = frame.typed.as_deref().unwrap_or(&frame.text);
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.iter().rposition(|line| line.trim_start().starts_with('›'))?;
    let mut content = vec![lines[at].trim_start().trim_start_matches('›').trim()];
    content.extend(lines[at + 1..].iter().take_while(|line| !line.trim().is_empty()).map(|line| line.trim()));
    let mut content = content.join("\n");
    if content == CODEX_PLACEHOLDER {
        content.clear();
    }
    Some(ComposerSnapshot {
        content,
        placeholders: BTreeSet::new(),
        stashed: false,
    })
}
fn own_residue(current: &ComposerSnapshot, text: &str, before: &ComposerSnapshot) -> bool {
    let visible = compact(&current.content);
    let wanted = compact(text);
    if !visible.is_empty()
        && (visible == wanted || visible.chars().count() >= 12 && wanted.starts_with(&visible))
    {
        return true;
    }
    let mut rest = current.content.clone();
    for marker in PLACEHOLDER.find_iter(&current.content) {
        if let Some(end) = current.content[marker.start()..].find(']') {
            rest = rest.replace(
                &current.content[marker.start()..marker.start() + end + 1],
                "",
            );
        }
    }
    !current.placeholders.is_empty()
        && current.placeholders.is_disjoint(&before.placeholders)
        && compact(&rest).is_empty()
}
async fn cleanup_own(
    target: &OtherTarget,
    io: &dyn OtherIo,
    text: &str,
    before: Option<&ComposerSnapshot>,
) -> Cleanup {
    let Some(before) = before.filter(|before| before.is_empty()) else {
        return Cleanup::Unproved;
    };
    let mut changed = false;
    for _ in 0..12 {
        let Ok(frame) = capture(target, io).await else {
            return Cleanup::Unproved;
        };
        if blocked(&frame)
            || !pending(target, io)
                .await
                .is_ok_and(|pending| pending.is_none())
        {
            return Cleanup::Unproved;
        }
        let Some(current) = composer(target, &frame) else {
            return Cleanup::Unproved;
        };
        if current.is_empty() {
            return if changed {
                Cleanup::Proved
            } else {
                Cleanup::Unproved
            };
        }
        if !own_residue(&current, text, before) {
            return Cleanup::Unproved;
        }
        if life(target, io).await.is_err() {
            return Cleanup::Unproved;
        }
        if write_io(target, io, "C-u", false, None).await.is_err() {
            return Cleanup::Unproved;
        }
        changed = true;
        if life(target, io).await.is_err() {
            return Cleanup::Unproved;
        }
        tokio::time::sleep(PROOF_INTERVAL).await;
        if life(target, io).await.is_err() {
            return Cleanup::Unproved;
        }
    }
    match capture(target, io)
        .await
        .ok()
        .and_then(|frame| composer(target, &frame))
    {
        Some(current) if current.is_empty() => Cleanup::Proved,
        _ => Cleanup::Unproved,
    }
}
async fn before_submit_failure(
    target: &OtherTarget,
    io: &dyn OtherIo,
    text: &str,
    before: Option<&ComposerSnapshot>,
    stage: DeliveryStage,
    code: &str,
) -> DeliveryResult {
    let cleanup = cleanup_own(target, io, text, before).await;
    result(
        if cleanup == Cleanup::Proved {
            Disposition::Deferred
        } else {
            Disposition::Unknown
        },
        stage,
        cleanup,
        code,
    )
}

async fn line_by_line(target: &OtherTarget, io: &dyn OtherIo, text: &str) -> Result<(), RuntimeError> {
    let mut wrote = false;
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            write_io(target, io, "C-j", false, None).await.map_err(|error| if wrote { failure("other_terminal_delivery_unconfirmed") } else { error })?;
            wrote = true;
        }
        if !line.is_empty() {
            write_io(target, io, line, true, None).await.map_err(|error| if wrote { failure("other_terminal_delivery_unconfirmed") } else { error })?;
            wrote = true;
        }
    }
    Ok(())
}

pub async fn prompt(
    target: &OtherTarget,
    io: &dyn OtherIo,
    text: &str,
    initial: Option<&OtherFrame>,
) -> DeliveryResult {
    prompt_inner(target, io, text, initial, None).await
}

pub(super) async fn prompt_with_episode(
    target: &OtherTarget, io: &dyn OtherIo, text: &str, initial: Option<&OtherFrame>,
    composer_checked: &mut (dyn FnMut(bool) + Send),
) -> DeliveryResult {
    prompt_inner(target, io, text, initial, Some(composer_checked)).await
}

async fn prompt_inner(
    target: &OtherTarget, io: &dyn OtherIo, text: &str, initial: Option<&OtherFrame>,
    composer_checked: Option<&mut (dyn FnMut(bool) + Send)>,
) -> DeliveryResult {
    if !valid_text(text, true) || text.trim().is_empty() {
        return result(
            Disposition::Rejected,
            DeliveryStage::Validate,
            Cleanup::NotNeeded,
            "invalid_text",
        );
    }
    if life(target, io).await.is_err() {
        return result(
            Disposition::Deferred,
            DeliveryStage::Identity,
            Cleanup::NotNeeded,
            "other_terminal_life_unavailable",
        );
    }
    let mut frame = match initial {
        Some(frame) => frame.clone(),
        None => capture(target, io).await.unwrap_or(OtherFrame {
            text: String::new(),
            typed: None,
            styled: None,
            editor: None,
        }),
    };
    match pending(target, io).await {
        Ok(Some(_)) => {
            return result(
                Disposition::Deferred,
                DeliveryStage::Ready,
                Cleanup::NotNeeded,
                "question_open",
            );
        }
        Err(error) => {
            return result(
                Disposition::Deferred,
                DeliveryStage::Identity,
                Cleanup::NotNeeded,
                &error.code,
            );
        }
        _ => {}
    }
    if blocked(&frame) {
        return result(
            Disposition::Deferred,
            DeliveryStage::Ready,
            Cleanup::NotNeeded,
            "input_unavailable",
        );
    }
    // Codex cru só recebe com a caixa vazia: menu ou diálogo aberto, popup de barra ou texto de
    // alguém esperam na fila (as teclas cairiam dentro deles).
    if codex(target) {
        if crate::terminal_state::codex_menu_of(&frame.text).is_some() {
            return result(Disposition::Deferred, DeliveryStage::Ready, Cleanup::NotNeeded, "codex_menu_open");
        }
        if composer(target, &frame).is_some_and(|current| !current.is_empty()) {
            return result(Disposition::Deferred, DeliveryStage::Composer, Cleanup::NotNeeded, "composer_busy");
        }
    }
    let deadline = Instant::now() + if codex(target) { CODEX_READY_WAIT } else { Duration::from_secs(8) };
    while !ready(target, &frame) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if life(target, io).await.is_err() {
            return result(
                Disposition::Deferred,
                DeliveryStage::Identity,
                Cleanup::NotNeeded,
                "other_terminal_life_changed",
            );
        }
        frame = capture(target, io).await.unwrap_or(OtherFrame {
            text: String::new(),
            typed: None,
            styled: None,
            editor: None,
        });
        if blocked(&frame) {
            return result(
                Disposition::Deferred,
                DeliveryStage::Ready,
                Cleanup::NotNeeded,
                "input_unavailable",
            );
        }
    }
    if !ready(target, &frame)
        && READY_WARNED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((
                target.key.clone(),
                target.generation,
                target.binding.conversation.clone(),
            ))
    {
        tracing::warn!(
            code = "other_terminal_ready_timeout",
            provider = target.provider.as_str(), session = %target.name, generation = target.generation,
            "a TUI não mostrou prontidão no prazo; o envio físico continua"
        );
    }
    let before = composer(target, &frame);
    // Tela que nunca ficou pronta ou sem caixa de digitar: espera na fila em vez de digitar às cegas.
    if codex(target) && (!ready(target, &frame) || before.is_none()) {
        return result(Disposition::Deferred, DeliveryStage::Ready, Cleanup::NotNeeded, "input_unavailable");
    }
    if let Some(composer_checked) = composer_checked {
        composer_checked(before.as_ref().is_some_and(ComposerSnapshot::is_empty));
    }
    if before.as_ref().is_some_and(|before| !before.is_empty()) {
        return result(
            Disposition::Deferred,
            DeliveryStage::Composer,
            Cleanup::NotNeeded,
            "composer_busy",
        );
    }
    if life(target, io).await.is_err() {
        return result(
            Disposition::Deferred,
            DeliveryStage::Identity,
            Cleanup::NotNeeded,
            "other_terminal_life_changed",
        );
    }
    let written = match write_io(target, io, text, true, None).await {
        // Como o `paste_text` do Python: sem buffer, linha a linha com C-j.
        Err(error) if codex(target) && error.code == "buffer_failed" => line_by_line(target, io, text).await,
        written => written,
    };
    if let Err(error) = written {
        return before_submit_failure(
            target,
            io,
            text,
            before.as_ref(),
            DeliveryStage::Write,
            &error.code,
        )
        .await;
    }
    if life(target, io).await.is_err() {
        return result(
            Disposition::Unknown,
            DeliveryStage::Write,
            Cleanup::Unproved,
            "other_terminal_life_changed",
        );
    }
    let slash = text.trim_start().starts_with('/');
    tokio::time::sleep(if slash {
        SETTLE
    } else if text.contains('\n') {
        Duration::from_millis(500)
    } else {
        INPUT_SETTLE
    })
    .await;
    if life(target, io).await.is_err() {
        return result(
            Disposition::Unknown,
            DeliveryStage::InputProof,
            Cleanup::Unproved,
            "other_terminal_life_changed",
        );
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut positive = false;
    loop {
        match capture(target, io).await {
            Ok(current) => {
                if blocked(&current) && !slash {
                    return result(
                        Disposition::Unknown,
                        DeliveryStage::InputProof,
                        Cleanup::Unproved,
                        "input_unavailable",
                    );
                }
                match (composer(target, &current), before.as_ref()) {
                    (Some(current), Some(before)) => match current.proves(text, before) {
                        Proof::Present => {
                            positive = true;
                            break;
                        }
                        Proof::Unreadable => break,
                        Proof::Absent => {}
                    },
                    _ => break,
                }
            }
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            return before_submit_failure(
                target,
                io,
                text,
                before.as_ref(),
                DeliveryStage::InputProof,
                "input_unproved",
            )
            .await;
        }
        tokio::time::sleep(PROOF_INTERVAL).await;
        if life(target, io).await.is_err() {
            return result(
                Disposition::Unknown,
                DeliveryStage::InputProof,
                Cleanup::Unproved,
                "other_terminal_life_changed",
            );
        }
    }
    for submit in 0..if slash { 2 } else { 1 } {
        if !pending(target, io)
            .await
            .is_ok_and(|pending| pending.is_none())
        {
            return result(
                Disposition::Unknown,
                DeliveryStage::Submit,
                Cleanup::Unproved,
                "question_open",
            );
        }
        if life(target, io).await.is_err() {
            return result(
                Disposition::Unknown,
                DeliveryStage::Submit,
                Cleanup::Unproved,
                "other_terminal_life_changed",
            );
        }
        if let Err(error) = write_io(target, io, "Enter", false, None).await {
            return result(
                Disposition::Unknown,
                DeliveryStage::Submit,
                Cleanup::Unproved,
                &error.code,
            );
        }
        if life(target, io).await.is_err() {
            return result(
                Disposition::Unknown,
                DeliveryStage::Submit,
                Cleanup::Unproved,
                "other_terminal_life_changed",
            );
        }
        tokio::time::sleep(if slash { SETTLE } else { PROOF_INTERVAL }).await;
        if life(target, io).await.is_err() {
            return result(
                Disposition::Unknown,
                DeliveryStage::SubmitProof,
                Cleanup::Unproved,
                "other_terminal_life_changed",
            );
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let current = match capture(target, io).await {
                Ok(current) => current,
                Err(_) => {
                    return result(
                        Disposition::Unknown,
                        DeliveryStage::SubmitProof,
                        Cleanup::Unproved,
                        "submit_unproved",
                    );
                }
            };
            let Some(current) = composer(target, &current) else {
                // Codex sem composer legível: como o Python, sem prova o Enter vale.
                if codex(target) {
                    if slash && submit == 0 {
                        break;
                    }
                    return result(
                        Disposition::Accepted,
                        DeliveryStage::SubmitProof,
                        Cleanup::NotNeeded,
                        "submit_unreadable",
                    );
                }
                return result(
                    Disposition::Unknown,
                    DeliveryStage::SubmitProof,
                    Cleanup::Unproved,
                    "submit_unproved",
                );
            };
            let absent = before
                .as_ref()
                .is_some_and(|before| current.proves(text, before) == Proof::Absent)
                || current.is_empty();
            if absent {
                if slash && submit == 0 {
                    if !current.is_empty() {
                        return result(
                            if positive {
                                Disposition::Accepted
                            } else {
                                Disposition::Unknown
                            },
                            DeliveryStage::SubmitProof,
                            if positive {
                                Cleanup::NotNeeded
                            } else {
                                Cleanup::Unproved
                            },
                            "submit_proved",
                        );
                    }
                    break;
                }
                return result(
                    if positive {
                        Disposition::Accepted
                    } else {
                        Disposition::Unknown
                    },
                    DeliveryStage::SubmitProof,
                    if positive {
                        Cleanup::NotNeeded
                    } else {
                        Cleanup::Unproved
                    },
                    if positive {
                        "submit_proved"
                    } else {
                        "submit_unproved"
                    },
                );
            }
            if slash && submit == 0 {
                break;
            }
            if Instant::now() >= deadline {
                return result(
                    Disposition::Unknown,
                    DeliveryStage::SubmitProof,
                    Cleanup::Unproved,
                    "submit_unproved",
                );
            }
            tokio::time::sleep(PROOF_INTERVAL).await;
            if life(target, io).await.is_err() {
                return result(
                    Disposition::Unknown,
                    DeliveryStage::SubmitProof,
                    Cleanup::Unproved,
                    "other_terminal_life_changed",
                );
            }
        }
    }
    result(
        Disposition::Unknown,
        DeliveryStage::SubmitProof,
        Cleanup::Unproved,
        "submit_unproved",
    )
}

struct Control<'a> {
    target: &'a OtherTarget,
    io: &'a dyn OtherIo,
    store: &'a InteractionStore,
    operation: &'a str,
    expected: Option<ExpectedInteraction>,
    native: Option<PendingInteraction>,
    initial: Option<OtherFrame>,
    wrote: bool,
    submitted: bool,
}
impl Control<'_> {
    fn check(&self, closed: bool) -> Result<(), RuntimeError> {
        if let Some(expected) = &self.expected {
            if closed {
                self.store
                    .check_after_close(expected)
                    .map_err(interaction_error)
            } else {
                let current = self
                    .store
                    .expected(&expected.binding, Some(&expected.interaction_id))
                    .map_err(interaction_error)?;
                expected
                    .check(
                        &current.target,
                        &current.semantic,
                        current.source_revision.as_deref(),
                    )
                    .map_err(interaction_error)
            }
        } else {
            Ok(())
        }
    }
    async fn guard(&self, closed: bool) -> Result<(), RuntimeError> {
        self.check(closed)?;
        life(self.target, self.io).await?;
        self.check(closed)
    }
    async fn pause(&self, duration: Duration, closed: bool) -> Result<(), RuntimeError> {
        self.guard(closed).await?;
        tokio::time::sleep(duration).await;
        self.guard(closed).await
    }
    async fn frame(&mut self, first: bool, closed: bool) -> Result<OtherFrame, RuntimeError> {
        self.guard(closed).await?;
        let frame = if first {
            match self.initial.take() {
                Some(frame) => Ok(frame),
                None => self.io.capture().await,
            }
        } else {
            self.io.capture().await
        };
        self.guard(closed).await?;
        frame
    }
    async fn load_native(&mut self) -> Result<PendingInteraction, RuntimeError> {
        self.guard(false).await?;
        let latest = pending(self.target, self.io).await?;
        self.guard(false).await?;
        let latest = latest.ok_or_else(|| failure("erro_sem_pergunta_kimi"))?;
        let expected = self
            .expected
            .as_ref()
            .ok_or_else(|| interaction_error(InteractionError::Missing))?;
        expected
            .check(
                &latest.target,
                &latest.semantic,
                expected.source_revision.as_deref(),
            )
            .map_err(interaction_error)?;
        self.native = Some(latest.clone());
        Ok(latest)
    }
    async fn write(
        &mut self,
        value: &str,
        literal: bool,
        closing: bool,
    ) -> Result<(), RuntimeError> {
        self.guard(false).await?;
        if self.native.is_some() {
            self.load_native().await?;
        }
        self.check(false)?;
        let guard = self.native.as_ref().map(|pending| OtherWriteGuard {
            target: pending.target.clone(),
            semantic: pending.semantic.clone(),
            closing,
        });
        let sent = write_io(self.target, self.io, value, literal, guard.as_ref()).await;
        match sent {
            Ok(()) => {
                self.wrote = true;
                if closing {
                    self.submitted = true;
                }
            }
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "erro_interacao_desatualizada" | "other_terminal_conversation_unavailable"
                ) =>
            {
                return Err(error);
            }
            Err(error) => {
                self.wrote = true;
                if closing {
                    self.submitted = true;
                }
                return Err(error);
            }
        }
        self.guard(closing).await
    }
    fn reply(&self, disposition: RuntimeDisposition, payload: Value) -> OtherControlOutcome {
        OtherControlOutcome {
            reply: RuntimeReply {
                operation_id: self.operation.into(),
                disposition,
                payload,
            },
            fallback_text: None,
        }
    }
    fn unknown(&self, code: &str) -> OtherControlOutcome {
        self.reply(
            RuntimeDisposition::Unknown,
            json!({"ok":false,"error_code":code,"unconfirmed":true}),
        )
    }
    async fn confirm(
        &mut self,
        id: &str,
        approval: bool,
        feedback: bool,
    ) -> Result<bool, RuntimeError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (path, id) = (self.target.transcript.clone(), id.to_owned());
            let arrived = tokio::task::spawn_blocking(move || {
                if approval {
                    hangar_transcript::interactions::kimi_interaction_resolved(&path, &id)
                } else {
                    hangar_transcript::interactions::kimi_response_arrived(&path, &id)
                }
            })
            .await
            .map_err(|_| failure("other_terminal_interaction_worker"))?;
            life(self.target, self.io).await?;
            if let Some(expected) = &self.expected {
                let current = self
                    .store
                    .current(&expected.binding, &expected.target)
                    .map_err(interaction_error)?;
                if let Some(current) = current {
                    expected
                        .check(
                            &current.target,
                            &current.semantic,
                            current.source_revision.as_deref(),
                        )
                        .map_err(interaction_error)?;
                }
            }
            if arrived.unwrap_or(false) {
                return Ok(true);
            }
            self.check(true)?;
            if feedback {
                let frame = self.frame(false, true).await?;
                if frame.text.contains("Type feedback") {
                    let latest = pending(self.target, self.io).await?;
                    self.guard(true).await?;
                    if latest
                        .as_ref()
                        .zip(self.native.as_ref())
                        .is_some_and(|(a, b)| {
                            a.native_id == b.native_id && a.semantic == b.semantic
                        })
                    {
                        return Ok(true);
                    }
                }
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            self.pause(Duration::from_millis(200), true).await?;
        }
    }
}

#[derive(Clone)]
struct Step {
    value: String,
    literal: bool,
    text: bool,
}
impl Step {
    fn number(number: usize) -> Self {
        Self {
            value: number.to_string(),
            literal: true,
            text: false,
        }
    }
    fn key(key: &str) -> Self {
        Self {
            value: key.into(),
            literal: false,
            text: false,
        }
    }
    fn text(text: &str) -> Self {
        Self {
            value: text.into(),
            literal: true,
            text: true,
        }
    }
}

#[derive(Clone)]
struct Answer {
    steps: Vec<Step>,
    question: String,
    text: Option<String>,
    fallback: String,
}
fn answers(payload: &Value, questions: &[Value], kimi: bool) -> Result<Vec<Answer>, RuntimeError> {
    let supplied = payload["answers"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| failure("invalid_answers"))?;
    if supplied.len() != questions.len() {
        return Err(failure("invalid_answers"));
    }
    supplied
        .iter()
        .zip(questions)
        .map(|(answer, question)| {
            let options = question["options"].as_array().map_or(0, Vec::len);
            let title = question["question"].as_str().unwrap_or("").to_owned();
            match answer["kind"].as_str() {
                Some("option") => {
                    let indices = answer["indices"]
                        .as_array()
                        .filter(|indices| !indices.is_empty())
                        .ok_or_else(|| failure("invalid_answers"))?;
                    let indices = indices
                        .iter()
                        .map(|index| {
                            index
                                .as_u64()
                                .and_then(|index| usize::try_from(index).ok())
                                .filter(|index| *index < options)
                                .ok_or_else(|| failure("invalid_answers"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if indices.iter().copied().collect::<BTreeSet<_>>().len() != indices.len()
                        || indices.len() > 1 && (!kimi || answer["multi"] != true)
                    {
                        return Err(failure("invalid_answers"));
                    }
                    let labels = answer["labels"]
                        .as_array()
                        .map(|labels| {
                            labels
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    if !kimi && labels.trim().is_empty() {
                        return Err(failure("invalid_answers"));
                    }
                    let mut steps = indices
                        .into_iter()
                        .map(|index| {
                            let number = index + 1;
                            if kimi && number > 9 {
                                Err(failure("invalid_answers"))
                            } else {
                                Ok(Step::number(number))
                            }
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if kimi && answer["multi"] == true {
                        steps.push(Step::key("Tab"));
                    }
                    Ok(Answer {
                        steps,
                        question: title,
                        text: None,
                        fallback: labels,
                    })
                }
                Some("text") => {
                    let text = answer["value"]
                        .as_str()
                        .filter(|text| !text.trim().is_empty() && valid_text(text, false))
                        .ok_or_else(|| failure("invalid_answers"))?;
                    if kimi && options + 1 > 9 {
                        return Err(failure("invalid_answers"));
                    }
                    Ok(Answer {
                        steps: vec![
                            Step::number(options + 1),
                            Step::text(text),
                            Step::key("Enter"),
                        ],
                        question: title,
                        text: Some(text.into()),
                        fallback: text.into(),
                    })
                }
                _ => Err(failure("invalid_answers")),
            }
        })
        .collect()
}
fn same_question(frame: &OtherFrame, question: &str) -> bool {
    if question.is_empty() {
        return true;
    }
    let Some(capture) = QUESTION.captures(&frame.text) else {
        return false;
    };
    let visible = capture[1].split_whitespace().collect::<Vec<_>>().join(" ");
    let expected = question.split_whitespace().collect::<Vec<_>>().join(" ");
    let head_visible: String = visible.chars().take(40).collect();
    let head_expected: String = expected.chars().take(40).collect();
    !visible.is_empty()
        && !expected.is_empty()
        && (visible.starts_with(&head_expected) || expected.starts_with(&head_visible))
}
fn pi_cursor(target: &OtherTarget, frame: &OtherFrame) -> Option<usize> {
    if target.provider != hangar_transcript::Provider::Omp {
        return PI_CURSOR
            .captures_iter(&frame.text)
            .last()
            .and_then(|c| c[1].parse().ok());
    }
    let lines: Vec<_> = frame.text.lines().collect();
    let top = lines
        .iter()
        .rposition(|line| line.contains('╭') && line.contains('─'))
        .or_else(|| {
            lines
                .iter()
                .rposition(|line| line.contains('┌') && line.contains('─'))
        })?;
    let mut position = 0;
    for line in &lines[top + 1..] {
        if line.trim_start().starts_with(['╰', '└']) && line.contains('─') {
            break;
        }
        let Some(line) = line.trim().strip_prefix('│') else {
            continue;
        };
        let line = line.trim();
        let selected = line.strip_prefix('\u{f054}').map(str::trim_start);
        if selected.unwrap_or(line).starts_with('\u{f10c}') {
            position += 1;
            if selected.is_some() {
                return Some(position);
            }
        }
    }
    None
}
async fn fallback(
    control: &mut Control<'_>,
    answer: &Answer,
) -> Result<OtherControlOutcome, RuntimeError> {
    if answer.fallback.trim().is_empty() {
        return Err(failure("erro_drive_sem_fallback"));
    }
    let frame = control.frame(false, false).await?;
    let open = if control.target.provider == hangar_transcript::Provider::Kimi {
        kimi_picker_open(&frame.text)
    } else {
        crate::terminal_state::analyze(&frame.text).overlay
    };
    if !open {
        return Ok(control.unknown("erro_sem_confirmacao_resposta"));
    }
    control.write("Escape", false, true).await?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let frame = control.frame(false, true).await?;
        let analysis = crate::terminal_state::analyze(&frame.text);
        if !kimi_picker_open(&frame.text) && !analysis.overlay && !frame.text.trim().is_empty() {
            return Ok(OtherControlOutcome {
                reply: RuntimeReply {
                    operation_id: control.operation.into(),
                    disposition: RuntimeDisposition::Deferred,
                    payload: json!({"fallback":true,"queued":true,"delivered":false}),
                },
                fallback_text: Some(format!("Respondendo à pergunta: {}", answer.fallback)),
            });
        }
        if Instant::now() >= deadline {
            return Err(failure("erro_drive_fallback_falhou"));
        }
        control.pause(Duration::from_millis(100), true).await?;
    }
}
async fn kimi_answer(
    control: &mut Control<'_>,
    payload: &Value,
    submit_only: bool,
) -> Result<OtherControlOutcome, RuntimeError> {
    let pending = control.load_native().await?;
    if pending.target.kind != InteractionKind::Question {
        return Err(failure("erro_sem_pergunta_kimi"));
    }
    let plan = if submit_only {
        vec![]
    } else {
        answers(payload, &pending.questions, true)?
    };
    let mut frame = control.frame(true, false).await?;
    if !submit_only && !kimi_picker_open(&frame.text) {
        return Err(failure("erro_sem_pergunta_kimi"));
    }
    let driven = async {
        for (index, answer) in plan.iter().enumerate() {
            if index > 0 {
                frame = control.frame(false, false).await?;
            }
            for attempt in 0..6 {
                if same_question(&frame, &answer.question) {
                    break;
                }
                if attempt == 5 {
                    return Err(failure("other_terminal_wrong_question"));
                }
                control.pause(SETTLE, false).await?;
                frame = control.frame(false, false).await?;
            }
            for step in &answer.steps {
                let (value, literal) = (&step.value, step.literal);
                frame = control.frame(false, false).await?;
                if !kimi_picker_open(&frame.text) || !same_question(&frame, &answer.question) {
                    return Err(failure("other_terminal_wrong_question"));
                }
                let numeric = literal && !step.text;
                if numeric && frame.text.contains("type answer")
                    || literal && !numeric && !frame.text.contains("type answer")
                {
                    return Err(failure("other_terminal_wrong_input_mode"));
                }
                control.write(value, literal, false).await?;
                control
                    .pause(
                        if literal && !numeric {
                            INPUT_SETTLE
                        } else {
                            SETTLE
                        },
                        false,
                    )
                    .await?;
            }
        }
        if !submit_only {
            control.pause(OPEN_SETTLE, false).await?;
            frame = control.frame(false, false).await?;
        }
        if !frame.text.contains("Ready to submit your answers?") {
            return Err(failure("other_terminal_review_missing"));
        }
        control.write("1", true, true).await?;
        control.pause(OPEN_SETTLE, true).await?;
        Ok::<_, RuntimeError>(())
    }
    .await;
    if let Err(error) = driven {
        if control.submitted {
            return Ok(control.unknown("erro_sem_confirmacao_resposta"));
        }
        if matches!(
            error.code.as_str(),
            "other_terminal_wrong_question"
                | "other_terminal_wrong_input_mode"
                | "other_terminal_review_missing"
        ) && !plan.is_empty()
        {
            return fallback(control, &plan[0]).await;
        }
        return Err(error);
    }
    if control.confirm(&pending.native_id, false, false).await? {
        return Ok(control.reply(
            RuntimeDisposition::Accepted,
            json!({"ok":true,"fallback":false}),
        ));
    }
    let frame = control.frame(false, true).await?;
    if kimi_picker_open(&frame.text) && !plan.is_empty() {
        control.submitted = false;
        return fallback(control, &plan[0]).await;
    }
    Ok(control.unknown("erro_sem_confirmacao_resposta"))
}
async fn approval(
    control: &mut Control<'_>,
    payload: &Value,
) -> Result<OtherControlOutcome, RuntimeError> {
    let pending = control.load_native().await?;
    let option = payload["option"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n >= 1 && *n <= 9)
        .ok_or_else(|| failure("erro_opcao_fora_da_lista"))?;
    let choices = pending
        .approval
        .as_ref()
        .and_then(|a| a["escolhas"].as_array())
        .ok_or_else(|| failure("erro_sem_pergunta_kimi"))?;
    let choice = choices
        .get(option - 1)
        .ok_or_else(|| failure("erro_opcao_fora_da_lista"))?;
    let feedback = choice["requires_feedback"] == true;
    let frame = control.frame(true, false).await?;
    if !kimi_approval_open(&frame.text) || frame.text.contains("Type feedback") {
        return Err(failure("erro_sem_pergunta_kimi"));
    }
    control.write(&option.to_string(), true, true).await?;
    if control.confirm(&pending.native_id, true, feedback).await? {
        Ok(control.reply(
            RuntimeDisposition::Accepted,
            json!({"ok":true,"feedback_pendente":feedback}),
        ))
    } else {
        let frame = control.frame(false, true).await?;
        if kimi_approval_open(&frame.text) && !frame.text.contains("Type feedback") {
            control.load_native().await?;
            return Ok(control.reply(
                RuntimeDisposition::Rejected,
                json!({"ok":false,"error_code":"erro_opcao_nao_convergiu"}),
            ));
        }
        Ok(control.unknown("erro_sem_confirmacao_resposta"))
    }
}
async fn pi_answer(
    control: &mut Control<'_>,
    payload: &Value,
) -> Result<OtherControlOutcome, RuntimeError> {
    let pending = control.load_native().await?;
    if pending.target.kind != InteractionKind::Question || pending.questions.len() != 1 {
        return Err(failure("invalid_answers"));
    }
    let plan = answers(payload, &pending.questions, false)?;
    let answer = &plan[0];
    let target: usize = answer.steps[0]
        .value
        .parse()
        .map_err(|_| failure("invalid_answers"))?;
    let mut frame = control.frame(true, false).await?;
    if !crate::terminal_state::analyze(&frame.text).overlay
        || pi_cursor(control.target, &frame).is_none()
    {
        return Err(failure("other_terminal_picker_unavailable"));
    }
    for attempt in 0..4 {
        let row = pi_cursor(control.target, &frame)
            .ok_or_else(|| failure("other_terminal_cursor_unavailable"))?;
        if row == target {
            break;
        }
        if attempt == 3 {
            return fallback(control, answer).await;
        }
        for _ in 0..row.abs_diff(target) {
            control
                .write(if row < target { "Down" } else { "Up" }, false, false)
                .await?;
            control.pause(SETTLE, false).await?;
        }
        frame = control.frame(false, false).await?;
    }
    control.write("Enter", false, answer.text.is_none()).await?;
    if let Some(text) = &answer.text {
        control.pause(SETTLE, false).await?;
        control.write(text, true, false).await?;
        control.pause(INPUT_SETTLE, false).await?;
        control.write("Enter", false, true).await?;
    }
    control.pause(OPEN_SETTLE, true).await?;
    let mut after = control.frame(false, true).await?;
    if after.text.trim().is_empty() {
        control.pause(Duration::from_millis(500), true).await?;
        after = control.frame(false, true).await?;
    }
    if after.text.trim().is_empty() {
        return Ok(control.unknown("erro_sem_confirmacao_resposta"));
    }
    if crate::terminal_state::analyze(&after.text).overlay
        && pi_cursor(control.target, &after).is_some()
    {
        control.submitted = false;
        return fallback(control, answer).await;
    }
    Ok(control.reply(
        RuntimeDisposition::Accepted,
        json!({"ok":true,"fallback":false}),
    ))
}
async fn pi_select(
    control: &mut Control<'_>,
    payload: &Value,
) -> Result<OtherControlOutcome, RuntimeError> {
    if control
        .expected
        .as_ref()
        .is_some_and(|expected| expected.target.kind == InteractionKind::Menu)
    {
        return generic_select(control, payload, false).await;
    }
    let pending = control.load_native().await?;
    let option = payload["option"]
        .as_u64()
        .and_then(|option| usize::try_from(option).ok())
        .filter(|option| *option >= 1)
        .ok_or_else(|| failure("erro_opcao_fora_da_lista"))?;
    let value = pending
        .questions
        .first()
        .and_then(|question| question["options"].as_array())
        .and_then(|options| options.get(option - 1))
        .ok_or_else(|| failure("erro_opcao_fora_da_lista"))?;
    let label = value
        .as_str()
        .or_else(|| value["label"].as_str())
        .filter(|label| !label.trim().is_empty())
        .ok_or_else(|| failure("invalid_answers"))?;
    pi_answer(
        control,
        &json!({"answers":[{"kind":"option","indices":[option-1],"labels":[label]}]}),
    )
    .await
}

fn allowed_key(value: &str, interactive: bool) -> Option<&str> {
    match value {
        "Up" | "Down" | "Left" | "Right" | "Enter" | "Escape" | "Tab" | "BTab" | "Space" => {
            Some(value)
        }
        "PageUp" | "PageDown" => Some(value),
        "Backspace" | "Delete" if interactive => Some(value),
        "Home" | "End" | "C-c" | "C-d" | "C-r" | "C-u" | "C-k" | "C-w" | "C-a" | "C-e" | "C-l"
        | "C-z" | "C-p" | "C-n" | "C-b" | "C-f" | "C-g"
            if interactive =>
        {
            Some(value)
        }
        _ => None,
    }
}
async fn generic_select(
    control: &mut Control<'_>,
    payload: &Value,
    submit: bool,
) -> Result<OtherControlOutcome, RuntimeError> {
    let expected = control
        .expected
        .clone()
        .ok_or_else(|| interaction_error(InteractionError::Missing))?;
    if expected.target.kind != InteractionKind::Menu {
        return Err(failure("other_terminal_picker_unavailable"));
    }
    let options = expected.semantic["options"]
        .as_array()
        .ok_or_else(|| failure("other_terminal_picker_unavailable"))?;
    let option = if submit {
        None
    } else {
        Some(
            payload["option"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n >= 1 && *n <= options.len())
                .ok_or_else(|| failure("erro_opcao_fora_da_lista"))?,
        )
    };
    let mut frame = control.frame(true, false).await?;
    for attempt in 0..4 {
        let analysis = crate::terminal_state::analyze(&frame.text);
        let observed = menu_interaction(analysis.question, analysis.options.unwrap_or_default());
        expected
            .check(
                &observed.target,
                &observed.semantic,
                expected.source_revision.as_deref(),
            )
            .map_err(interaction_error)?;
        if submit {
            if !MULTIPLE.is_match(&frame.text) {
                return Err(failure("other_terminal_submit_unavailable"));
            }
            control.write("Right", false, false).await?;
            control.pause(Duration::from_millis(120), false).await?;
            frame = control.frame(false, false).await?;
            if !frame.text.contains("Submit answers") {
                return Err(failure("other_terminal_submit_unavailable"));
            }
            break;
        }
        let row = (if control.target.provider == hangar_transcript::Provider::Omp {
            pi_cursor(control.target, &frame)
        } else {
            CURSOR
                .captures_iter(&frame.text)
                .last()
                .and_then(|c| c[1].parse().ok())
        })
        .ok_or_else(|| failure("other_terminal_cursor_unavailable"))?;
        let option = option.unwrap();
        if row == option {
            break;
        }
        if attempt == 3 {
            return Err(failure("erro_opcao_nao_convergiu"));
        }
        for _ in 0..row.abs_diff(option) {
            control
                .write(if row < option { "Down" } else { "Up" }, false, false)
                .await?;
            control.pause(Duration::from_millis(120), false).await?;
        }
        frame = control.frame(false, false).await?;
    }
    let multiple = MULTIPLE.is_match(&frame.text) && !submit;
    control.write("Enter", false, !multiple).await?;
    control.pause(SETTLE, true).await?;
    let frame = control.frame(false, true).await?;
    if multiple && !frame.text.trim().is_empty() && MULTIPLE.is_match(&frame.text) {
        return Ok(control.reply(
            RuntimeDisposition::Accepted,
            json!({"ok":true,"submitted":false}),
        ));
    }
    if frame.text.trim().is_empty() || crate::terminal_state::analyze(&frame.text).overlay {
        return Ok(control.unknown("erro_sem_confirmacao_resposta"));
    }
    Ok(control.reply(RuntimeDisposition::Accepted, json!({"ok":true})))
}

pub async fn control(
    target: &OtherTarget,
    io: &dyn OtherIo,
    interactions: &InteractionStore,
    operation_id: &str,
    kind: &str,
    payload: &Value,
    initial: Option<&OtherFrame>,
) -> Result<OtherControlOutcome, RuntimeError> {
    let guarded = matches!(
        kind,
        "select"
            | "select_submit"
            | "select/submit"
            | "submit_selected"
            | "answer"
            | "answer_questions"
    );
    let expected = if guarded {
        Some(
            interactions
                .expected(&binding(target), payload["interaction_id"].as_str())
                .map_err(interaction_error)?,
        )
    } else {
        None
    };
    let mut context = Control {
        target,
        io,
        store: interactions,
        operation: operation_id,
        expected,
        native: None,
        initial: initial.cloned(),
        wrote: false,
        submitted: false,
    };
    let outcome = async {
        match kind {
            "select" if target.provider == hangar_transcript::Provider::Kimi => {
                approval(&mut context, payload).await
            }
            "select" => pi_select(&mut context, payload).await,
            "select_submit" | "select/submit" | "submit_selected"
                if target.provider == hangar_transcript::Provider::Kimi =>
            {
                kimi_answer(&mut context, payload, true).await
            }
            "select_submit" | "select/submit" | "submit_selected" => {
                generic_select(&mut context, payload, true).await
            }
            "answer" | "answer_questions"
                if target.provider == hangar_transcript::Provider::Kimi =>
            {
                kimi_answer(&mut context, payload, false).await
            }
            "answer" | "answer_questions" => pi_answer(&mut context, payload).await,
            "interrupt" => {
                context.write("Escape", false, false).await?;
                if payload["clear"] == true {
                    context.pause(SETTLE, false).await?;
                    context.write("Escape", false, false).await?;
                }
                Ok(context.reply(RuntimeDisposition::Accepted, json!({"ok":true})))
            }
            "keys" | "key" | "navigation_key" | "interactive_key" => {
                let key = payload["key"]
                    .as_str()
                    .and_then(|key| {
                        allowed_key(
                            key,
                            kind == "interactive_key" || payload["interactive"] == true,
                        )
                    })
                    .ok_or_else(|| failure("key_not_allowed"))?;
                context.write(key, false, false).await?;
                Ok(context.reply(RuntimeDisposition::Accepted, json!({"ok":true})))
            }
            "term_input" | "term-input" | "terminal_input" => {
                if let Some(key) = payload["key"].as_str() {
                    let key = allowed_key(key, true).ok_or_else(|| failure("key_not_allowed"))?;
                    context.write(key, false, false).await?;
                } else {
                    let text = payload["text"]
                        .as_str()
                        .filter(|text| valid_text(text, true))
                        .ok_or_else(|| failure("invalid_text"))?;
                    if !text.is_empty() {
                        context.write(text, true, false).await?;
                    } else {
                        context.guard(false).await?;
                    }
                }
                Ok(context.reply(RuntimeDisposition::Accepted, json!({"ok":true})))
            }
            "steer" | "steer_queue" | "steer_entry" => {
                let before = context.frame(true, false).await;
                if before
                    .as_ref()
                    .is_ok_and(|frame| !frame.text.contains("ctrl-s to steer immediately"))
                {
                    return Ok(context.reply(
                        RuntimeDisposition::Accepted,
                        json!({"ok":true,"steered":false,"sem_fila":true}),
                    ));
                }
                context.write("C-s", false, false).await?;
                context.pause(SETTLE, false).await?;
                let after = context.frame(false, false).await?;
                if !after.text.trim().is_empty()
                    && !after.text.contains("ctrl-s to steer immediately")
                {
                    Ok(context.reply(
                        RuntimeDisposition::Accepted,
                        json!({"ok":true,"steered":true}),
                    ))
                } else {
                    Ok(context.unknown("other_terminal_steer_unconfirmed"))
                }
            }
            _ => Err(failure("other_terminal_control_unavailable")),
        }
    }
    .await;
    match outcome {
        Ok(outcome) => Ok(outcome),
        Err(error) if context.wrote => Ok(context.unknown(if context.submitted {
            "erro_sem_confirmacao_resposta"
        } else {
            &error.code
        })),
        Err(error) => Err(error),
    }
}
