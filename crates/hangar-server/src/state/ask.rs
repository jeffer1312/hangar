//! Pergunta nativa do AskUserQuestion do Claude com terminal: o sidecar do hook PreToolUse
//! (`<config>/.hangar-askq/<sid>.json`) casado com o menu que o pane mostra. Porte de
//! `sse._ask_question_event` e `askquestion.read_pending_askq`. Falhou o casamento, o app fica com
//! os botões do menu do pane; nunca abre o stepper sobre outra pergunta.
use std::collections::{BTreeSet, HashMap};
use std::sync::{LazyLock, Mutex};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use hangar_api::ask::{AskQuestion, AskQuestionItem};
use hangar_api::state::StateEvent;
use serde::Deserialize;

/// Linhas que o TUI acrescenta a toda pergunta e que nunca estão no payload do hook, comparadas
/// sem a caixinha e sem o ponto final (a múltipla escolha mostra "Type something" sem ponto).
const TUI_EXTRAS: [&str; 2] = ["Type something", "Chat about this"];

/// `_CAIXA_MULTI` (`^\[.?\]\s*`): a caixinha da múltipla escolha é desenho, não rótulo.
fn without_box(label: &str) -> &str {
    let Some(rest) = label.strip_prefix('[') else { return label };
    let mut chars = rest.chars();
    let after = match chars.next() {
        Some(c) if c != '\n' && chars.as_str().starts_with(']') => &chars.as_str()[1..],
        _ => match rest.strip_prefix(']') {
            Some(after) => after,
            None => return label,
        },
    };
    after.trim_start()
}

fn is_tui_extra(label: &str) -> bool { TUI_EXTRAS.contains(&without_box(label).trim_end_matches('.')) }

/// O sidecar responde à pergunta que o pane mostra agora? Só em `awaiting_input`.
pub fn matches(state: &StateEvent, payload: &AskQuestion) -> bool {
    payload.questions.first().is_some_and(|first| matches_item(state, payload, first, true))
}

fn matches_item(state: &StateEvent, payload: &AskQuestion, item: &AskQuestionItem, log: bool) -> bool {
    if state.state != "awaiting_input" {
        return false;
    }
    let first: BTreeSet<&str> = item.options.iter().map(|o| o.label.as_str()).collect();
    let pane: BTreeSet<&str> = state.options.iter().flatten().filter(|o| !is_tui_extra(o)).map(|o| without_box(o)).collect();
    if first.is_empty() || pane.is_empty() {
        return false;
    }
    let has_preview = payload.questions.iter().flat_map(|q| &q.options).any(|o| !o.preview.is_empty());
    // Sem prévia, igualdade exata: a resposta vai por índice, e opção a mais no pane cairia na
    // linha errada. Com prévia o pane trunca o rótulo: prefixo numa direção só, mesma contagem
    // (rótulo curto do pane casando com um longo do sidecar não troca a permissão por outra).
    let (ok, reason) = if has_preview {
        (first.len() == pane.len() && first.iter().all(|l| pane.iter().any(|s| !s.is_empty() && l.starts_with(s))), "askq_preview_mismatch")
    } else if !first.is_subset(&pane) {
        (false, "askq_label_outside_menu")
    } else {
        (pane.is_subset(&first), "askq_pane_extra_option")
    };
    // Degradar para os botões do pane é silencioso na tela: a frequência no diário é o sinal de
    // que o TUI mudou o texto das linhas dele.
    if !ok && log && crate::warn_limit::allow(Some(&state.session), reason) {
        tracing::info!(session = state.session.as_str(), code = reason, sidecar = first.len(), pane = pane.len(), "askq: menu não casa, fica com os botões do pane");
    }
    ok
}

/// Pergunta que quebra em linhas: o pane só dá a última delas, então basta caber no texto do sidecar.
fn same_question(pane: Option<&str>, item: &str) -> bool {
    let compact = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    pane.map(compact).is_some_and(|pane| !pane.is_empty() && compact(item).contains(&pane))
}

fn later_step(event: &StateEvent, question: &AskQuestion) -> bool {
    question.questions.iter().skip(1)
        .any(|item| same_question(event.question.as_deref(), &item.question) && matches_item(event, question, item, false))
}

fn question_target() -> crate::interactions::InteractionTarget {
    crate::interactions::InteractionTarget { kind: crate::interactions::InteractionKind::Question, request_key: "terminal:askq".into() }
}

/// O sidecar responde por este menu: a primeira etapa, ou uma seguinte do formulário já publicado.
/// O resto (outra pergunta, rótulo que não casa, etapa sem a primeira) fica com os botões do pane.
fn holds(interactions: &crate::interactions::InteractionStore, binding: &crate::interactions::InteractionBinding,
    question: &AskQuestion, revision: &str, event: &StateEvent) -> Result<bool, String> {
    if matches(event, question) { return Ok(true); }
    if !later_step(event, question) { return Ok(false); }
    Ok(interactions.current(binding, &question_target()).map_err(|e| e.code().to_owned())?
        .is_some_and(|published| published.source_revision.as_deref() == Some(revision)))
}

/// `holds` para o menu já retratado (`interaction_pane`), que é o que o escritor confere.
pub fn holds_pane(interactions: &crate::interactions::InteractionStore, binding: &crate::interactions::InteractionBinding,
    jsonl: &Path, pane: &serde_json::Value) -> Result<bool, String> {
    let Some((question, revision)) = read_revision(jsonl)? else { return Ok(false) };
    holds(interactions, binding, &question, &revision, &pane_event(pane))
}

fn pane_event(pane: &serde_json::Value) -> StateEvent {
    let options = pane["options"].as_array().map(|o| o.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect());
    StateEvent { state: "awaiting_input".into(), question: pane["question"].as_str().map(str::to_owned), options, ..Default::default() }
}

/// O menu retratado é a etapa `index` do formulário, pelo mesmo casamento da publicação (prefixo
/// quando há prévia) e, depois da primeira, pelo texto da pergunta.
pub fn pane_is_step(questions: &serde_json::Value, index: usize, pane: &serde_json::Value) -> bool {
    let Ok(items) = serde_json::from_value::<Vec<AskQuestionItem>>(questions.clone()) else { return false };
    let Some(item) = items.get(index) else { return false };
    let event = pane_event(pane);
    let form = AskQuestion { questions: items.clone(), interaction_id: None };
    matches_item(&event, &form, item, false) && (index == 0 || same_question(event.question.as_deref(), &item.question))
}

/// `<config>/projects/<cwd>/<sid>.jsonl` → `<config>/.hangar-askq/<sid>.json`.
pub fn sidecar_path(jsonl: &Path) -> Option<PathBuf> {
    let config = jsonl.parent()?.parent()?.parent()?;
    Some(config.join(crate::list::facts_files::ASKQ_DIR).join(format!("{}.json", jsonl.file_stem()?.to_str()?)))
}

#[derive(Deserialize)]
struct Sidecar { tool_input: ToolInput, #[serde(default)] tool_use_id: Option<String> }

#[derive(Deserialize)]
struct ToolInput { questions: Vec<AskQuestionItem> }

/// Pergunta pendente do sidecar. Ausente é o caso normal (`Ok(None)`); `Err` é o código de um
/// arquivo que existe e não serve (contrato do hook quebrado), que vai ao diário.
pub fn read_pending(jsonl: &Path) -> Result<Option<AskQuestion>, String> {
    Ok(read_revision(jsonl)?.map(|(question, _)| question))
}

static CONSUMED: LazyLock<Mutex<HashMap<PathBuf, BTreeSet<String>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Sinais do plugin: chamadas do AskUserQuestion que terminaram (`/ask-fim`, vença o app ou o terminal)
/// e quando começou a vida atual de cada conversa (o `/pull` de uma instância nova). O sidecar não
/// diz quando a pergunta acabou; o transcript fica só como reserva sem o plugin.
#[derive(Default)]
struct PluginSignals { ended: BTreeSet<String>, lives: HashMap<String, f64> }
static SIGNALS: LazyLock<Mutex<PluginSignals>> = LazyLock::new(Default::default);

/// A chamada `tool_use_id` do AskUserQuestion acabou no plugin.
pub fn call_ended(tool_use_id: &str) {
    if tool_use_id.is_empty() { return; }
    let Ok(mut signals) = SIGNALS.lock() else { return };
    // ponytail: ids únicos por chamada; acima de 512 recomeça (só volta a valer a reserva do transcript).
    if signals.ended.len() > 512 { signals.ended.clear(); }
    signals.ended.insert(tool_use_id.into());
}

/// Instância do plugin (`<ms em base 36>-…`) puxando pela conversa `session_id`: a vida dela começou ali.
pub fn life_started(session_id: &str, instance: &str) {
    let Some(started) = instance.split('-').next().and_then(|stamp| u64::from_str_radix(stamp, 36).ok()).map(|ms| ms as f64 / 1000.0) else { return };
    let Ok(mut signals) = SIGNALS.lock() else { return };
    if signals.lives.len() > 512 { signals.lives.clear(); }
    let life = signals.lives.entry(session_id.into()).or_insert(started);
    *life = life.max(started);
}

/// O plugin já encerrou esta pergunta: a chamada terminou, ou o sidecar é de uma vida anterior da
/// conversa (o Claude morreu nela e a conversa voltou, sem `PostToolUse`).
pub fn ended_by_plugin(session_id: &str, tool_use_id: Option<&str>, written: f64) -> bool {
    call_ended_by_plugin(tool_use_id) || older_than_life(session_id, written)
}

fn call_ended_by_plugin(tool_use_id: Option<&str>) -> bool {
    SIGNALS.lock().is_ok_and(|signals| tool_use_id.is_some_and(|call| signals.ended.contains(call)))
}

/// Sidecar escrito antes da vida atual da conversa começar. Não é prova definitiva: um worker do plugin
/// que reinicia com a pergunta aberta também parece vida nova.
fn older_than_life(session_id: &str, written: f64) -> bool {
    SIGNALS.lock().is_ok_and(|signals| signals.lives.get(session_id).is_some_and(|started| written + 1.0 < *started))
}

/// Não apaga o caminho: ele pode já guardar a pergunta seguinte.
pub fn consume_revision(jsonl: &Path, revision: &str) -> Result<(), String> {
    let path = sidecar_path(jsonl).ok_or("askq_path")?;
    CONSUMED.lock().map_err(|_| "askq_consumed_lock")?.entry(path).or_default().insert(revision.into());
    Ok(())
}

/// Até onde o transcript já foi lido à procura da resposta de cada revisão: só o que cresceu é relido.
static SCANNED: LazyLock<Mutex<HashMap<(PathBuf, String), (u64, bool)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// `(o transcript tem o tool_result da chamada, a própria chamada já apareceu)`. Lê só linhas
/// inteiras desde a última leitura.
fn answered_in_transcript(jsonl: &Path, call: &str, revision: &str) -> Result<(bool, bool), String> {
    use std::io::{Read, Seek, SeekFrom};
    let key = (jsonl.to_path_buf(), revision.to_owned());
    let (start, seen_before) = SCANNED.lock().map_err(|_| "askq_scan_lock")?.get(&key).copied().unwrap_or((0, false));
    let mut buf = Vec::new();
    let read = std::fs::File::open(jsonl).and_then(|mut file| { file.seek(SeekFrom::Start(start))?; file.read_to_end(&mut buf) });
    match read {
        Ok(_) => {}
        // Sem transcript a chamada nunca chegou a ele: o prazo do órfão decide.
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok((false, seen_before)),
        // Ilegível não prova nada: a pergunta segue pendente, e o menu que não é ela segue respondível.
        Err(e) => {
            if crate::warn_limit::allow(jsonl.file_stem().and_then(|s| s.to_str()), "askq_transcript_unreadable") {
                tracing::warn!(transcript = %jsonl.display(), code = "askq_transcript_unreadable", io_kind = ?e.kind(), "askq: transcript ilegível, pergunta segue pendente");
            }
            return Ok((false, true));
        }
    }
    let complete = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let contains = |line: &[u8], needle: &str| line.windows(needle.len()).any(|w| w == needle.as_bytes());
    let needle = format!("\"tool_use_id\":\"{call}\"");
    // Só um bloco `tool_result` da chamada prova a resposta; outra entrada com o mesmo par (anexo, progresso) não.
    let found = buf[..complete].split(|&b| b == b'\n').filter(|line| contains(line, &needle)).any(|line| {
        serde_json::from_slice::<serde_json::Value>(line).ok().is_some_and(|entry| entry["message"]["content"].as_array()
            .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result" && b["tool_use_id"] == call)))
    });
    let call_seen = seen_before || contains(&buf[..complete], &format!("\"id\":\"{call}\""));
    let mut scanned = SCANNED.lock().map_err(|_| "askq_scan_lock")?;
    // ponytail: uma entrada por revisão; acima de 64 recomeça do zero (só custa uma releitura).
    if scanned.len() > 64 { scanned.clear(); }
    scanned.insert(key, (start + complete as u64, call_seen));
    Ok((found, call_seen))
}

pub fn read_revision(jsonl: &Path) -> Result<Option<(AskQuestion, String)>, String> {
    let path = sidecar_path(jsonl).ok_or("askq_path")?;
    let mut consumed = CONSUMED.lock().map_err(|_| "askq_consumed_lock")?;
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == ErrorKind::NotFound => { consumed.remove(&path); return Ok(None); },
        Err(e) => return Ok(unusable(&path, "askq_unreadable", Some(e.kind()))),
    };
    let stamp = file_stamp(&metadata)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => { consumed.remove(&path); return Ok(None); },
        Err(e) => return Ok(unusable(&path, "askq_unreadable", Some(e.kind()))),
    };
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    digest.update(stamp.as_bytes());
    digest.update(&bytes);
    let revision: String = digest.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
    // A publicação nunca combina a leitura de A com a revisão de B.
    match std::fs::read(&path) {
        Ok(current) if current == bytes && std::fs::metadata(&path).ok().and_then(|m| file_stamp(&m).ok()).as_ref() == Some(&stamp) => {},
        Ok(_) => return Err("askq_changed_during_read".into()),
        Err(e) if e.kind() == ErrorKind::NotFound => { consumed.remove(&path); return Ok(None); },
        Err(e) => return Ok(unusable(&path, "askq_unreadable", Some(e.kind()))),
    }
    if consumed.get(&path).is_some_and(|revisions| revisions.contains(&revision)) { return Ok(None); }
    // ponytail: `multiSelect` como texto ("true") o pydantic aceita e o serde recusa; o resultado
    // é o mesmo de um sidecar quebrado (fica o menu do pane), e o hook grava o booleano.
    let Ok(sidecar) = serde_json::from_slice::<Sidecar>(&bytes) else { return Ok(unusable(&path, "askq_malformed", None)) };
    drop(consumed);
    // O hook não apaga o sidecar quando a resposta vem pelo terminal: sem isto a pergunta velha
    // prenderia todo menu seguinte (plano, permissão) em askq_pane_mismatch. Só a resposta achada
    // no transcript tira a pergunta; dúvida a mantém aberta.
    // ponytail: sidecar sem tool_use_id (o hook sempre grava) não é conferido.
    // Órfão: a chamada nunca chegou ao transcript passada a carência; a lista o expira igual.
    let age = metadata.modified().ok().and_then(|m| m.elapsed().ok()).map_or(0.0, |d| d.as_secs_f64());
    let written = metadata.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64());
    // O plugin viu a chamada acabar: é o sinal do próprio Claude Code, e vale de vez.
    if call_ended_by_plugin(sidecar.tool_use_id.as_deref()) {
        CONSUMED.lock().map_err(|_| "askq_consumed_lock")?.entry(path).or_default().insert(revision);
        return Ok(None);
    }
    // Sidecar de antes da vida atual: fora enquanto ela durar, sem consumir a revisão (um worker do plugin
    // reiniciado com a pergunta aberta também parece vida nova) e com registro.
    let sid = jsonl.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    if older_than_life(sid, written) {
        if crate::warn_limit::allow(Some(sid), "askq_previous_life") {
            tracing::info!(session = sid, code = "askq_previous_life", "askq: sidecar de uma vida anterior da conversa, fora da pergunta aberta");
        }
        return Ok(None);
    }
    if let Some(call) = sidecar.tool_use_id.as_deref()
        && let (answered, call_seen) = answered_in_transcript(jsonl, call, &revision)?
        && (answered || !call_seen && age >= crate::list::facts_files::ASK_GRACE) {
        if !answered { tracing::info!(transcript = %jsonl.display(), code = "askq_orphan", "askq: chamada fora do transcript, sidecar expirado"); }
        CONSUMED.lock().map_err(|_| "askq_consumed_lock")?.entry(path).or_default().insert(revision);
        return Ok(None);
    }
    Ok(Some((AskQuestion { questions: sidecar.tool_input.questions, interaction_id: None }, revision)))
}

/// Sidecar que existe e não serve (hook antigo, formato do `tool_input` mudado): sem pergunta, e o menu
/// do pane segue respondível; só o diário o mostra. Só o código: o serde citaria o texto da pergunta.
fn unusable(path: &Path, code: &'static str, io_kind: Option<ErrorKind>) -> Option<(AskQuestion, String)> {
    // Por sessão: uma conversa com o sidecar torto não cala o aviso das outras.
    let session = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
    if crate::warn_limit::allow(Some(session), code) {
        tracing::warn!(session, sidecar = %path.display(), code, io_kind = ?io_kind, "askq: sidecar inutilizável, fica o menu do pane");
    }
    None
}

fn file_stamp(metadata: &std::fs::Metadata) -> Result<String, String> {
    let modified = metadata.modified().map_err(|_| "askq_revision_missing")?;
    let mut stamp = format!("{:?}:{:?}:{}", modified, metadata.created().ok(), metadata.len());
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        stamp.push_str(&format!(":{}:{}", metadata.dev(), metadata.ino()));
    }
    Ok(stamp)
}

/// Aba de envio do formulário: o cursor vivo (o último da tela) está em "Submit answers".
pub(crate) fn submit_tab(pane: &str) -> bool {
    pane.lines().filter(|line| line.trim_start().starts_with(['❯', '›'])).last().is_some_and(|line| line.contains("Submit answers"))
}

pub fn observe_menu(interactions: &crate::interactions::InteractionStore,
    binding: &crate::interactions::InteractionBinding, jsonl: &Path, pane: &str) -> Result<Option<String>, String> {
    // A aba de envio é o mesmo menu sendo enviado: retratá-la trocaria o ID que o envio confere.
    if submit_tab(pane) { return Ok(None); }
    let target = crate::interactions::InteractionTarget { kind: crate::interactions::InteractionKind::Menu, request_key: "terminal:menu".into() };
    let analysis = crate::terminal_state::analyze(pane);
    let event = StateEvent { state: analysis.state.clone(), question: analysis.question.clone(), options: analysis.options.clone(), ..Default::default() };
    let held = match read_revision(jsonl)? {
        Some((question, revision)) if analysis.state == "awaiting_input" => holds(interactions, binding, &question, &revision, &event)?,
        _ => false,
    };
    let semantic = if analysis.state == "awaiting_input" && !held {
        Some(crate::terminal_input::interaction_semantic(pane, None).map_err(|e| e.code().to_owned())?)
    } else { None };
    interactions.observe(binding, &target, semantic).map_err(|e| e.code().to_owned())
}

pub fn observe_screen_menu(plugin: &crate::plugin::Store, binding: &crate::interactions::InteractionBinding,
    jsonl: &Path, pane: &str) -> Result<(Option<String>, Option<serde_json::Map<String, serde_json::Value>>), String> {
    let interactions = plugin.interactions();
    let mut notice = plugin.plan_notice(binding).map_err(|e| e.0.to_owned())?;
    if let Some(call) = notice.as_ref().map(|n| n.tool_use_id.clone())
        && answered_in_transcript(jsonl, &call, &format!("plan:{call}"))?.0 {
        plugin.drop_plan(binding, &call).map_err(|e| e.0.to_owned())?;
        interactions.observe(binding, &plan_target(&call), None).map_err(|e| e.code().to_owned())?;
        notice = None;
    }
    let analysis = crate::terminal_state::analyze(pane);
    if submit_tab(pane) || !crate::terminal_state::plan_dialog(&analysis) {
        // Plano pendente e um menu que a âncora não reconhece (o CLI mudou a frase): nunca vira menu comum.
        if notice.is_some() && analysis.state == "awaiting_input" && !submit_tab(pane) { return Err("plan_dialog_unrecognized".into()); }
        if let Some(n) = &notice { interactions.observe(binding, &plan_target(&n.tool_use_id), None).map_err(|e| e.code().to_owned())?; }
        return Ok((observe_menu(interactions, binding, jsonl, pane)?, None));
    }
    // O diálogo do plano nunca vira menu comum: sem o aviso do plugin, é falha visível.
    let menu = crate::interactions::InteractionTarget { kind: crate::interactions::InteractionKind::Menu, request_key: "terminal:menu".into() };
    interactions.observe(binding, &menu, None).map_err(|e| e.code().to_owned())?;
    let n = notice.ok_or("plan_notice_missing")?;
    let mut semantic = crate::terminal_input::interaction_semantic(pane, None).map_err(|e| e.code().to_owned())?;
    semantic["tool_use_id"] = serde_json::json!(n.tool_use_id);
    let id = interactions.observe(binding, &plan_target(&n.tool_use_id), Some(semantic)).map_err(|e| e.code().to_owned())?;
    let plan = serde_json::json!({"plan": n.plan, "path": n.path, "tool_use_id": n.tool_use_id});
    Ok((id, plan.as_object().cloned()))
}

/// Tipo `Menu`: a resposta é a tecla no diálogo do pane, conferida pelo mesmo escritor dos menus.
pub fn plan_target(tool_use_id: &str) -> crate::interactions::InteractionTarget {
    crate::interactions::plugin_target(crate::interactions::InteractionKind::Menu, &format!("plan:{tool_use_id}"))
}

/// Mesmo quadro e semântica que o escritor reconfere antes do primeiro efeito. Única regra para a
/// publicação (lista e monitor) e para os fatos do executor.
pub fn observe_terminal(interactions: &crate::interactions::InteractionStore,
    binding: &crate::interactions::InteractionBinding, jsonl: &Path, pane: &str) -> Result<Option<AskQuestion>, String> {
    let target = question_target();
    let pending = read_revision(jsonl)?;
    let Some((mut question, revision)) = pending else {
        interactions.observe_revision(binding, &target, None, None).map_err(|e| e.code().to_owned())?;
        return Ok(None);
    };
    let analysis = crate::terminal_state::analyze(pane);
    let submit = submit_tab(pane);
    // Sem menu na tela a pergunta não está aberta: o hook nunca apaga o sidecar respondido.
    if analysis.state != "awaiting_input" && !submit { return Ok(None); }
    let event = StateEvent { state: analysis.state, question: analysis.question, options: analysis.options, ..Default::default() };
    // Texto da pergunta junto: com opções iguais às da primeira, só ele separa as etapas.
    if submit || later_step(&event, &question) {
        // Etapa seguinte (ou envio) do formulário já publicado: o mesmo ID, sem retratar de novo,
        // porque o retrato desta etapa trocaria o ID que está sendo respondido. Sem a primeira
        // publicada (servidor reiniciado no meio), a etapa fica com os botões do pane.
        let Some(published) = interactions.current(binding, &target).map_err(|e| e.code().to_owned())?
            .filter(|published| published.source_revision.as_deref() == Some(revision.as_str())) else {
            if !submit && crate::warn_limit::allow(Some(&binding.key), "askq_step_unpublished") {
                tracing::info!(key = binding.key.as_str(), code = "askq_step_unpublished", "askq: etapa do formulário sem a primeira publicada, fica com os botões do pane");
            }
            return Ok(None);
        };
        if read_revision(jsonl)?.as_ref().map(|(_, current)| current) != Some(&revision) { return Err("askq_changed_during_read".into()); }
        question.interaction_id = Some(published.interaction_id);
        return Ok(Some(question));
    }
    // Menu que não é esta pergunta: fica com os botões do pane (`observe_menu`), nunca o stepper.
    if !matches(&event, &question) { return Ok(None); }
    let semantic = crate::terminal_input::interaction_semantic(pane, Some(&question)).map_err(|e| e.code().to_owned())?;
    if read_revision(jsonl)?.as_ref().map(|(_, current)| current) != Some(&revision) { return Err("askq_changed_during_read".into()); }
    question.interaction_id = interactions.observe_revision(binding, &target, Some(semantic), Some(revision))
        .map_err(|e| e.code().to_owned())?;
    Ok(Some(question))
}

/// Código estático da falha da pergunta, para a causa chegar ao resultado da entrega.
pub fn failure_code(code: &str) -> &'static str {
    const KNOWN: [&str; 9] = ["askq_pane_mismatch", "askq_changed_during_read", "askq_malformed", "askq_path",
        "askq_consumed_lock", "askq_revision_missing", "askq_unreadable", "askq_transcript_unreadable", "askq_scan_lock"];
    let base = code.split(':').next().unwrap_or(code);
    KNOWN.into_iter().find(|known| *known == base)
        .or_else(|| crate::interactions::InteractionError::from_code(code).map(|error| error.code()))
        .unwrap_or("terminal_question_unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn source_revision_is_file_revision_not_poll_and_same_content_republication_is_new() {
        let tmp = tempfile::tempdir().unwrap();
        let jsonl = tmp.path().join("projects/p/sid.jsonl");
        let path = sidecar_path(&jsonl).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = br#"{"tool_input":{"questions":[{"question":"Cor?","header":"Cor","options":[],"multiSelect":false}]}}"#;
        std::fs::write(&path, bytes).unwrap();
        let (_, first) = read_revision(&jsonl).unwrap().unwrap();
        assert_eq!(read_revision(&jsonl).unwrap().unwrap().1, first);
        consume_revision(&jsonl, &first).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(999))).unwrap();
        let (_, second) = read_revision(&jsonl).unwrap().unwrap();
        assert_ne!(first, second);
        assert_eq!(read_revision(&jsonl).unwrap().unwrap().1, second);
    }

    #[test]
    fn matches_menu_python_golden() {
        // Captura fixa, sem gerador: mudança de contrato edita o JSON junto com este teste.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/golden/ask_question.json");
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(rows.len() >= 15);
        let dir = tempfile::tempdir().unwrap();
        for row in &rows {
            let name = row["name"].as_str().unwrap();
            let state = match row["pane"].as_str() {
                Some(pane) => {
                    let a = crate::terminal_state::analyze(pane);
                    assert_eq!(serde_json::to_value(&a.options).unwrap(), row["options"], "{name}: menu do pane");
                    StateEvent { state: a.state, options: a.options, ..Default::default() }
                }
                None => StateEvent { state: row["state"].as_str().unwrap().into(),
                    options: serde_json::from_value(row["options"].clone()).unwrap(), ..Default::default() },
            };
            let jsonl = dir.path().join(name).join("projects/p/sid.jsonl");
            let askq = sidecar_path(&jsonl).unwrap();
            std::fs::create_dir_all(askq.parent().unwrap()).unwrap();
            match &row["sidecar"] {
                Value::Null => {}
                Value::String(raw) => std::fs::write(&askq, raw).unwrap(),
                other => std::fs::write(&askq, other.to_string()).unwrap(),
            }
            let event = read_pending(&jsonl).ok().flatten()
                .filter(|p| matches(&state, p))
                .map(|p| serde_json::to_value(p).unwrap());
            assert_eq!(event.unwrap_or(Value::Null), row["expected"], "{name}");
        }
    }

    const STEP_ONE: &str = "   Cor?\n\n ❯ 1. Azul\n   2. Verde\n   3. Type something.\n   4. Chat about this\n";
    const STEP_TWO: &str = "   Pasta?\n\n ❯ 1. A\n   2. B\n   3. Type something.\n   4. Chat about this\n";
    const SUBMIT: &str = "Review your answers\n → Azul\n → A\n\n ❯ 1. Submit answers\n   2. Cancel\n";
    const OTHER: &str = "   Outra?\n\n ❯ 1. X\n   2. Y\n";

    fn bound_store() -> (crate::interactions::InteractionStore, crate::interactions::InteractionBinding) {
        let store = crate::interactions::InteractionStore::default();
        let binding = crate::interactions::InteractionBinding { key: "k".into(), conversation: "sid".into(), generation: 1 };
        store.bind(&binding).unwrap();
        (store, binding)
    }

    #[test]
    fn later_step_of_the_same_form_keeps_the_published_id_and_replacement_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let jsonl = tmp.path().join("projects/p/sid.jsonl");
        let path = sidecar_path(&jsonl).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"tool_input":{"questions":[
            {"question":"Cor?","header":"Cor","multiSelect":false,"options":[{"label":"Azul"},{"label":"Verde"}]},
            {"question":"Pasta?","header":"Pasta","multiSelect":false,"options":[{"label":"A"},{"label":"B"}]}]}}"#).unwrap();
        let (store, binding) = bound_store();
        let published = observe_terminal(&store, &binding, &jsonl, STEP_ONE).unwrap().unwrap().interaction_id.unwrap();
        let expected = store.expected(&binding, Some(&published)).unwrap();
        // Segunda etapa e aba de envio são o mesmo formulário: lista, monitor e executor recebem a
        // pergunta com o mesmo ID, e o registro não muda.
        for pane in [STEP_ONE, STEP_TWO, SUBMIT] {
            let seen = observe_terminal(&store, &binding, &jsonl, pane).unwrap().unwrap();
            assert_eq!((seen.interaction_id.as_deref(), seen.questions.len()), (Some(published.as_str()), 2));
            assert_eq!(store.expected(&binding, Some(&published)).unwrap(), expected);
        }
        // Pergunta fechada (composer de volta, sidecar ainda no disco): nada aberto, registro intacto.
        assert_eq!(observe_terminal(&store, &binding, &jsonl, "────────────────\n❯ \n────────────────\n"), Ok(None));
        assert_eq!(store.expected(&binding, Some(&published)).unwrap(), expected);
        // Etapa 2 sem a primeira publicada (servidor reiniciado no meio): sem stepper, e os botões
        // do pane respondem, como no Python.
        let (fresh, fresh_binding) = bound_store();
        assert_eq!(observe_terminal(&fresh, &fresh_binding, &jsonl, STEP_TWO), Ok(None));
        assert!(observe_menu(&fresh, &fresh_binding, &jsonl, STEP_TWO).unwrap().is_some());
        // Menu fora do formulário: não é a pergunta, fica com os botões do pane.
        assert_eq!(observe_terminal(&store, &binding, &jsonl, OTHER), Ok(None));
        assert!(observe_menu(&store, &binding, &jsonl, OTHER).unwrap().is_some());
        // A etapa 2 do formulário publicado não responde como menu comum.
        assert_eq!(observe_menu(&store, &binding, &jsonl, STEP_TWO), Ok(None));
        assert_eq!(failure_code("askq_unreadable:PermissionDenied"), "askq_unreadable");
        assert_eq!(failure_code("algo_novo"), "terminal_question_unavailable");
        // Troca real A→B: B é outro formulário, com outra revisão e outro ID; o de A deixa de valer.
        std::fs::write(&path, r#"{"tool_input":{"questions":[
            {"question":"Pasta?","header":"Pasta","multiSelect":false,"options":[{"label":"A"},{"label":"B"}]}]}}"#).unwrap();
        assert_ne!(read_revision(&jsonl).unwrap().map(|(_, revision)| revision), expected.source_revision);
        let replaced = observe_terminal(&store, &binding, &jsonl, STEP_TWO).unwrap().unwrap().interaction_id.unwrap();
        assert_ne!(replaced, published);
        assert!(store.expected(&binding, Some(&published)).is_err());
    }

    #[test]
    fn sidecar_answered_in_the_transcript_does_not_hold_the_next_menu() {
        const PLAN: &str = "   Claude has written up a plan and is ready to execute. Would you like to proceed?\n\n   ❯ 1. Yes, and use auto mode\n     2. Yes, manually approve edits\n     3. Tell Claude what to change\n";
        let tmp = tempfile::tempdir().unwrap();
        let jsonl = tmp.path().join("projects/p/sid.jsonl");
        let path = sidecar_path(&jsonl).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(jsonl.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"tool_use_id":"toolu_cor","tool_input":{"questions":[
            {"question":"Cor?","header":"Cor","multiSelect":false,"options":[{"label":"Azul"},{"label":"Verde"}]}]}}"#).unwrap();
        let ask = r#"{"timestamp":"2099-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"toolu_cor","name":"AskUserQuestion"}]}}"#;
        let (store, binding) = bound_store();
        const COR: &str = "   Cor?\n\n ❯ 1. Azul\n   2. Verde\n   3. Type something.\n   4. Chat about this\n";
        // Sem transcript nada prova a resposta: a pergunta segue pendente para o menu dela, e o menu
        // de outra pergunta segue respondível pelos botões do pane.
        assert_eq!(observe_terminal(&store, &binding, &jsonl, PLAN), Ok(None));
        assert!(observe_menu(&store, &binding, &jsonl, PLAN).unwrap().is_some());
        assert!(observe_terminal(&store, &binding, &jsonl, COR).unwrap().is_some());
        // Pergunta sem resposta (o Claude morreu nela e a conversa voltou): o menu seguinte responde.
        std::fs::write(&jsonl, format!("{ask}\n")).unwrap();
        assert_eq!(observe_terminal(&store, &binding, &jsonl, PLAN), Ok(None));
        assert!(observe_menu(&store, &binding, &jsonl, PLAN).unwrap().is_some());
        // Só um bloco `tool_result` prova a resposta: outra entrada com o mesmo par não.
        let append = |line: &str| { use std::io::Write; writeln!(std::fs::OpenOptions::new().append(true).open(&jsonl).unwrap(), "{line}").unwrap() };
        append(r#"{"timestamp":"2099-01-01T00:00:01Z","type":"progress","data":{"tool_use_id":"toolu_cor"}}"#);
        assert!(observe_terminal(&store, &binding, &jsonl, COR).unwrap().is_some());
        // Respondida no terminal (o hook não apaga o sidecar): o plano é um menu comum, com identidade.
        append(r#"{"timestamp":"2099-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_cor"}]}}"#);
        assert_eq!(observe_terminal(&store, &binding, &jsonl, PLAN), Ok(None));
        assert!(observe_menu(&store, &binding, &jsonl, PLAN).unwrap().is_some());
        assert_eq!(observe_terminal(&store, &binding, &jsonl, COR), Ok(None));
    }

    #[test]
    fn the_plugin_ends_the_question_without_the_transcript() {
        const COR: &str = "   Cor?\n\n ❯ 1. Azul\n   2. Verde\n";
        let tmp = tempfile::tempdir().unwrap();
        let sidecar = |sid: &str, call: &str| {
            let jsonl = tmp.path().join(format!("projects/p/{sid}.jsonl"));
            let path = sidecar_path(&jsonl).unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::create_dir_all(jsonl.parent().unwrap()).unwrap();
            // A chamada está no transcript e não tem resposta: só o plugin pode dizer que acabou.
            std::fs::write(&jsonl, format!("{{\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"id\":\"{call}\"}}]}}}}\n")).unwrap();
            std::fs::write(&path, format!(r#"{{"tool_use_id":"{call}","tool_input":{{"questions":[
                {{"question":"Cor?","header":"Cor","multiSelect":false,"options":[{{"label":"Azul"}},{{"label":"Verde"}}]}}]}}}}"#)).unwrap();
            (jsonl, path)
        };
        let (store, binding) = bound_store();
        // `/ask-fim` da chamada: vença o terminal ou o app, a pergunta acabou.
        let (jsonl, _) = sidecar("sid-fim", "toolu_fim");
        assert!(observe_terminal(&store, &binding, &jsonl, COR).unwrap().is_some());
        call_ended("toolu_fim");
        assert_eq!(observe_terminal(&store, &binding, &jsonl, COR), Ok(None));
        // Vida nova da conversa (o Claude morreu na pergunta e voltou): o sidecar de antes não vale.
        let (jsonl, path) = sidecar("sid-vida", "toolu_vida");
        let before = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_times(std::fs::FileTimes::new().set_modified(before)).unwrap();
        assert!(observe_terminal(&store, &binding, &jsonl, COR).unwrap().is_some());
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let mut stamp = String::new(); let mut n = now;
        while n > 0 { stamp.insert(0, char::from_digit((n % 36) as u32, 36).unwrap()); n /= 36; }
        life_started("sid-vida", &format!("{stamp}-x"));
        assert_eq!(observe_terminal(&store, &binding, &jsonl, COR), Ok(None));
        // Pergunta feita nesta vida segue aberta.
        let (jsonl, _) = sidecar("sid-vida", "toolu_nova");
        assert!(observe_terminal(&store, &binding, &jsonl, COR).unwrap().is_some());
    }

    #[test]
    fn preview_label_cut_differently_by_the_pane_keeps_the_pane_buttons() {
        let tmp = tempfile::tempdir().unwrap();
        let jsonl = tmp.path().join("projects/p/sid.jsonl");
        let path = sidecar_path(&jsonl).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"tool_input":{"questions":[{"question":"Layout?","header":"L","multiSelect":false,
            "options":[{"label":"Grade","preview":"x"},{"label":"Lista","preview":"y"}]}]}}"#).unwrap();
        let (store, binding) = bound_store();
        let menu = "   Layout?\n\n ❯ 1. Grelha\n   2. Lista\n";
        assert_eq!(observe_terminal(&store, &binding, &jsonl, menu), Ok(None));
        assert!(observe_menu(&store, &binding, &jsonl, menu).unwrap().is_some());
    }

    #[test]
    fn orphan_sidecar_whose_call_never_reached_the_transcript_expires_after_the_grace() {
        const MENU: &str = "   Cor?\n\n ❯ 1. Azul\n   2. Verde\n";
        let tmp = tempfile::tempdir().unwrap();
        let jsonl = tmp.path().join("projects/p/sid.jsonl");
        let path = sidecar_path(&jsonl).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(jsonl.parent().unwrap()).unwrap();
        std::fs::write(&jsonl, "{\"timestamp\":\"2099-01-01T00:00:00Z\",\"type\":\"user\"}\n").unwrap();
        std::fs::write(&path, r#"{"tool_use_id":"toolu_orfa","tool_input":{"questions":[
            {"question":"Cor?","header":"Cor","multiSelect":false,"options":[{"label":"Azul"},{"label":"Verde"}]}]}}"#).unwrap();
        let (store, binding) = bound_store();
        // Recém-gravado, a chamada ainda pode estar a caminho do transcript: o menu é a pergunta.
        assert!(observe_terminal(&store, &binding, &jsonl, MENU).unwrap().is_some());
        assert_eq!(observe_menu(&store, &binding, &jsonl, MENU), Ok(None));
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_times(std::fs::FileTimes::new().set_modified(old)).unwrap();
        assert_eq!(observe_terminal(&store, &binding, &jsonl, MENU), Ok(None));
        assert!(observe_menu(&store, &binding, &jsonl, MENU).unwrap().is_some());
    }

    #[test]
    fn menu_without_sidecar_keeps_its_identity_on_the_submit_tab() {
        let tmp = tempfile::tempdir().unwrap();
        let jsonl = tmp.path().join("projects/p/sid.jsonl");
        let (store, binding) = bound_store();
        let menu = "   Quais?\n\n ❯ 1. [ ] Alfa\n   2. [x] Bravo\n";
        let id = observe_menu(&store, &binding, &jsonl, menu).unwrap().unwrap();
        let expected = store.expected(&binding, Some(&id)).unwrap();
        assert_eq!((expected.target.kind.clone(), expected.source_revision.clone()), (crate::interactions::InteractionKind::Menu, None));
        assert_eq!(observe_menu(&store, &binding, &jsonl, SUBMIT), Ok(None));
        assert_eq!(store.expected(&binding, Some(&id)).unwrap(), expected);
        assert_ne!(observe_menu(&store, &binding, &jsonl, OTHER).unwrap().unwrap(), id);
    }

    const PLAN: &str = include_str!("../../tests/fixtures/pane_plan_dialog.txt");

    #[test]
    fn plan_dialog_publishes_the_plugin_plan_id_never_the_pane_menu() {
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("projects/p/sid.jsonl");
        std::fs::create_dir_all(jsonl.parent().unwrap()).unwrap();
        std::fs::write(&jsonl, "").unwrap();
        let binding = crate::interactions::InteractionBinding { key: "k".into(), conversation: "sid".into(), generation: 1 };
        let store = crate::plugin::Store::new(std::sync::Arc::new(crate::interactions::InteractionStore::default()));
        store.interactions().bind(&binding).unwrap();
        store.bind(&binding).unwrap();
        let menu = crate::interactions::InteractionTarget { kind: crate::interactions::InteractionKind::Menu, request_key: "terminal:menu".into() };

        // Diálogo do plano sem o aviso do plugin: falha visível, e o menu do pane não ganha ID.
        assert_eq!(observe_screen_menu(&store, &binding, &jsonl, PLAN), Err("plan_notice_missing".to_owned()));
        assert!(store.interactions().current(&binding, &menu).unwrap().is_none());

        store.notice_plan(&binding, "toolu_1".into(), "# Plano".into(), Some("/p.md".into())).unwrap();
        let (id, plan) = observe_screen_menu(&store, &binding, &jsonl, PLAN).unwrap();
        let id = id.unwrap();
        assert_eq!(plan.unwrap(), *json!({"plan":"# Plano","path":"/p.md","tool_use_id":"toolu_1"}).as_object().unwrap());
        let expected = store.interactions().expected(&binding, Some(&id)).unwrap();
        assert_eq!(expected.target, crate::interactions::plugin_target(crate::interactions::InteractionKind::Menu, "plan:toolu_1"));
        assert_eq!(expected.semantic["pane"]["options"], json!(["Yes, and use auto mode", "Yes, manually approve edits", "Tell Claude what to change"]));
        assert!(store.interactions().current(&binding, &menu).unwrap().is_none());
        assert_eq!(observe_screen_menu(&store, &binding, &jsonl, PLAN).unwrap().0.as_deref(), Some(id.as_str()));

        // Plano pendente e um menu que a âncora não reconhece: erro, nunca menu comum respondível.
        const OTHER: &str = "   Outra?\n\n ❯ 1. X\n   2. Y\n";
        assert_eq!(observe_screen_menu(&store, &binding, &jsonl, OTHER), Err("plan_dialog_unrecognized".to_owned()));
        assert!(store.interactions().current(&binding, &menu).unwrap().is_none());

        // A resposta no transcript encerra o aviso: o mesmo quadro não revive o cartão velho.
        std::fs::write(&jsonl, "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_1\"}]}}\n").unwrap();
        assert_eq!(observe_screen_menu(&store, &binding, &jsonl, PLAN), Err("plan_notice_missing".to_owned()));
    }

    #[test]
    fn box_and_extras() {
        assert_eq!(without_box("[ ] Alfa"), "Alfa");
        assert_eq!(without_box("[x]Bravo"), "Bravo");
        assert_eq!(without_box("[]  C"), "C");
        assert_eq!(without_box("[]] D"), "D");
        assert_eq!(without_box("[ab] E"), "[ab] E");
        assert!(is_tui_extra("[ ] Type something.") && is_tui_extra("Chat about this"));
        assert!(!is_tui_extra("Type something else"));
    }

    #[test]
    fn missing_and_broken_sidecar_leave_the_pane_menu_answerable() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("projects/p/sid.jsonl");
        assert_eq!(read_pending(&jsonl), Ok(None));
        let askq = sidecar_path(&jsonl).unwrap();
        std::fs::create_dir_all(askq.parent().unwrap()).unwrap();
        // Hook antigo ou `tool_input` mudado: sem pergunta, como no Python, e o menu segue respondível.
        for broken in ["{", r#"{"tool_input":{"questions":"x"}}"#] {
            std::fs::write(&askq, broken).unwrap();
            assert_eq!(read_pending(&jsonl), Ok(None));
            let (store, binding) = bound_store();
            assert_eq!(observe_terminal(&store, &binding, &jsonl, OTHER), Ok(None));
            assert!(observe_menu(&store, &binding, &jsonl, OTHER).unwrap().is_some());
        }
        // `questions` vazio derruba o Python (`questions[0]`); aqui só não abre o stepper.
        let empty = AskQuestion { questions: Vec::new(), interaction_id: None };
        assert!(!matches(&StateEvent { state: "awaiting_input".into(), options: Some(vec!["A".into()]), ..Default::default() }, &empty));
    }
}
