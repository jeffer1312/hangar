use std::collections::{HashMap, HashSet};
use serde_json::{Value, json};
use crate::{api::dto::{AskItem, AskOption, AskPayload, ChatEvent}, delivery::SessionKey};

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
    Some(Ask { fingerprint: format!("tool:{id}:{input}"), payload: AskPayload { provider: None, request_id: None, questions }, tool_use_id: Some(id) })
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
    let mut answers = Vec::with_capacity(questions.len());
    for (item, pick) in questions.iter().zip(picks) {
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
        answers.push(answer);
    }
    let mut body = json!({"answers": answers});
    if let Some(id) = &ask.payload.request_id { body["request_id"] = id.clone(); }
    Some(body)
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

/// Porta de `planDisplayText` (core) sem a citação de memória: as marcas do protocolo não aparecem.
pub fn plan_display(text: &str) -> String {
    let mut text = text.to_owned();
    for (start, end, _) in plan_markers(&text).into_iter().rev() { text.replace_range(start..end, ""); }
    text
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
pub enum Action { Answer, Select(usize), Submit, Cancel, Steer, Discard(String), Implement }

impl Action {
    pub fn path(&self) -> Vec<&str> {
        match self {
            Action::Answer => vec!["answer"], Action::Select(_) => vec!["select"], Action::Submit => vec!["select", "submit"],
            Action::Cancel => vec!["interrupt"], Action::Steer => vec!["steer"], Action::Discard(id) => vec!["queue", id],
            Action::Implement => vec!["codex", "plan", "implement"],
        }
    }
    // Acima da espera do backend (35 s no Codex) para não declarar incerto o que ainda roda lá.
    pub fn seconds(&self) -> u64 { if matches!(self, Action::Answer) { 60 } else { 30 } }
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
    pub fn finish(&mut self, key: &SessionKey, action: &Action) -> Option<String> {
        if self.actions.get(key).is_none_or(|(current, _)| current != action) { return None; }
        self.actions.remove(key).map(|(_, snapshot)| snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dto::AskOption;

    fn item(multi: bool, other: bool, labels: &[&str]) -> AskItem {
        AskItem { id: Some("q1".into()), header: "H".into(), question: "Q?".into(), multi_select: multi, is_other: other, is_secret: false,
            options: labels.iter().map(|l| AskOption { label: (*l).into(), ..Default::default() }).collect() }
    }
    fn ask(provider: Option<&str>, request_id: Option<Value>, questions: Vec<AskItem>) -> Ask {
        let payload = AskPayload { provider: provider.map(str::to_owned), request_id, questions };
        Ask { fingerprint: "f".into(), payload, tool_use_id: None }
    }

    fn tool(kind: &str, id: &str, name: &str, input: Value) -> ChatEvent {
        ChatEvent { kind: kind.into(), id: format!("e-{kind}-{id}"), tool_use_id: Some(id.into()), tool_name: Some(name.into()),
            tool_input: Some(input), ..Default::default() }
    }

    #[test]
    fn transcript_questions_of_pi_omp_and_kimi() {
        let two = json!({"questions": [
            {"header": "A", "question": "Primeira?", "multi_select": true, "options": [{"label": "x"}, {"label": ""}]},
            {"question": "Segunda?", "options": [{"label": "y"}]}]});
        let omp = ask_from_events(&[tool("tool_use", "t1", "ask", two.clone())], "omp").unwrap();
        assert_eq!(omp.payload.questions.len(), 1);
        assert_eq!(omp.payload.questions[0].options.len(), 1);
        let kimi = ask_from_events(&[tool("tool_use", "t1", "AskUserQuestion", two)], "kimi").unwrap();
        assert!(kimi.payload.questions[0].multi_select && kimi.payload.questions.len() == 2);
        assert_eq!(kimi.tool_use_id.as_deref(), Some("t1"));
        assert!(kimi.payload.request_id.is_none() && kimi.payload.provider.is_none());
        let pi = json!({"question": "Qual?", "options": [{"label": "a", "description": "d"}]});
        assert_eq!(ask_from_events(&[tool("tool_use", "p", "question", pi.clone())], "pi").unwrap().payload.questions[0].options[0].description, "d");
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
    fn proposed_plan_ignores_markers_inside_fences() {
        assert_eq!(proposed_plan("intro\n<proposed_plan>\n# Plano\n- passo\n</proposed_plan>\n").as_deref(), Some("# Plano\n- passo"));
        assert_eq!(proposed_plan("```\n<proposed_plan>\nx\n</proposed_plan>\n```"), None);
        assert_eq!(proposed_plan("sem plano"), None);
        assert_eq!(plan_display("a\n<proposed_plan>\nb\n</proposed_plan>\n```\n<proposed_plan>\n```"), "a\n\nb\n\n```\n<proposed_plan>\n```");
    }

    #[test]
    fn in_flight_is_per_session_and_late_result_of_other_action_is_ignored() {
        let key = |jsonl: &str| SessionKey { server: "s".into(), name: "n".into(), jsonl: jsonl.into() };
        let mut flight = InFlight::default();
        assert!(flight.begin(key("a"), Action::Select(1), "q".into()));
        assert!(!flight.begin(key("a"), Action::Answer, "q".into()));
        assert!(flight.begin(key("b"), Action::Answer, "q".into()));
        assert_eq!(flight.finish(&key("a"), &Action::Answer), None);
        assert_eq!(flight.finish(&key("a"), &Action::Select(1)).as_deref(), Some("q"));
        assert!(!flight.busy(&key("a")));
    }
}
