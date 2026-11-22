use std::collections::{HashMap, HashSet};
use serde_json::{Value, json};
use crate::{api::dto::{AskItem, AskOption, AskPayload, ChatEvent}, delivery::SessionKey};

#[derive(Clone, Debug, PartialEq)]
pub struct PlanReview {
    pub tool_id: String,
    pub plan: String,
    pub path: Option<String>,
    pub file_candidates: Vec<String>,
}

/// A aprovação é da chamada atual, nunca de uma resposta antiga que também mencione um plano.
pub fn plan_review(events: &[ChatEvent], state: &crate::api::dto::SessionState, provider: &str) -> Option<PlanReview> {
    if provider != "claude" || state.state != "awaiting_input" || state.question.is_none()
        || state.options.as_ref().is_none_or(|options| options.is_empty()) { return None; }
    let pending = state.claude_plan_pending.as_ref();
    let pending_id = pending.and_then(|p| p.get("tool_use_id")).and_then(Value::as_str).filter(|id| !id.is_empty());
    let text = |value: Option<&Value>| value.and_then(Value::as_str).filter(|s| !s.trim().is_empty()).map(str::to_owned);
    if let Some(id) = pending_id {
        if events.iter().any(|e| e.kind == "tool_result" && e.tool_use_id.as_deref() == Some(id)) { return None; }
        let matching = events.iter().rposition(|e| e.kind == "tool_use" && e.tool_name.as_deref() == Some("ExitPlanMode") && e.tool_use_id.as_deref() == Some(id));
        if matching.is_some_and(|at| events[at + 1..].iter().any(|e| e.kind == "user_msg" && !e.id.starts_with("queued-"))) { return None; }
        let matching = matching.map(|at| &events[at]);
        return Some(PlanReview { tool_id: id.to_owned(), file_candidates: confirmed_plan_paths(events, id),
            plan: text(pending.and_then(|p| p.get("plan"))).or_else(|| text(matching.and_then(|e| e.tool_input.as_ref()?.get("plan")))).unwrap_or_default(),
            path: text(pending.and_then(|p| p.get("path"))).or_else(|| text(matching.and_then(|e| e.tool_input.as_ref()?.get("planFilePath")))) });
    }
    let question = state.question.as_deref()?.trim();
    if pending.is_none() && !(question.starts_with("Claude has written up a plan") || question == "Would you like to proceed?" || question == "Ready to code?") { return None; }
    let mut answered = HashSet::new();
    for event in events.iter().rev() {
        if event.kind == "user_msg" && !event.id.starts_with("queued-") { break; }
        if event.kind == "tool_result" { if let Some(id) = event.tool_use_id.as_deref() { answered.insert(id); } }
        if event.kind == "tool_use" && event.tool_name.as_deref() == Some("ExitPlanMode") {
            let id = event.tool_use_id.as_deref().filter(|id| !id.is_empty() && !answered.contains(id))?;
            return Some(PlanReview { tool_id: id.to_owned(), file_candidates: confirmed_plan_paths(events, id),
                plan: text(pending.and_then(|p| p.get("plan"))).or_else(|| text(event.tool_input.as_ref().and_then(|i| i.get("plan")))).unwrap_or_default(),
                path: text(pending.and_then(|p| p.get("path"))).or_else(|| text(event.tool_input.as_ref().and_then(|i| i.get("planFilePath")))) });
        }
    }
    let pending = pending?;
    // Sem id nem histórico a fonte de estado ainda é autoritativa; uma chamada antiga conhecida não é reaproveitada.
    if events.iter().any(|e| e.tool_name.as_deref() == Some("ExitPlanMode")) { return None; }
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(pending).unwrap_or_default().hash(&mut hash);
    Some(PlanReview { tool_id: format!("pending:{:x}", hash.finish()), plan: text(pending.get("plan")).unwrap_or_default(),
        path: text(pending.get("path")), file_candidates: Vec::new() })
}

fn confirmed_plan_paths(events: &[ChatEvent], exit_id: &str) -> Vec<String> {
    let mut completed = HashSet::new();
    let mut paths = Vec::new();
    let mut before_exit = false;
    for event in events.iter().rev() {
        if event.kind == "user_msg" && !event.id.starts_with("queued-") { break; }
        if event.kind == "tool_use" && event.tool_use_id.as_deref() == Some(exit_id) { before_exit = true; continue; }
        if !before_exit { continue; }
        if event.kind == "tool_result" && event.is_error != Some(true) {
            if let Some(id) = event.tool_use_id.as_deref() { completed.insert(id); }
        }
        if event.kind == "tool_use" && matches!(event.tool_name.as_deref(), Some("Write" | "Edit"))
            && event.tool_use_id.as_deref().is_some_and(|id| completed.contains(id)) {
            if let Some(path) = event.tool_input.as_ref().and_then(|i| i.get("file_path")).and_then(Value::as_str)
                && path.to_ascii_lowercase().ends_with(".md") && !paths.iter().any(|known| known == path) { paths.push(path.to_owned()); }
        }
    }
    paths
}

/// Abrevia só as escolhas conhecidas da TUI; o texto completo continua disponível no botão.
pub fn plan_choice_key(option: &str) -> Option<&'static str> {
    match option {
        "Yes, and bypass permissions" | "Yes, and switch to BYPASS PERMISSIONS (no further prompts) for this session" => Some("plan_review_bypass"),
        "Yes, and use auto mode" => Some("plan_review_auto_mode"),
        "Yes, manually approve edits" => Some("plan_review_manual"),
        "Yes, and auto-accept edits" => Some("plan_review_auto"),
        "Tell Claude what to change" => Some("plan_review_changes"),
        "No, keep planning" => Some("plan_review_keep"),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pick { Empty, Options(Vec<usize>), Text(String), Chat }

#[derive(Clone, Debug, PartialEq)]
pub struct Ask { pub payload: AskPayload, pub fingerprint: String, pub tool_use_id: Option<String> }

impl Ask {
    pub fn new(payload: AskPayload, raw: &Value) -> Self { Self { payload, fingerprint: raw.to_string(), tool_use_id: None } }
    pub fn codex(&self) -> bool { self.payload.provider.as_deref() == Some("codex") }
    pub fn allows_text(&self, item: &AskItem) -> bool { !self.codex() || item.is_other || item.options.is_empty() }
    pub fn allows_chat(&self) -> bool { !self.codex() }
}

/// Pi, omp e Kimi não mandam `ask_question`: a pergunta pendente é o último `tool_use` da ferramenta
/// sem `tool_result` (mesma regra do `pendingPiQuestion` do web). Formato inesperado → None.
pub fn ask_from_events(events: &[ChatEvent], provider: &str) -> Option<Ask> {
    let tool = match provider { "pi" => "question", "omp" => "ask", "kimi" => "AskUserQuestion", _ => return None };
    let mut answered = HashSet::new();
    let mut last = None;
    for event in events {
        match (event.kind.as_str(), event.tool_use_id.as_deref()) {
            ("tool_result", Some(id)) => { answered.insert(id); }
            ("tool_use", Some(_)) if event.tool_name.as_deref() == Some(tool) => last = Some(event),
            _ => {}
        }
    }
    let event = last.filter(|e| !answered.contains(e.tool_use_id.as_deref().unwrap_or("")))?;
    let input = event.tool_input.as_ref()?;
    let text = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_owned();
    let options = |v: Option<&Value>| v.and_then(Value::as_array).map(|list| list.iter().map(|o| AskOption {
        label: text(o.get("label")), description: text(o.get("description")), preview: None,
    }).filter(|o| !o.label.is_empty()).collect()).unwrap_or_else(Vec::new);
    let questions: Vec<AskItem> = if provider == "pi" {
        let item = AskItem { header: text(input.get("header")), question: text(input.get("question")),
            multi_select: input.get("multiSelect") == Some(&Value::Bool(true)), options: options(input.get("options")), ..Default::default() };
        vec![item]
    } else {
        let list = input.get("questions").and_then(Value::as_array)?;
        // O picker do omp mostra uma por vez e o backend só dirige answers[0].
        let take = if provider == "omp" { 1 } else { list.len() };
        list.iter().take(take).map(|q| AskItem {
            header: text(q.get("header")), question: text(q.get("question")),
            multi_select: q.get("multi_select") == Some(&Value::Bool(true)), options: options(q.get("options")), ..Default::default()
        }).collect()
    };
    let questions: Vec<AskItem> = questions.into_iter().filter(|q| !q.question.is_empty() && !q.options.is_empty()).collect();
    if questions.is_empty() { return None; }
    let id = event.tool_use_id.clone()?;
    Some(Ask { fingerprint: format!("tool:{id}:{}", serde_json::to_string(input).unwrap_or_default()), payload: AskPayload { provider: None, request_id: None, interaction_id: None, is_async: false, questions }, tool_use_id: Some(id) })
}

pub fn toggle(item: &AskItem, pick: &Pick, index: usize) -> Pick {
    if !item.multi_select { return Pick::Options(vec![index]); }
    let mut chosen = match pick { Pick::Options(chosen) => chosen.clone(), _ => Vec::new() };
    if let Some(at) = chosen.iter().position(|&i| i == index) { chosen.remove(at); } else { chosen.push(index); }
    if chosen.is_empty() { Pick::Empty } else { Pick::Options(chosen) }
}

/// Corpo exato de POST /answer; `None` enquanto alguma pergunta está sem resposta.
pub fn answer_body(ask: &Ask, picks: &[Pick]) -> Option<Value> {
    let questions = &ask.payload.questions;
    if questions.is_empty() || picks.len() != questions.len() { return None; }
    let answers = questions.iter().zip(picks).map(|(item, pick)| answer(ask, item, pick)).collect::<Option<Vec<_>>>()?;
    let mut body = json!({"answers": answers});
    if let Some(id) = &ask.payload.request_id { body["request_id"] = id.clone(); }
    if let Some(id) = &ask.payload.interaction_id { body["interaction_id"] = json!(id); }
    Some(body)
}

/// Corpo do `/question/skip`: o pedido e a identidade da pergunta exibida, que o servidor exige.
pub fn skip_body(ask: &Ask) -> Option<Value> {
    let mut body = json!({"request_id": ask.payload.request_id.clone()?});
    if let Some(id) = &ask.payload.interaction_id { body["interaction_id"] = json!(id); }
    Some(body)
}

/// Resposta de uma pergunta; `None` enquanto a escolha não vale.
pub fn answer(ask: &Ask, item: &AskItem, pick: &Pick) -> Option<Value> {
    let mut answer = match pick {
        Pick::Options(chosen) if !chosen.is_empty() && chosen.iter().all(|&i| i < item.options.len()) => json!({
            "kind": "option", "indices": chosen, "multi": item.multi_select,
            "labels": chosen.iter().map(|&i| item.options[i].label.clone()).collect::<Vec<_>>(),
        }),
        Pick::Text(value) if !value.trim().is_empty() && ask.allows_text(item) => {
            let value = value.trim();
            json!({"kind": "text", "value": value, "type_index": item.options.len(), "labels": [value]})
        }
        Pick::Chat if ask.allows_chat() => json!({"kind": "chat", "chat_index": item.options.len() + 1}),
        _ => return None,
    };
    if let Some(id) = &item.id { answer["question_id"] = json!(id); }
    Some(answer)
}

/// Parêntese do rótulo que pede para escrever: "Canal (digite qual)", "Email (enter address)".
/// Em inglês com espaço depois do verbo: "(enterprise)" e "(write-only)" são parte do nome.
const WRITE_ASKS: [&str; 10] = ["(digite", "(informe", "(descreva", "(escreva", "(diga",
    "(enter ", "(specify", "(describe ", "(write ", "(tell "];

/// A opção que pede o complemento no campo livre, como o agente costuma escrever: "Canal (digite qual)",
/// "Outro monitor — você diz qual no campo de texto", "informe a chave em Other".
pub fn asks_for_text(option: &AskOption) -> bool {
    let label = option.label.to_lowercase();
    let both = format!("{} {}", option.label, option.description);
    let lower = both.to_lowercase();
    // O campo "Other" só conta com maiúscula: "in other regions" é descrição comum, não pedido de escrita.
    WRITE_ASKS.iter().any(|k| label.contains(k))
        || ["em Other", "no Other", "in Other"].iter().any(|k| both.contains(k))
        || ["campo outro", "campo de texto", "campo livre", "digite no campo", "escreva no campo", "informe no campo",
            "the text field", "the free text field", "the free-text field"].iter().any(|k| lower.contains(k))
}

/// O começo da resposta escrita a partir da opção: o nome dela e dois-pontos. O parêntese do fim só sai quando é o
/// pedido de escrita; "Servidor (produção)" fica inteiro.
pub fn text_prefix(option: &AskOption) -> String {
    let label = option.label.trim();
    let base = match label.rfind(" (") {
        Some(at) if label.ends_with(')') && WRITE_ASKS.iter().any(|k| label[at + 1..].to_lowercase().starts_with(k)) => &label[..at],
        _ => label,
    };
    format!("{}: ", base.trim())
}

/// Seletor com as linhas "Type something." e "Chat about this": o do AskUserQuestion, mas também menus do próprio
/// Claude Code (mods). Quem decide se o card nativo responde é `Chat::ask_pane`.
pub fn ask_picker(options: &[String]) -> bool {
    // Prefixo, não igualdade: o pane pode trazer a coluna da direita (prévia, diff) grudada na mesma linha.
    options.iter().any(|option| {
        let label = checkbox(option).map_or(option.as_str(), |(_, rest)| rest).trim_start();
        label.starts_with("Type something") || label.starts_with("Chat about this")
    })
}

/// O seletor de múltipla escolha do terminal só se revela pela caixinha no rótulo.
pub fn checkbox(label: &str) -> Option<(bool, &str)> {
    let rest = label.strip_prefix('[')?;
    let mut chars = rest.char_indices();
    let (mark, close) = match chars.next()? {
        (_, ']') => (false, 0),
        (_, ch) => match chars.next()? { (at, ']') => (!ch.is_whitespace(), at), _ => return None },
    };
    Some((mark, rest[close + 1..].trim_start()))
}

/// Porta de `proposedPlan` (core): conteúdo entre `<proposed_plan>` em linhas próprias, fora de cerca.
pub fn proposed_plan(text: &str) -> Option<String> {
    let found = plan_markers(text);
    let close = found.iter().rev().find(|m| m.2)?;
    let open = found.iter().rev().find(|m| !m.2 && m.0 < close.0);
    let plan = text[open.map(|m| m.1).unwrap_or(0)..close.0].trim();
    (!plan.is_empty()).then(|| plan.to_owned())
}

/// Porta de `planDisplayText` (core): as marcas do protocolo não aparecem.
pub fn plan_display(text: &str) -> String {
    let mut text = text.to_owned();
    if let Some((start, end)) = memory_citation(&text) { text.replace_range(start..end, ""); }
    for (start, end, _) in plan_markers(&text).into_iter().rev() { text.replace_range(start..end, ""); }
    text
}

fn memory_citation(text: &str) -> Option<(usize, usize)> {
    let (mut offset, mut fence, mut start) = (0usize, String::new(), None);
    for line in text.split('\n') {
        if fence.is_empty() && (start.is_some() || line.trim_start().starts_with("<oai-mem-citation>")) {
            let start = *start.get_or_insert(offset);
            if let Some(close) = line.find("</oai-mem-citation>") {
                return Some((start, offset + close + "</oai-mem-citation>".len()));
            }
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let body = &line[indent..];
        let run = |ch: char| body.chars().take_while(|&c| c == ch).count();
        let delimiter = if indent <= 3 && run('`') >= 3 { Some("`".repeat(run('`'))) }
            else if indent <= 3 && run('~') >= 3 { Some("~".repeat(run('~'))) } else { None };
        if let Some(value) = delimiter {
            if fence.is_empty() { fence = value; }
            else if value.starts_with(&fence[..1]) && value.len() >= fence.len() { fence.clear(); }
        }
        offset += line.len() + 1;
    }
    start.map(|start| (start, text.len()))
}

fn plan_markers(text: &str) -> Vec<(usize, usize, bool)> {
    let mut found = Vec::new();
    let (mut offset, mut fence) = (0usize, String::new());
    for line in text.split('\n') {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let body = &line[indent..];
        let run = |ch: char| body.chars().take_while(|&c| c == ch).count();
        let delimiter = if indent <= 3 && run('`') >= 3 { Some("`".repeat(run('`'))) }
            else if indent <= 3 && run('~') >= 3 { Some("~".repeat(run('~'))) } else { None };
        if let Some(value) = delimiter {
            if fence.is_empty() { fence = value; }
            else if value.starts_with(&fence[..1]) && value.len() >= fence.len() { fence.clear(); }
        } else if fence.is_empty() && matches!(line.trim(), "<proposed_plan>" | "</proposed_plan>") {
            found.push((offset, offset + line.len(), line.contains("</")));
        }
        offset += line.len() + 1;
    }
    found
}

#[derive(Clone, Debug, PartialEq)]
/// `SelectText`: a escolha que leva o texto digitado junto (o "o que mudar" do plano). `Queue*`: ações do painel "Na fila",
/// pelo id cru da entrada; `QueueReorder` leva o `before_id` (`None` é o fim).
pub enum Action { Answer, Skip, Select(usize), SelectText(usize, String), Submit, Cancel, Steer, Discard(String), Implement,
    QueueCancel(String), QueueEdit(String, String), QueueSteer(String), QueueReorder(String, Option<String>) }

impl Action {
    pub fn path(&self) -> Vec<&str> {
        match self {
            Action::Answer => vec!["answer"], Action::Skip => vec!["question", "skip"], Action::Select(_) | Action::SelectText(..) => vec!["select"], Action::Submit => vec!["select", "submit"],
            Action::Cancel => vec!["interrupt"], Action::Steer => vec!["steer"], Action::Discard(id) => vec!["queue", id],
            Action::Implement => vec!["codex", "plan", "implement"],
            Action::QueueCancel(id) | Action::QueueEdit(id, _) => vec!["queue", id], Action::QueueSteer(id) => vec!["queue", id, "steer"],
            Action::QueueReorder(..) => vec!["queue", "reorder"],
        }
    }
    // Acima da espera do backend (35 s no Codex) para não declarar incerto o que ainda roda lá.
    pub fn seconds(&self) -> u64 { if matches!(self, Action::Answer) { 60 } else { 30 } }

    pub fn method(&self) -> reqwest::Method {
        match self {
            Action::Discard(_) | Action::QueueCancel(_) => reqwest::Method::DELETE,
            Action::QueueEdit(..) => reqwest::Method::PATCH,
            _ => reqwest::Method::POST,
        }
    }

    /// O X do painel só tira a que espera (`only_waiting`): entrega incerta nunca vira desistência por ele.
    pub fn query(&self) -> Option<(&'static str, &'static str)> {
        matches!(self, Action::QueueCancel(_)).then_some(("only_waiting", "true"))
    }

    /// Corpo das ações do painel da fila; `before_id` nulo é o fim da fila.
    pub fn queue_body(&self) -> Option<Value> {
        match self {
            Action::QueueEdit(_, text) => Some(json!({"text": text})),
            Action::QueueReorder(id, before) => Some(json!({"id": id, "before_id": before})),
            _ => None,
        }
    }

    /// Id da entrada da fila em que a ação do painel age.
    pub fn queue_entry(&self) -> Option<&str> {
        match self {
            Action::QueueCancel(id) | Action::QueueEdit(id, _) | Action::QueueSteer(id) | Action::QueueReorder(id, _) => Some(id),
            _ => None,
        }
    }
}

/// Uma mutação de interação por sessão; outra sessão segue livre.
#[derive(Default)]
pub struct InFlight { actions: HashMap<SessionKey, (Action, String)> }

impl InFlight {
    pub fn begin(&mut self, key: SessionKey, action: Action, snapshot: String) -> bool {
        if self.actions.contains_key(&key) { return false; }
        self.actions.insert(key, (action, snapshot));
        true
    }
    pub fn busy(&self, key: &SessionKey) -> bool { self.actions.contains_key(key) }
    pub fn running(&self, key: &SessionKey) -> Option<&Action> { self.actions.get(key).map(|(action, _)| action) }
    pub fn finish(&mut self, key: &SessionKey, action: &Action, snapshot: &str) -> Option<String> {
        if self.actions.get(key).is_none_or(|(current, pending)| current != action || pending != snapshot) { return None; }
        self.actions.remove(key).map(|(_, snapshot)| snapshot)
    }
}

/// URLs `http(s)://` do texto do cartão, sem pontuação final e sem o que está entre crases.
pub fn question_links(question: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, chunk) in question.split('`').enumerate() {
        if i % 2 == 1 { continue; }
        let mut rest = chunk;
        while let Some(at) = rest.find("http://").into_iter().chain(rest.find("https://")).min() {
            let tail = &rest[at..];
            let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
            let url = tail[..end].trim_end_matches(|c| matches!(c, ':' | '.' | ',' | ';' | '!' | '?' | ')' | ']' | '}' | '\'' | '"'));
            if url != "http://" && url != "https://" { out.push(url.to_owned()); }
            rest = &tail[end..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_option_that_asks_for_text_leads_to_the_free_field_with_its_name() {
        let opt = |label: &str, description: &str| crate::api::dto::AskOption { label: label.into(), description: description.into(), preview: None };
        assert!(super::asks_for_text(&opt("Canal (digite qual)", "Poste num canal; informe o nome em Other.")));
        assert!(super::asks_for_text(&opt("Outro monitor", "Você diz qual no campo de texto.")));
        assert!(super::asks_for_text(&opt("Já tenho a PM", "Informe a chave em Other.")));
        assert!(!super::asks_for_text(&opt("Inline nesta sessão (Recomendado)", "Executo tudo aqui, em sequência.")));
        assert_eq!(super::text_prefix(&opt("Canal (digite qual)", "")), "Canal: ");
        assert_eq!(super::text_prefix(&opt("Já tenho a PM", "")), "Já tenho a PM: ");
        assert!(super::asks_for_text(&opt("Email (enter address)", "")));
        assert_eq!(super::text_prefix(&opt("Email (enter address)", "")), "Email: ");
        assert!(!super::asks_for_text(&opt("Plano (enterprise)", "")));
        assert!(super::asks_for_text(&opt("Other server", "Type it in Other.")));
        // Descrição comum não troca a opção pelo campo livre.
        assert!(!super::asks_for_text(&opt("US", "Available in other regions")));
        assert!(!super::asks_for_text(&opt("Sem filtro", "Não há outro modo; disponível em outros países.")));
        assert!(!super::asks_for_text(&opt("Single", "No other option fits.")));
        // Parêntese que é parte do nome fica.
        assert!(super::asks_for_text(&opt("Servidor (produção)", "Informe a chave em Other")));
        assert_eq!(super::text_prefix(&opt("Servidor (produção)", "Informe a chave em Other")), "Servidor (produção): ");
    }

    use super::*;
    #[test]
    fn question_links_keeps_only_http_urls_without_trailing_punctuation() {
        assert_eq!(question_links("srv pede para abrir https://a.com/x?y=1: confirme (http://b.com/c)."),
            vec!["https://a.com/x?y=1", "http://b.com/c"]);
        assert!(question_links("rode `curl https://a.com` e ftp://x.com javascript:alert(1)").is_empty());
        assert!(question_links("Aprovar o plano?").is_empty());
    }

    #[test]
    fn queue_actions_route_to_the_entry_by_its_raw_id() {
        assert_eq!(Action::QueueCancel("a".into()).path(), vec!["queue", "a"]);
        assert_eq!(Action::QueueEdit("a".into(), "x".into()).path(), vec!["queue", "a"]);
        assert_eq!(Action::QueueSteer("a".into()).path(), vec!["queue", "a", "steer"]);
        assert_eq!(Action::QueueReorder("a".into(), None).path(), vec!["queue", "reorder"]);
        assert_eq!(Action::QueueReorder("b".into(), Some("a".into())).queue_entry(), Some("b"));
        assert_eq!(Action::Steer.queue_entry(), None);
    }

    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;
    use crate::api::dto::AskOption;

    fn plan_state() -> crate::api::dto::SessionState {
        crate::api::dto::SessionState { state: "awaiting_input".into(),
            question: Some("Claude has written up a plan and is ready to execute. Would you like to proceed?".into()),
            options: Some(vec!["Yes, manually approve edits".into(), "Tell Claude what to change".into()]), ..Default::default() }
    }

    #[test]
    fn claude_plan_review_accepts_headless_pending_without_a_tool_id() {
        let state = crate::api::dto::SessionState { question: Some("Aprovar o plano?".into()),
            claude_plan_pending: json!({"plan":"# Plano", "path":"/plans/p.md"}).as_object().cloned(), ..plan_state() };
        assert_eq!(plan_review(&[], &state, "claude").unwrap().plan, "# Plano");
    }

    #[test]
    fn claude_plan_review_headless_fallback_tracks_confirmed_files() {
        let state = crate::api::dto::SessionState { claude_plan_pending: json!({"plan":"", "tool_use_id":"p"}).as_object().cloned(), ..plan_state() };
        let events = vec![tool("tool_use", "w", "Write", json!({"file_path":"/plans/p.md"})),
            tool("tool_result", "w", "", json!({})), tool("tool_use", "p", "ExitPlanMode", json!({}))];
        assert_eq!(plan_review(&events, &state, "claude").unwrap().file_candidates, vec!["/plans/p.md"]);
    }

    #[test]
    fn claude_plan_review_discards_authoritative_state_when_history_proves_resolution() {
        let state = crate::api::dto::SessionState { claude_plan_pending: json!({"plan":"# Plano", "tool_use_id":"p"}).as_object().cloned(), ..plan_state() };
        let mut events = vec![tool("tool_use", "p", "ExitPlanMode", json!({"plan":"# Plano"}))];
        events.push(tool("tool_result", "p", "", json!({})));
        assert!(plan_review(&events, &state, "claude").is_none());
        events.pop();
        events.push(ChatEvent { kind: "user_msg".into(), id: "u".into(), text: Some("Nova tarefa".into()), ..Default::default() });
        assert!(plan_review(&events, &state, "claude").is_none());
    }

    #[test]
    fn claude_plan_review_accepts_the_terminal_state_with_the_tui_choices() {
        let state = crate::api::dto::SessionState {
            options: Some(vec!["Yes, and use auto mode".into(), "Yes, manually approve edits".into(), "Tell Claude what to change".into()]),
            interaction_id: Some("i1".into()),
            claude_plan_pending: json!({"plan":"# Plano", "path":"/plans/p.md", "tool_use_id":"p1"}).as_object().cloned(), ..plan_state() };
        let events = vec![tool("tool_use", "p1", "ExitPlanMode", json!({"plan":"# Plano"}))];
        let plan = plan_review(&events, &state, "claude").unwrap();
        assert_eq!((plan.tool_id.as_str(), plan.plan.as_str(), plan.path.as_deref()), ("p1", "# Plano", Some("/plans/p.md")));
        assert!(plan_review(&[], &state, "claude").is_some());
        let keys: Vec<_> = state.options.unwrap().iter().map(|o| plan_choice_key(o)).collect();
        assert_eq!(keys, vec![Some("plan_review_auto_mode"), Some("plan_review_manual"), Some("plan_review_changes")]);
        assert_eq!(plan_choice_key("Yes, and auto-accept edits"), Some("plan_review_auto"));
    }

    #[test]
    fn plan_review_labels_preserve_clear_context_and_unknown_choices() {
        assert_eq!(plan_choice_key("Yes, and bypass permissions"), Some("plan_review_bypass"));
        assert_eq!(plan_choice_key("Yes, and switch to BYPASS PERMISSIONS (no further prompts) for this session"), Some("plan_review_bypass"));
        assert_eq!(plan_choice_key("Yes, manually approve edits"), Some("plan_review_manual"));
        assert_eq!(plan_choice_key("Yes, clear context and bypass permissions"), None);
        assert_eq!(plan_choice_key("Yes, clear context and auto-accept edits"), None);
        assert_eq!(plan_choice_key("Yes, and bypass permissions for a different scope"), None);
    }

    #[test]
    fn claude_plan_review_file_fallback_requires_a_confirmed_write_in_this_turn() {
        let exit = tool("tool_use", "p", "ExitPlanMode", json!({}));
        let write = tool("tool_use", "w", "Write", json!({"file_path":"/plans/p.md"}));
        let done = tool("tool_result", "w", "", json!({}));
        assert!(plan_review(&[write.clone(), exit.clone()], &plan_state(), "claude").unwrap().file_candidates.is_empty());
        assert_eq!(plan_review(&[write.clone(), done.clone(), exit.clone()], &plan_state(), "claude").unwrap().file_candidates, vec!["/plans/p.md"]);
        let failed = ChatEvent { is_error: Some(true), ..done.clone() };
        assert!(plan_review(&[write.clone(), failed, exit.clone()], &plan_state(), "claude").unwrap().file_candidates.is_empty());
        let prompt = ChatEvent { kind: "user_msg".into(), id: "u".into(), text: Some("Outro pedido".into()), ..Default::default() };
        assert!(plan_review(&[write, done, prompt, exit], &plan_state(), "claude").unwrap().file_candidates.is_empty());
    }

    #[test]
    fn claude_plan_review_uses_the_full_pending_tool_input() {
        let events = vec![tool("tool_use", "p1", "ExitPlanMode", json!({"plan": "# Plano\n\n## Etapas\n- Implementar"}))];
        let plan = plan_review(&events, &plan_state(), "claude").unwrap();
        assert_eq!(plan.plan, "# Plano\n\n## Etapas\n- Implementar");
        assert_eq!(plan.tool_id, "p1");
    }

    #[test]
    fn claude_plan_review_rejects_answered_old_and_unrelated_requests() {
        let event = tool("tool_use", "p1", "ExitPlanMode", json!({"plan": "# Antigo"}));
        let mut events = vec![event];
        assert!(plan_review(&events, &plan_state(), "codex").is_none());
        let unrelated = crate::api::dto::SessionState { question: Some("Allow Edit?".into()), ..plan_state() };
        assert!(plan_review(&events, &unrelated, "claude").is_none());
        events.push(tool("tool_result", "p1", "", json!({})));
        assert!(plan_review(&events, &plan_state(), "claude").is_none());
        events.pop();
        events.push(ChatEvent { id: "u1".into(), kind: "user_msg".into(), text: Some("Outra tarefa".into()), ..Default::default() });
        assert!(plan_review(&events, &plan_state(), "claude").is_none());
    }

    #[test]
    fn claude_plan_review_accepts_authoritative_headless_state_before_history() {
        let state = crate::api::dto::SessionState { claude_plan_pending: json!({"plan":"# Plano", "tool_use_id":"p1", "path":"/plans/p.md"}).as_object().cloned(), ..plan_state() };
        let plan = plan_review(&[], &state, "claude").unwrap();
        assert_eq!(plan.plan, "# Plano");
        assert_eq!(plan.path.as_deref(), Some("/plans/p.md"));
        let events = vec![tool("tool_use", "other", "ExitPlanMode", json!({"plan":"# Outro"}))];
        assert_eq!(plan_review(&events, &state, "claude").unwrap().plan, "# Plano");
        let state = crate::api::dto::SessionState { state: "working".into(), ..state };
        assert!(plan_review(&events, &state, "claude").is_none());
    }

    fn item(multi: bool, other: bool, labels: &[&str]) -> AskItem {
        AskItem { id: Some("q1".into()), header: "H".into(), question: "Q?".into(), multi_select: multi, is_other: other, is_secret: false,
            options: labels.iter().map(|l| AskOption { label: (*l).into(), ..Default::default() }).collect() }
    }
    fn ask(provider: Option<&str>, request_id: Option<Value>, questions: Vec<AskItem>) -> Ask {
        let payload = AskPayload { provider: provider.map(str::to_owned), request_id, interaction_id: None, is_async: false, questions };
        Ask { fingerprint: "f".into(), payload, tool_use_id: None }
    }

    fn tool(kind: &str, id: &str, name: &str, input: Value) -> ChatEvent {
        ChatEvent { kind: kind.into(), id: format!("e-{kind}-{id}"), tool_use_id: Some(id.into()), tool_name: Some(name.into()),
            tool_input: input.as_object().cloned(), ..Default::default() }
    }

    #[test]
    fn transcript_questions_of_pi_omp_and_kimi() {
        let two = json!({"questions": [
            {"header": "A", "question": "Primeira?", "multi_select": true, "options": [{"label": "x"}, {"label": ""}]},
            {"question": "Segunda?", "options": [{"label": "y"}]}]});
        let omp = ask_from_events(&[tool("tool_use", "t1", "ask", two.clone())], "omp").unwrap();
        assert_eq!(omp.payload.questions.len(), 1);
        assert!(omp.payload.interaction_id.is_none());
        assert_eq!(omp.payload.questions[0].options.len(), 1);
        let kimi = ask_from_events(&[tool("tool_use", "t1", "AskUserQuestion", two)], "kimi").unwrap();
        assert!(kimi.payload.questions[0].multi_select && kimi.payload.questions.len() == 2);
        assert_eq!(kimi.tool_use_id.as_deref(), Some("t1"));
        assert!(kimi.payload.request_id.is_none() && kimi.payload.provider.is_none() && kimi.payload.interaction_id.is_none());
        let pi = json!({"question": "Qual?", "options": [{"label": "a", "description": "d"}]});
        let pi_ask = ask_from_events(&[tool("tool_use", "p", "question", pi.clone())], "pi").unwrap();
        assert_eq!(pi_ask.payload.questions[0].options[0].description, "d");
        assert!(pi_ask.payload.interaction_id.is_none());
        assert!(ask_from_events(&[tool("tool_use", "p", "question", json!({"question": "Qual?"}))], "pi").is_none());
        let answered = [tool("tool_use", "p", "question", pi.clone()), ChatEvent { kind: "tool_result".into(), id: "r".into(), tool_use_id: Some("p".into()), ..Default::default() }];
        assert!(ask_from_events(&answered, "pi").is_none());
        assert!(ask_from_events(&[tool("tool_use", "p", "question", pi)], "claude").is_none());
    }

    #[test]
    fn answer_body_matches_web_payload_with_zero_based_indices_and_raw_request_id() {
        let a = ask(Some("codex"), Some(json!(7)), vec![item(false, true, &["A", "B"])]);
        let body = answer_body(&a, &[Pick::Options(vec![1])]).unwrap();
        assert_eq!(body, json!({"answers": [{"kind": "option", "indices": [1], "multi": false, "labels": ["B"], "question_id": "q1"}], "request_id": 7}));
        let text = answer_body(&a, &[Pick::Text("  livre ".into())]).unwrap();
        assert_eq!(text["answers"][0], json!({"kind": "text", "value": "livre", "type_index": 2, "labels": ["livre"], "question_id": "q1"}));
    }

    #[test]
    fn conversation_answer_body_preserves_displayed_interaction() {
        let raw = json!({"provider":"claude", "request_id":7, "interaction_id":"interaction-a", "is_async":false,
            "questions":[{"header":"Escolha", "question":"Qual?", "multiSelect":false,
                "options":[{"label":"A", "description":"Primeira"}]}]});
        let a = Ask::new(serde_json::from_value(raw.clone()).unwrap(), &raw);
        let mut next = raw.clone();
        next["interaction_id"] = json!("interaction-b");
        let b = Ask::new(serde_json::from_value(next.clone()).unwrap(), &next);
        assert_eq!(a.payload.questions, b.payload.questions);
        assert_ne!(a, b);
        assert_ne!(a.fingerprint, b.fingerprint);
        let body = answer_body(&a, &[Pick::Options(vec![0])]).unwrap();
        assert_eq!(body["interaction_id"], "interaction-a");
        assert_eq!(body["request_id"], 7);
        assert_eq!(answer_body(&b, &[Pick::Options(vec![0])]).unwrap()["interaction_id"], "interaction-b");
        assert_eq!(skip_body(&a).unwrap(), json!({"request_id": 7, "interaction_id": "interaction-a"}));
        assert_eq!(skip_body(&b).unwrap()["interaction_id"], "interaction-b");
    }

    #[test]
    fn incomplete_or_unsupported_answers_are_not_sent() {
        let codex = ask(Some("codex"), Some(json!("r")), vec![item(false, false, &["A"])]);
        assert!(answer_body(&codex, &[Pick::Empty]).is_none());
        assert!(answer_body(&codex, &[Pick::Chat]).is_none());
        assert!(answer_body(&codex, &[Pick::Text("x".into())]).is_none());
        assert!(answer_body(&codex, &[Pick::Options(vec![3])]).is_none());
        let claude = ask(None, None, vec![item(false, false, &["A"]), item(true, false, &["X", "Y"])]);
        let body = answer_body(&claude, &[Pick::Chat, Pick::Options(vec![1, 0])]).unwrap();
        assert_eq!(body["answers"][0], json!({"kind": "chat", "chat_index": 2, "question_id": "q1"}));
        assert_eq!(body["answers"][1]["labels"], json!(["Y", "X"]));
        assert!(body.get("request_id").is_none());
    }

    #[test]
    fn single_choice_replaces_and_multi_choice_toggles() {
        assert_eq!(toggle(&item(false, false, &["A", "B"]), &Pick::Options(vec![0]), 1), Pick::Options(vec![1]));
        let multi = item(true, false, &["A", "B"]);
        let picked = toggle(&multi, &Pick::Text("x".into()), 1);
        assert_eq!(picked, Pick::Options(vec![1]));
        assert_eq!(toggle(&multi, &picked, 1), Pick::Empty);
    }

    #[test]
    fn terminal_checkbox_labels() {
        assert_eq!(checkbox("[ ] Alfa"), Some((false, "Alfa")));
        assert_eq!(checkbox("[✔] Beta"), Some((true, "Beta")));
        assert_eq!(checkbox("[] Gama"), Some((false, "Gama")));
        assert_eq!(checkbox("Yes"), None);
        assert_eq!(checkbox("[abc] x"), None);
    }

    #[test]
    fn ask_picker_is_recognized_by_the_tui_lines() {
        let owned = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(ask_picker(&owned(&["Alfa", "Beta", "Type something.", "Chat about this"])));
        assert!(ask_picker(&owned(&["[ ] Alfa", "[ ] Type something."])));
        assert!(ask_picker(&owned(&["Lint     91 +# diff", "Type something      94 +# do usuário"])));
        assert!(!ask_picker(&owned(&["Yes", "Yes, and don't ask again", "No"])));
    }

    #[test]
    fn proposed_plan_ignores_markers_inside_fences() {
        assert_eq!(proposed_plan("intro\n<proposed_plan>\n# Plano\n- passo\n</proposed_plan>\n").as_deref(), Some("# Plano\n- passo"));
        assert_eq!(proposed_plan("```\n<proposed_plan>\nx\n</proposed_plan>\n```"), None);
        assert_eq!(proposed_plan("sem plano"), None);
        assert_eq!(plan_display("a\n<proposed_plan>\nb\n</proposed_plan>\n```\n<proposed_plan>\n```"), "a\n\nb\n\n```\n<proposed_plan>\n```");
    }

    #[test]
    fn reply_hides_memory_footer_and_streaming_tail_but_preserves_examples() {
        assert_eq!(plan_display("Resposta\n<oai-mem-citation>\nMEMORY.md:1\n</oai-mem-citation>\nfim"), "Resposta\n\nfim");
        assert_eq!(plan_display("Resposta\n  <oai-mem-citation>MEMORY.md:1</oai-mem-citation> fim"), "Resposta\n fim");
        assert_eq!(plan_display("Resposta\n<oai-mem-citation>\nMEMORY.md:1"), "Resposta\n");
        for example in ["```xml\n<oai-mem-citation>x</oai-mem-citation>\n```", "~~~\n<oai-mem-citation>x\n~~~", "`<oai-mem-citation>`", "Exemplo: <oai-mem-citation>x</oai-mem-citation>"] {
            assert_eq!(plan_display(example), example);
        }
    }

    #[test]
    fn conversation_late_result_does_not_finish_a_different_interaction() {
        let key = SessionKey { server: "s".into(), name: "n".into(), jsonl: "j".into() };
        let mut flight = InFlight::default();
        assert!(flight.begin(key.clone(), Action::Answer, "interaction-a".into()));
        assert!(flight.finish(&key, &Action::Answer, "interaction-a").is_some());
        assert!(flight.begin(key.clone(), Action::Answer, "interaction-b".into()));
        assert!(flight.finish(&key, &Action::Answer, "interaction-a").is_none());
        assert!(flight.busy(&key));
        assert!(flight.finish(&key, &Action::Answer, "interaction-b").is_some());
    }

    #[test]
    fn in_flight_is_per_session_and_late_result_of_other_action_is_ignored() {
        let key = |jsonl: &str| SessionKey { server: "s".into(), name: "n".into(), jsonl: jsonl.into() };
        let mut flight = InFlight::default();
        assert!(flight.begin(key("a"), Action::Select(1), "q".into()));
        assert!(!flight.begin(key("a"), Action::Answer, "q".into()));
        assert!(flight.begin(key("b"), Action::Answer, "q".into()));
        assert_eq!(flight.finish(&key("a"), &Action::Answer, "q"), None);
        assert_eq!(flight.finish(&key("a"), &Action::Select(1), "q").as_deref(), Some("q"));
        assert!(!flight.busy(&key("a")));
    }
}
