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
/// Colagem longa que o Codex recolhe na caixa: o texto não aparece, só o marcador com o tamanho.
static CODEX_PASTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[Pasted Content (\d+) chars\]").unwrap());
static PI_CURSOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*>\s*(\d+)\.").unwrap());
static AGENT_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:❯\s+)?[●◯]\s+(\S+)").unwrap());
static PI_NOTICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Subagentasyncgroupedresultintercomdeliverywasnotacknowledgedfor.*?/tmp/pi-subagents-[^']*'?\.?").unwrap()
});
/// A caixa vazia do `agy` mostra a dica do modo (`Auto mode: … (shift+tab to cycle)`), não um rascunho.
/// Até onde a caixa do `agy` pode estar do fim da tela: abaixo dela ficam o rodapé e o painel de tarefas de fundo.
const AGY_BOX_FROM_END: usize = 24;
static AGY_MODE_HINT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[A-Z][\w-]* mode: .*\(shift\+tab to cycle\)\z").unwrap());
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
const CODEX_MENU_FOOTERS: [&str; 5] = ["enter select", "enter to confirm", "enter to continue", "enter confirm", "enter continue"];

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
/// A leitura da tela pelo agente: o `agy` tem rodapé e menu próprios; os outros usam o leitor comum.
pub(crate) fn screen(provider: hangar_transcript::Provider, pane: &str) -> crate::terminal_state::PaneAnalysis {
    match provider {
        hangar_transcript::Provider::Antigravity => crate::terminal_state::agy_analyze(pane),
        // O Kimi desenha a mensagem da pessoa como `❯ texto`, no histórico e na fila da TUI: com número, `❯ 1. …` pareceria
        // o cursor de um menu. Pergunta e aprovação dele vêm do arquivo da conversa; da tela só valem os painéis abertos.
        hangar_transcript::Provider::Kimi => {
            let mut analysis = crate::terminal_state::analyze(pane);
            if analysis.state == "awaiting_input" {
                analysis.question = None;
                analysis.options = None;
                analysis.label = analysis.spinner.as_deref().map(|spinner| spinner.chars().skip(2).collect::<String>().trim().to_owned());
                analysis.state = if analysis.spinner.is_some() { "working".into() } else { String::new() };
            }
            // O painel de aprovação ou de escolha (rodapé `↵ confirm`, `↵ choose`…) ocupa a caixa: nada se digita sobre ele.
            analysis.overlay |= kimi_approval_open(pane) || kimi_picker_open(pane);
            analysis
        }
        _ => crate::terminal_state::analyze(pane),
    }
}
fn blocked(target: &OtherTarget, frame: &OtherFrame) -> bool {
    let analysis = screen(target.provider, &frame.text);
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
    composer_for(target.provider, frame)
}
/// O rascunho na caixa do `agy` (vazio = caixa vazia ou só a dica do modo); `None` sem caixa legível.
pub(crate) fn agy_draft(screen: &str) -> Option<String> {
    let frame = OtherFrame { text: screen.to_owned(), typed: None, styled: None, editor: None };
    composer_for(hangar_transcript::Provider::Antigravity, &frame).map(|composer| composer.content)
}
fn composer_for(provider: hangar_transcript::Provider, frame: &OtherFrame) -> Option<ComposerSnapshot> {
    if provider == hangar_transcript::Provider::Codex {
        return codex_composer(frame);
    }
    if matches!(
        provider,
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
    let last = *rules.last()?;
    let (top, bottom) = if provider == hangar_transcript::Provider::Antigravity {
        // O `agy` desenha o painel de tarefas em segundo plano entre linhas, abaixo da caixa: a caixa é a que abre com `>`.
        rules
            .windows(2)
            .rev()
            .map(|pair| (pair[0], pair[1]))
            // A caixa é o último par ou o de cima do painel de tarefas: um pedido antigo repetido mais acima não conta.
            .take(2)
            .take_while(|(_, bottom)| lines.len() - bottom <= AGY_BOX_FROM_END)
            .find(|(top, bottom)| *bottom > top + 1
                && lines[top + 1].trim().trim_start_matches(['│', '┃', '║']).trim_start().starts_with(['>', '❯']))?
    } else {
        (*rules.get(rules.len().checked_sub(2)?)?, last)
    };
    if lines.len() - last > 8 || bottom - top > 15 {
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
        provider,
        hangar_transcript::Provider::Pi | hangar_transcript::Provider::Omp
    ) {
        content = PI_NOTICE.replace_all(&compact(&content), "").into_owned();
    }
    if provider == hangar_transcript::Provider::Antigravity && AGY_MODE_HINT.is_match(content.trim()) {
        content.clear();
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
/// Menu ou diálogo do Codex na tela: o seletor que a lista reconhece (`codex_menu_of`) ou opções
/// numeradas com o cursor e o rodapé de escolha. O de modelo e esforço ("enter select") fica só aqui:
/// em `CODEX_FOOTERS` ele viraria pergunta para o app.
fn codex_menu_open(frame: &OtherFrame) -> bool {
    if crate::terminal_state::codex_menu_of(&frame.text).is_some() {
        return true;
    }
    // A captura traz o resto do pane em branco: o rodapé é das últimas linhas com texto.
    let footer = frame.text.trim_end().lines().rev().take(8)
        .any(|line| { let line = line.to_lowercase(); CODEX_MENU_FOOTERS.iter().any(|mark| line.contains(mark)) });
    footer && CURSOR.is_match(&frame.text)
}
/// Codex cru só recebe com a caixa vazia: com menu ou diálogo aberto, popup de barra ou texto de alguém
/// nada é digitado (as teclas cairiam dentro deles).
fn codex_busy(target: &OtherTarget, frame: &OtherFrame) -> Option<DeliveryResult> {
    if !codex(target) {
        return None;
    }
    if codex_menu_open(frame) {
        return Some(result(Disposition::Deferred, DeliveryStage::Ready, Cleanup::NotNeeded, "codex_menu_open"));
    }
    composer(target, frame).filter(|current| !current.is_empty())
        .map(|_| result(Disposition::Deferred, DeliveryStage::Composer, Cleanup::NotNeeded, "composer_busy"))
}
/// A caixa de digitar do Codex: a última linha `›` e as que continuam o texto. Parágrafo deixa linha em
/// branco dentro da caixa, então ela vai até a última linha em branco: o bloco depois dessa é a barra de
/// estado. A sugestão vem esmaecida e sai do `typed`; a captura sem estilo traz a sugestão padrão por extenso.
fn codex_composer(frame: &OtherFrame) -> Option<ComposerSnapshot> {
    let text = frame.typed.as_deref().unwrap_or(&frame.text);
    let lines: Vec<&str> = text.lines().collect();
    let at = lines.iter().rposition(|line| line.trim_start().starts_with('›'))?;
    let below = &lines[at + 1..];
    let below = &below[..below.iter().rposition(|line| !line.trim().is_empty()).map_or(0, |last| last + 1)];
    let below = &below[..below.iter().rposition(|line| line.trim().is_empty()).unwrap_or(below.len())];
    let mut content = vec![lines[at].trim_start().trim_start_matches('›').trim()];
    content.extend(below.iter().map(|line| line.trim()));
    let mut content = content.join("\n").trim_end().to_owned();
    if content == CODEX_PLACEHOLDER {
        content.clear();
    }
    let placeholders = CODEX_PASTE.captures_iter(&content).map(|found| format!("pasted{}", &found[1])).collect();
    Some(ComposerSnapshot {
        content,
        placeholders,
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
        if blocked(target, &frame)
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
    // Como o Python no Codex: o que ficou na caixa fica, e a resposta diz isso (nunca "limpo").
    if codex(target) {
        return result(Disposition::Unknown, stage, Cleanup::Unproved, code);
    }
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
    // Antes do bloqueio genérico: o menu do Codex sai com o código dele.
    if let Some(refused) = codex_busy(target, &frame) {
        return refused;
    }
    if blocked(target, &frame) {
        return result(
            Disposition::Deferred,
            DeliveryStage::Ready,
            Cleanup::NotNeeded,
            "input_unavailable",
        );
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
        if blocked(target, &frame) {
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
    // A tela pode ter mudado durante a espera: menu ou texto que abriu agora também barra.
    if let Some(refused) = codex_busy(target, &frame) {
        return refused;
    }
    let before = composer(target, &frame);
    // Tela que nunca ficou pronta ou sem caixa de digitar: recusa em vez de digitar às cegas.
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
                if blocked(target, &current) && !slash {
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
            // No Codex um Enter já roda o comando de barra. Outro só sai se o comando continua na caixa: em
            // qualquer outra tela (seletor do `/model`, lista do `/resume`, ainda carregando) ele escolheria
            // a linha sob o cursor.
            if codex(target) && slash && submit == 0 {
                let still_typed = !codex_menu_open(&current)
                    && composer(target, &current).is_some_and(|box_now| compact(&box_now.content) == compact(text));
                if still_typed {
                    break;
                }
                return result(Disposition::Accepted, DeliveryStage::SubmitProof, Cleanup::NotNeeded, "submit_proved");
            }
            let Some(current) = composer(target, &current) else {
                // Codex sem composer legível: como o Python, sem prova o Enter vale.
                if codex(target) {
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
        let analysis = screen(control.target.provider, &frame.text);
        let observed = menu_interaction(analysis.question, analysis.options.unwrap_or_default());
        expected
            .check(
                &observed.target,
                &observed.semantic,
                expected.source_revision.as_deref(),
            )
            .map_err(interaction_error)?;
        // Os Termos do Google não têm opção numerada: as teclas saem da caixa e do foco na tela.
        if !submit && control.target.provider == hangar_transcript::Provider::Antigravity
            && crate::terminal_state::agy_terms(&frame.text).is_some()
        {
            return agy_terms_answer(control, option.unwrap()).await;
        }
        // A lista do primeiro uso (cores) tem cursor sem número: a linha dele vem da leitura da tela.
        if !submit && control.target.provider == hangar_transcript::Provider::Antigravity
            && let Some((_, _, row)) = crate::terminal_state::agy_onboarding_list(&frame.text)
        {
            let want = option.unwrap() - 1;
            if row == want {
                control.write("Enter", false, true).await?;
                control.pause(SETTLE, true).await?;
                return Ok(control.reply(RuntimeDisposition::Accepted, json!({"ok":true})));
            }
            if attempt == 3 { return Err(failure("erro_opcao_nao_convergiu")); }
            for _ in 0..row.abs_diff(want) {
                control.write(if row < want { "Down" } else { "Up" }, false, false).await?;
                control.pause(Duration::from_millis(120), false).await?;
            }
            frame = control.frame(false, false).await?;
            continue;
        }
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

/// A opção que a pessoa tocou no cartão dos Termos (`AGY_TERMS_OPTIONS`): a caixa de dados como ela escolheu e
/// o botão. Uma tecla por vez, relendo a tela; o Hangar nunca escolhe pela pessoa.
async fn agy_terms_answer(control: &mut Control<'_>, option: usize) -> Result<OtherControlOutcome, RuntimeError> {
    use crate::terminal_state::{AgyTermsFocus as Focus, AgyTermsKey as Key, agy_terms, agy_terms_key};
    for _ in 0..8 {
        let frame = control.frame(false, false).await?;
        let terms = agy_terms(&frame.text).ok_or_else(|| failure("erro_interacao_desatualizada"))?;
        let key = agy_terms_key(terms, option).ok_or_else(|| failure("erro_opcao_fora_da_lista"))?;
        let key = if key == Key::Enter && terms.focus != Focus::Checkbox {
            let button = terms.focus;
            control.write("Enter", false, true).await?;
            control.pause(SETTLE, true).await?;
            let frame = control.frame(false, true).await?;
            // Captura vazia no meio do redesenho não prova que o botão funcionou.
            return Ok(if frame.text.trim().is_empty() || agy_terms(&frame.text).is_some_and(|after| after.focus == button) {
                control.unknown("erro_sem_confirmacao_resposta")
            } else {
                control.reply(RuntimeDisposition::Accepted, json!({"ok":true}))
            });
        } else {
            key.name()
        };
        control.write(key, false, false).await?;
        control.pause(Duration::from_millis(150), false).await?;
    }
    Err(failure("erro_opcao_nao_convergiu"))
}

/// O ciclo do shift+tab do `agy`, na ordem da tela; `manual` é o modo padrão dele, sem rótulo no rodapé. O
/// `bypassPermissions` é a flag `--dangerously-skip-permissions`, fora do ciclo: a rota reabre o pane para ele.
const AGY_MODES: [&str; 4] = ["auto", "plan", "manual", "acceptEdits"];
const AGY_LISTED: [&str; 5] = ["auto", "plan", "manual", "acceptEdits", "bypassPermissions"];
const AGY_MODE_WAIT: Duration = Duration::from_secs(2);
const AGY_MODE_POLL: Duration = Duration::from_millis(150);

/// Lê ou troca o modo do `agy` pelo rodapé; cada BTab só conta com o rodapé novo na tela.
async fn agy_permission(context: &mut Control<'_>, payload: &Value, set: bool) -> Result<OtherControlOutcome, RuntimeError> {
    let mode_of = |frame: &OtherFrame| crate::terminal_state::agy_footer(&frame.text).and_then(|footer| footer.mode);
    let frame = context.frame(false, false).await?;
    let Some(initial) = mode_of(&frame) else {
        return Ok(context.reply(RuntimeDisposition::Rejected, json!({"ok":false,"current":null,"error_code":"erro_permissao_leitura"})));
    };
    if !set {
        return Ok(context.reply(RuntimeDisposition::Accepted,
            json!({"ok":true,"current":initial,"modes":AGY_LISTED,"sondavel":false,"previous_non_plan":null,"uncertain":false})));
    }
    let requested = payload["mode"].as_str().unwrap_or_default();
    // A rota reabre o pane para entrar ou sair do bypass; aqui só confere que a tela permite fechá-lo.
    let reopen = payload["reopen"] == true;
    let refuse = |context: &Control<'_>, code: &str| context.reply(RuntimeDisposition::Rejected,
        json!({"ok":false,"mode":initial,"current":initial,"requested":requested,"error_code":code}));
    if requested == "bypassPermissions" && !reopen { return Ok(refuse(context, "erro_permissao_so_na_abertura")); }
    if !AGY_LISTED.contains(&requested) { return Ok(refuse(context, "erro_permissao_invalida")); }
    if requested == initial && !reopen {
        return Ok(context.reply(RuntimeDisposition::Accepted,
            json!({"ok":true,"mode":initial,"current":initial,"requested":requested,"changed":false,"previous_non_plan":null})));
    }
    // Como no Claude: a tecla seria resposta de um menu, e o rascunho é de quem está digitando.
    let analysis = crate::terminal_state::agy_analyze(&frame.text);
    if analysis.overlay || analysis.state == "awaiting_input" { return Ok(refuse(context, "permission_menu_open")); }
    if analysis.state == "working" { return Ok(refuse(context, "erro_sessao_trabalhando")); }
    match composer_for(hangar_transcript::Provider::Antigravity, &frame) {
        Some(composer) if composer.content.trim().is_empty() => {}
        Some(_) => return Ok(refuse(context, "erro_caixa_com_rascunho")),
        None => return Ok(refuse(context, "erro_permissao_leitura")),
    }
    if reopen {
        return Ok(context.reply(RuntimeDisposition::Accepted,
            json!({"ok":true,"mode":initial,"current":initial,"requested":requested,"requires_reopen":true})));
    }
    let mut current = initial;
    for _ in 0..AGY_MODES.len() {
        context.write("BTab", false, false).await?;
        let until = Instant::now() + AGY_MODE_WAIT;
        let next = loop {
            tokio::time::sleep(AGY_MODE_POLL).await;
            let frame = context.frame(false, false).await?;
            if let Some(mode) = mode_of(&frame).filter(|mode| *mode != current) { break Some(mode); }
            if Instant::now() >= until { break None; }
        };
        // Sem rodapé novo a tecla pode ter chegado ou não: outra tecla giraria às cegas.
        let Some(next) = next else {
            return Ok(context.reply(RuntimeDisposition::Unknown,
                json!({"ok":false,"current":null,"requested":requested,"uncertain":true,"error_code":"permission_key_timeout"})));
        };
        current = next;
        if current == requested {
            return Ok(context.reply(RuntimeDisposition::Accepted,
                json!({"ok":true,"mode":current,"current":current,"requested":requested,"changed":true,"previous_non_plan":null})));
        }
    }
    Ok(context.reply(RuntimeDisposition::Rejected,
        json!({"ok":false,"mode":current,"current":current,"requested":requested,"error_code":"erro_permissao_ciclo_agy"})))
}


const KIMI_MODES: [&str; 3] = ["manual", "yolo", "auto"];
const KIMI_MODE_LABELS: [&str; 3] = ["Always Ask", "Ask When Needed", "Never Ask"];

fn kimi_mode_row(line: &str) -> Option<usize> {
    let label = line.trim().trim_start_matches('❯').trim();
    KIMI_MODE_LABELS.iter().position(|name| label == *name || label.strip_prefix(name).is_some_and(|tail| tail.trim() == "← current"))
}

struct KimiPermissionMenu { selected: usize, current: usize }

fn kimi_permission_menu(pane: &str) -> Option<KimiPermissionMenu> {
    let lines: Vec<_> = pane.lines().collect();
    let title = lines.iter().rposition(|line| line.trim() == "Select permission mode")?;
    let rule = |line: &&str| line.matches('─').count() >= 10 && line.trim().chars().all(|ch| ch == '─');
    if !lines[..title].iter().rev().find(|line| !line.trim().is_empty()).is_some_and(rule) { return None; }
    let bottom = title + 1 + lines[title + 1..].iter().position(rule)?;
    // O seletor substitui a caixa: um título citado no histórico acima da caixa não é menu aberto.
    if lines[bottom + 1..].iter().any(|line| line.trim_start().starts_with('╭')) { return None; }
    let mut selected = None;
    let mut current = None;
    let mut present = [false; 3];
    for line in &lines[title + 1..bottom] {
        if let Some(index) = kimi_mode_row(line) {
            present[index] = true;
            if line.trim_start().starts_with('❯') { selected = Some(index); }
            if line.trim_end().ends_with("← current") { current = Some(index); }
        }
    }
    present.into_iter().all(|seen| seen).then_some(KimiPermissionMenu { selected: selected?, current: current? })
}

async fn close_kimi_permissions(context: &mut Control<'_>) -> Result<(), RuntimeError> {
    context.write("Escape", false, false).await?;
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        context.pause(PROOF_INTERVAL, false).await?;
        let frame = context.frame(false, false).await?;
        if kimi_permission_menu(&frame.text).is_none() && composer(context.target, &frame).is_some() { return Ok(()); }
        if Instant::now() >= until { return Err(failure("erro_permissao_leitura")); }
    }
}

async fn open_kimi_permissions(context: &mut Control<'_>) -> Result<KimiPermissionMenu, RuntimeError> {
    context.write("/permission", true, false).await?;
    context.pause(SETTLE, false).await?;
    context.write("Enter", false, false).await?;
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        context.pause(PROOF_INTERVAL, false).await?;
        let frame = context.frame(false, false).await?;
        if let Some(menu) = kimi_permission_menu(&frame.text) { return Ok(menu); }
        if Instant::now() >= until {
            // Sem o seletor, nada do comando fica para quem digitar depois: tela aberta ou o texto na caixa.
            // Escape num turno em voo o interromperia, e numa aprovação aberta a recusaria. A limpeza não
            // troca o motivo da falha.
            let analysis = screen(context.target.provider, &frame.text);
            let frame = if composer(context.target, &frame).is_none() && !matches!(analysis.state.as_str(), "working" | "awaiting_input")
                && !analysis.overlay {
                let escaped = context.write("Escape", false, false).await.is_ok() && context.pause(SETTLE, false).await.is_ok();
                if escaped { context.frame(false, false).await.ok() } else { None }
            } else { Some(frame) };
            if frame.is_some_and(|frame| composer(context.target, &frame).is_some_and(|draft| draft.content.trim() == "/permission")) {
                let _ = context.write("C-u", false, false).await;
            }
            return Err(failure("erro_permissao_leitura"));
        }
    }
}

async fn kimi_permission(context: &mut Control<'_>, payload: &Value, set: bool) -> Result<OtherControlOutcome, RuntimeError> {
    context.guard(false).await?;
    let current = context.io.kimi_permission().await?;
    if !set {
        return Ok(context.reply(RuntimeDisposition::Accepted,
            json!({"current":current,"modes":KIMI_MODES,"sondavel":false,"previous_non_plan":null})));
    }
    let requested = payload["mode"].as_str().unwrap_or_default();
    let refuse = |context: &Control<'_>, code: &str| context.reply(RuntimeDisposition::Rejected,
        json!({"current":current,"requested":requested,"error_code":code}));
    let Some(wanted) = KIMI_MODES.iter().position(|mode| *mode == requested) else {
        return Ok(refuse(context, "erro_permissao_invalida"));
    };
    if current.as_deref() == Some(requested) {
        return Ok(context.reply(RuntimeDisposition::Accepted, json!({"mode":current,"current":current,"changed":false})));
    }
    let frame = context.frame(false, false).await?;
    let analysis = screen(context.target.provider, &frame.text);
    if kimi_permission_menu(&frame.text).is_some() || analysis.overlay || analysis.state == "awaiting_input" {
        return Ok(refuse(context, "permission_menu_open"));
    }
    // O comando usa a caixa da TUI: não se mistura ao rascunho nem à entrada de um turno em voo.
    if analysis.state == "working" { return Ok(refuse(context, "erro_sessao_trabalhando")); }
    match composer(context.target, &frame) {
        Some(composer) if composer.is_empty() => {}
        Some(_) => return Ok(refuse(context, "erro_caixa_com_rascunho")),
        None => return Ok(refuse(context, "erro_permissao_leitura")),
    }
    let menu = open_kimi_permissions(context).await?;
    let selected = menu.selected;
    for _ in 0..selected.abs_diff(wanted) {
        context.write(if wanted < selected { "Up" } else { "Down" }, false, false).await?;
    }
    context.pause(INPUT_SETTLE, false).await?;
    context.write("Enter", false, false).await?;
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        context.pause(PROOF_INTERVAL, false).await?;
        let frame = context.frame(false, false).await?;
        let actual = context.io.kimi_permission().await?;
        let closed = kimi_permission_menu(&frame.text).is_none() && composer(context.target, &frame).is_some();
        if closed && actual.as_deref() == Some(requested) {
            return Ok(context.reply(RuntimeDisposition::Accepted, json!({"mode":requested,"current":requested,"changed":true})));
        }
        if closed && actual.is_none() {
            if !composer(context.target, &frame).is_some_and(|draft| draft.is_empty()) {
                return Ok(context.unknown("erro_permissao_leitura"));
            }
            // Antes da primeira mensagem ainda não há wire: o tique do seletor é a confirmação.
            let menu = open_kimi_permissions(context).await?;
            let actual = Some(KIMI_MODES[menu.current]);
            close_kimi_permissions(context).await?;
            if actual == Some(requested) {
                return Ok(context.reply(RuntimeDisposition::Accepted, json!({"mode":requested,"current":requested,"changed":true})));
            }
            return Ok(context.unknown("erro_permissao_leitura"));
        }
        if Instant::now() >= until {
            if kimi_permission_menu(&frame.text).is_some() { close_kimi_permissions(context).await?; }
            return Ok(context.unknown("erro_permissao_leitura"));
        }
    }
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
            "permission_modes" | "set_permission_mode"
                if target.provider == hangar_transcript::Provider::Antigravity =>
            {
                agy_permission(&mut context, payload, kind == "set_permission_mode").await
            }
            "permission_modes" | "set_permission_mode"
                if target.provider == hangar_transcript::Provider::Kimi =>
            {
                kimi_permission(&mut context, payload, kind == "set_permission_mode").await
            }
            "steer" | "steer_queue" | "steer_entry" => {
                // O quadro guardado pode ser de antes de a mensagem entrar na fila da TUI: relê antes de concluir.
                let waiting = |frame: &Result<OtherFrame, RuntimeError>| {
                    frame.as_ref().is_ok_and(|frame| !frame.text.contains("ctrl-s to steer immediately"))
                };
                let mut before = context.frame(true, false).await;
                for _ in 0..3 {
                    if !waiting(&before) {
                        break;
                    }
                    context.pause(SETTLE, false).await?;
                    // Releitura que falha não vira Ctrl+S às cegas: vale o último quadro lido.
                    match context.frame(false, false).await {
                        Ok(frame) => before = Ok(frame),
                        Err(_) => break,
                    }
                }
                if waiting(&before) {
                    return Ok(context.reply(
                        RuntimeDisposition::Accepted,
                        json!({"ok":true,"steered":false,"sem_fila":true}),
                    ));
                }
                context.write("C-s", false, false).await?;
                // O Kimi tira a fila depois da resposta assíncrona à orientação. Espera o sinal,
                // sem repetir C-s: a primeira tecla já pode ter sido aceita.
                let until = Instant::now() + Duration::from_secs(5);
                loop {
                    context.pause(PROOF_INTERVAL, false).await?;
                    let after = context.frame(false, false).await?;
                    if !after.text.trim().is_empty() && !after.text.contains("ctrl-s to steer immediately") {
                        break Ok(context.reply(RuntimeDisposition::Accepted, json!({"ok":true,"steered":true})));
                    }
                    if Instant::now() >= until { break Ok(context.unknown("other_terminal_steer_unconfirmed")); }
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

/// Telas reais do Codex (`tests/it/fixtures/raw_codex/`, origem em `ORIGEM.txt`).
#[cfg(test)]
mod codex_screens {
    use super::*;

    fn frame(text: &str) -> OtherFrame {
        OtherFrame { text: text.to_owned(), typed: None, styled: None, editor: None }
    }

    // Parágrafo deixa linha em branco dentro da caixa: ler só até ela fazia a prova da digitação falhar.
    #[test]
    fn composer_with_paragraphs_is_read_whole_and_proves_the_text() {
        let text = "Oi, tudo bem?\n\nSegue o plano completo.";
        let empty = codex_composer(&frame(include_str!("../../../tests/it/fixtures/raw_codex/pane_codex_idle.txt"))).unwrap();
        assert_eq!(empty.content, "");
        let typed = codex_composer(&frame(include_str!("../../../tests/it/fixtures/raw_codex/pane_codex_paragraphs.txt"))).unwrap();
        assert_eq!(typed.content, text);
        assert!(typed.proves(text, &empty) == Proof::Present);
    }

    // A caixa de uma linha continua sem a barra de estado.
    #[test]
    fn composer_of_one_line_stops_before_the_status_bar() {
        let typed = codex_composer(&frame(include_str!("../../../tests/it/fixtures/raw_codex/pane_codex_composer_text.txt"))).unwrap();
        assert!(!typed.content.is_empty() && !typed.content.contains('\n'), "{:?}", typed.content);
    }

    // Depois do Enter no `/resume` a tela não é a caixa com o comando: outro Enter retomaria uma conversa.
    #[test]
    fn resume_selector_is_not_the_composer_with_the_command() {
        for screen in [include_str!("../../../tests/it/fixtures/raw_codex/pane_codex_resume_loading.txt"),
            include_str!("../../../tests/it/fixtures/raw_codex/pane_codex_resume_list.txt")] {
            let still_typed = codex_composer(&frame(screen)).is_some_and(|now| compact(&now.content) == compact("/resume"));
            assert!(!still_typed);
        }
    }
}
