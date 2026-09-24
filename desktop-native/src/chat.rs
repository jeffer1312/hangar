use std::collections::{HashMap, HashSet};
use serde_json::Value;
use crate::{api::dto::{ChatEvent, Preview, SessionState}, interaction::Ask};

#[derive(Clone, Debug, PartialEq)]
pub struct LiveTool { pub name: String, pub input: Value }

#[derive(Default)]
pub struct Chat {
    pub events: Vec<ChatEvent>,
    pub preview: Preview,
    pub state: SessionState,
    pub ask: Option<Ask>,
    pub live_thinking: String,
    pub live_tool: Option<LiveTool>,
    // Último registro durável que consolidou um item em voo; barra o quadro atrasado do mesmo item.
    settled_thinking: String,
    settled_tool: Option<(LiveTool, Option<String>)>,
    removed: HashSet<String>,
    claimed_real: HashSet<String>,
    index: HashMap<String, usize>,
}

impl Chat {
    fn reindex(&mut self) {
        self.index = self.events.iter().enumerate().map(|(i, e)| (e.id.clone(), i)).collect();
    }

    pub fn apply(&mut self, event: ChatEvent) {
        if event.queued_confirmed == Some(true) {
            self.removed.insert(event.id.clone());
            self.events.retain(|e| e.id != event.id);
            self.reindex();
            return;
        }
        if self.removed.contains(&event.id) { return; }
        if let Some(&i) = self.index.get(&event.id) {
            if event.kind == "assistant_msg" && self.events[i].text != event.text
                && event.text.as_deref().is_some_and(|text| preview_matches(&self.preview.text, text)) {
                self.clear_preview();
            }
            self.events[i] = event;
            return;
        }
        if event.kind == "assistant_msg" && event.text.as_deref().is_some_and(|text| preview_matches(&self.preview.text, text)) {
            self.clear_preview();
        }
        if event.kind == "user_msg" && event.queued() {
            let candidates: Vec<_> = self.events.iter().filter(|e| e.kind == "user_msg" && e.queued()).collect();
            let covered = self.events.iter().filter(|real| real.kind == "user_msg" && !real.queued()
                && !self.claimed_real.contains(&real.id)).find(|real| {
                let Some(candidate) = claim(real, &event) else { return false; };
                !candidates.iter().any(|other| claim(real, other).is_some_and(|score| score >= candidate))
            });
            if let Some(real) = covered {
                self.claimed_real.insert(real.id.clone());
                self.removed.insert(event.id);
                return;
            }
        }
        if event.kind == "user_msg" && !event.queued() && !self.claimed_real.contains(&event.id) {
            let owner = self.events.iter().enumerate().filter(|(_, e)| e.queued())
                .filter_map(|(i, e)| claim(&event, e).map(|score| (i, score)))
                .max_by(|(ai, a), (bi, b)| a.cmp(b).then_with(|| bi.cmp(ai))).map(|(i, _)| i);
            if let Some(i) = owner {
                self.removed.insert(self.events[i].id.clone());
                self.events.remove(i);
                self.reindex();
                self.claimed_real.insert(event.id.clone());
            }
        }
        self.settle_live(&event);
        self.index.insert(event.id.clone(), self.events.len());
        self.events.push(event);
    }

    // Só o registro que corresponde ao item em voo o substitui; outro pensamento ou chamada não apaga.
    fn settle_live(&mut self, event: &ChatEvent) {
        match event.kind.as_str() {
            "thinking" => {
                let text = event.text.clone().unwrap_or_default();
                if thought_covers(&text, &self.live_thinking) { self.live_thinking.clear(); }
                self.settled_thinking = text;
            }
            "tool_use" => {
                let durable = LiveTool { name: event.tool_name.clone().unwrap_or_default(), input: event.tool_input.clone().unwrap_or(Value::Null) };
                if self.live_tool.as_ref().is_some_and(|live| tool_matches(live, &durable)) { self.live_tool = None; }
                self.settled_tool = Some((durable, event.tool_use_id.clone()));
            }
            "tool_result" => {
                if self.settled_tool.as_ref().is_some_and(|(_, id)| id.is_some() && *id == event.tool_use_id) { self.settled_tool = None; }
            }
            _ => {}
        }
    }

    /// Returns false for a late frame of a thought that is already recorded.
    pub fn update_live_thinking(&mut self, text: String) -> bool {
        if thought_covers(&self.settled_thinking, &text) { return false; }
        self.live_thinking = text;
        true
    }

    pub fn update_live_tool(&mut self, tool: LiveTool) -> bool {
        if self.settled_tool.as_ref().is_some_and(|(settled, _)| tool_matches(&tool, settled)) { return false; }
        self.live_tool = Some(tool);
        true
    }

    pub fn merge_history(&mut self, history: Vec<ChatEvent>) {
        // Mantém os eventos ao vivo e as baixas que chegaram enquanto o HTTP estava em voo.
        // Reaplicar o histórico não é evento novo: o que está em voo continua como estava.
        let live = (self.preview.clone(), self.live_thinking.clone(), self.live_tool.clone(),
            self.settled_thinking.clone(), self.settled_tool.clone());
        let history: Vec<_> = history.into_iter().filter(|e| !self.removed.contains(&e.id)).collect();
        let ids: HashSet<_> = history.iter().map(|e| e.id.clone()).collect();
        let current = std::mem::take(&mut self.events);
        let positions: HashMap<_, _> = current.iter().enumerate().map(|(i, e)| (e.id.clone(), i)).collect();
        let Some(first) = current.iter().position(|e| ids.contains(&e.id)) else {
            self.events = Vec::new();
            self.index.clear();
            for event in history.into_iter().chain(current) { self.apply(event); }
            (self.preview, self.live_thinking, self.live_tool, self.settled_thinking, self.settled_tool) = live;
            self.drop_recorded_preview();
            return;
        };
        let mut merged = current[..first].to_vec();
        let mut cursor = first;
        for event in history {
            if let Some(&position) = positions.get(&event.id) {
                while cursor < position {
                    if !ids.contains(&current[cursor].id) { merged.push(current[cursor].clone()); }
                    cursor += 1;
                }
                merged.push(current[position].clone());
                cursor = cursor.max(position + 1);
            } else { merged.push(event); }
        }
        merged.extend(current[cursor..].iter().filter(|e| !ids.contains(&e.id)).cloned());
        self.events = Vec::new();
        self.index.clear();
        for event in merged { self.apply(event); }
        (self.preview, self.live_thinking, self.live_tool, self.settled_thinking, self.settled_tool) = live;
        self.drop_recorded_preview();
    }

    // O histórico recarregado pode trazer gravada a resposta que a prévia ainda mostra.
    fn drop_recorded_preview(&mut self) {
        if self.recently_committed(&self.preview.text) { self.clear_preview(); }
    }

    pub fn update_preview(&mut self, next: Preview) -> bool {
        if next.text.is_empty() { return false; }
        if self.preview.md == next.md && self.preview.full == next.full && self.preview.vivo == next.vivo
            && next.text.len() < self.preview.text.len() && self.preview.text.starts_with(&next.text) {
            return false;
        }
        // O pane reemite o bloco anterior entre turnos e entre ferramentas; já gravado, não volta como prévia.
        if self.recently_committed(&next.text) {
            let visible = !self.preview.text.is_empty();
            self.clear_preview();
            return visible;
        }
        self.preview = next;
        true
    }

    fn recently_committed(&self, preview: &str) -> bool {
        let preview = flatten(preview);
        preview.chars().count() >= 16 && self.events.iter().rev()
            .filter(|e| e.kind == "assistant_msg")
            .filter_map(|e| e.text.as_deref().filter(|text| !text.is_empty()))
            .take(6).any(|text| flatten(text).contains(&preview))
    }

    pub fn update_state(&mut self, state: SessionState) {
        // Pergunta do Claude fecha quando o pane sai do aguardo; a do Codex só pelo `null`; a do transcript, pelo `tool_result`.
        if state.state != "awaiting_input" && self.ask.as_ref().is_some_and(|ask| !ask.codex() && ask.tool_use_id.is_none()) { self.ask = None; }
        self.state = state;
    }

    /// Nova pergunta só troca a atual quando o conteúdo muda: o retrato de reconexão não apaga escolhas.
    pub fn update_ask(&mut self, ask: Option<Ask>) -> bool {
        if self.ask == ask { return false; }
        self.ask = ask;
        true
    }

    /// Porta de `queuedMessages` (core): no Codex e sem terminal a fila é do processo, entregue não espera mais.
    pub fn waiting(&self, provider: &str, headless: bool) -> usize {
        let process = provider == "codex" || headless;
        self.events.iter().filter(|e| e.kind == "user_msg" && e.queued() && e.desistiu != Some(true)
            && !(process && e.queued_delivered == Some(true))).count()
    }

    pub fn retire(&mut self, id: &str) {
        self.removed.insert(id.to_owned());
        self.events.retain(|e| e.id != id);
        self.reindex();
    }

    // Resposta de /steer: `queued_ids` já saiu para o turno; `promoted` baixa a fila inteira.
    pub fn steered(&mut self, promoted: bool, ids: &[String]) {
        if promoted {
            let queued: Vec<String> = self.events.iter().filter(|e| e.kind == "user_msg" && e.queued()).map(|e| e.id.clone()).collect();
            for id in queued { self.retire(&id); }
            return;
        }
        for event in self.events.iter_mut().filter(|e| ids.contains(&e.id)) { event.queued_delivered = Some(true); }
    }

    pub fn clear_preview(&mut self) {
        self.preview = Preview::default();
    }
}

fn preview_matches(preview: &str, committed: &str) -> bool {
    if preview.is_empty() || committed.is_empty() { return false; }
    let preview = flatten(preview);
    let committed = flatten(committed);
    preview == committed || preview.chars().count() >= 16
        && (committed.contains(&preview) || preview.starts_with(&committed))
}

// O pane chega renderizado e o transcript em markdown cru: compara sem marcas nem marcador de lista.
fn flatten(source: &str) -> String {
    source.lines().map(|line| {
        let line = line.trim_start();
        if let Some(marker) = line.chars().next() {
            if matches!(marker, '-' | '•' | '◦' | '▪') {
                let rest = &line[marker.len_utf8()..];
                if rest.starts_with(char::is_whitespace) { return rest.trim_start(); }
            }
        }
        line
    })
        .collect::<Vec<_>>().join(" ").chars().filter(|ch| !matches!(ch, '`' | '*' | '_' | '~' | '#' | '>'))
        .collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn thought_covers(durable: &str, live: &str) -> bool {
    let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let live = flat(live);
    !live.is_empty() && flat(durable).contains(&live)
}

fn tool_matches(live: &LiveTool, durable: &LiveTool) -> bool {
    let empty = |v: &Value| v.is_null() || v.as_object().is_some_and(|m| m.is_empty());
    live.name == durable.name && (live.input == durable.input || empty(&live.input))
}

fn claim(real: &ChatEvent, queued: &ChatEvent) -> Option<(u8, usize)> {
    if let (Some(real_ts), Some(queue_ts)) = (real.ts, queued.queued_ts.or(queued.ts)) {
        if real_ts + 2.0 < queue_ts { return None; }
    }
    let text = real.text.as_deref()?.trim();
    let needle = queued.text.as_deref()?.trim();
    if needle.is_empty() { return None; }
    let size = needle.chars().count();
    if text == needle || text.lines().any(|line| line.trim() == needle) { return Some((3, size)); }
    let caption = |s: &str| -> String {
        let index = ["📎 imagem:", "📎 arquivo:"].iter().filter_map(|m| s.find(m)).min();
        index.map(|i| s[..i].trim_end().trim_end_matches('—').trim_end().to_owned()).unwrap_or_else(|| s.to_owned())
    };
    let q = caption(needle);
    if !q.is_empty() && caption(text) == q { return Some((2, size)); }
    if size >= 8 && text.lines().any(|line| line.trim().starts_with(needle)) { return Some((1, size)); }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, id: &str, text: &str) -> ChatEvent {
        ChatEvent { kind: kind.into(), id: id.into(), text: Some(text.into()), ..Default::default() }
    }

    #[test]
    fn replay_replaces_one_event_by_id() {
        let mut chat = Chat::default();
        chat.apply(event("assistant_msg", "same", "before"));
        chat.apply(event("assistant_msg", "same", "after"));
        assert_eq!(chat.events.len(), 1);
        assert_eq!(chat.events[0].text.as_deref(), Some("after"));
    }

    #[test]
    fn identical_real_messages_keep_distinct_ids() {
        let mut chat = Chat::default();
        chat.apply(event("user_msg", "first", "same text"));
        chat.apply(event("user_msg", "second", "same text"));
        assert_eq!(chat.events.iter().map(|event| event.id.as_str()).collect::<Vec<_>>(), ["first", "second"]);
    }

    #[test]
    fn queue_and_real_match_in_either_arrival_order() {
        for queued_first in [true, false] {
            let mut chat = Chat::default();
            let queued = event("user_msg", "queued-one", "one prompt");
            let real = event("user_msg", "real-one", "one prompt");
            if queued_first { chat.apply(queued); chat.apply(real); }
            else { chat.apply(real); chat.apply(queued); }
            assert_eq!(chat.events.len(), 1);
            assert_eq!(chat.events[0].id, "real-one");
        }
    }

    #[test]
    fn one_real_message_claims_only_one_identical_queue_entry() {
        let mut chat = Chat::default();
        chat.apply(event("user_msg", "queued-one", "same prompt"));
        chat.apply(event("user_msg", "queued-two", "same prompt"));
        chat.apply(event("user_msg", "real-one", "same prompt"));
        assert_eq!(chat.events.len(), 2);
        assert!(chat.events.iter().any(|event| event.id == "queued-two"));
        assert!(chat.events.iter().any(|event| event.id == "real-one"));
    }

    #[test]
    fn history_merge_does_not_reuse_a_real_message_for_another_queue_entry() {
        let mut chat = Chat::default();
        chat.apply(event("user_msg", "queued-one", "ok"));
        chat.apply(event("user_msg", "queued-two", "ok"));
        chat.apply(event("user_msg", "real-one", "ok"));
        chat.merge_history(vec![event("user_msg", "real-one", "ok")]);
        assert_eq!(chat.events.iter().map(|event| event.id.as_str()).collect::<Vec<_>>(), ["queued-two", "real-one"]);
    }

    #[test]
    fn confirmed_queue_entry_stays_removed_after_replay() {
        let mut chat = Chat::default();
        chat.apply(event("user_msg", "queued-one", "one prompt"));
        let mut confirmed = event("user_msg", "queued-one", "one prompt");
        confirmed.queued_confirmed = Some(true);
        chat.apply(confirmed);
        chat.apply(event("user_msg", "queued-one", "one prompt"));
        assert!(chat.events.is_empty());
    }

    #[test]
    fn newer_live_event_survives_history_merge_at_the_end() {
        let mut chat = Chat::default();
        chat.apply(event("assistant_msg", "seam", "newer version"));
        chat.apply(event("assistant_msg", "live", "after snapshot"));
        chat.merge_history(vec![event("user_msg", "older", "before"), event("assistant_msg", "seam", "old version")]);
        assert_eq!(chat.events.iter().map(|event| event.id.as_str()).collect::<Vec<_>>(), ["older", "seam", "live"]);
        assert_eq!(chat.events[1].text.as_deref(), Some("newer version"));
    }

    #[test]
    fn transient_pane_frames_keep_preview_until_final_message() {
        let mut chat = Chat::default();
        chat.update_state(SessionState { state: "working".into(), ..Default::default() });
        chat.update_preview(Preview { text: "first line\nsecond line".into(), ..Default::default() });
        chat.update_preview(Preview { text: "first line".into(), ..Default::default() });
        assert_eq!(chat.preview.text, "first line\nsecond line");
        chat.update_preview(Preview::default());
        assert_eq!(chat.preview.text, "first line\nsecond line");
        chat.update_state(SessionState { state: "idle".into(), ..Default::default() });
        assert_eq!(chat.preview.text, "first line\nsecond line");
        chat.apply(event("assistant_msg", "answer", "first line\nsecond line"));
        assert!(chat.preview.text.is_empty());
    }

    #[test]
    fn unrelated_assistant_commit_and_unchanged_replay_keep_preview() {
        let mut chat = Chat::default();
        let mut recent = event("user_msg", "recent", "new request");
        recent.ts = Some(200.0);
        chat.apply(recent);
        chat.update_preview(Preview { text: "new answer".into(), ..Default::default() });
        let mut stale = event("assistant_msg", "stale", "old answer");
        stale.ts = Some(201.0);
        chat.apply(stale);
        assert_eq!(chat.preview.text, "new answer");
        chat.apply(event("assistant_msg", "stale", "new answer"));
        assert!(chat.preview.text.is_empty());

        chat.update_preview(Preview { text: "old answer".into(), ..Default::default() });
        chat.apply(event("assistant_msg", "stale", "new answer"));
        assert_eq!(chat.preview.text, "old answer");
    }

    #[test]
    fn unchanged_replay_does_not_clear_matching_new_preview() {
        let mut chat = Chat::default();
        chat.apply(event("assistant_msg", "old", "same opening"));
        chat.update_preview(Preview { text: "same opening".into(), ..Default::default() });
        chat.apply(event("assistant_msg", "old", "same opening"));
        assert_eq!(chat.preview.text, "same opening");
    }

    #[test]
    fn pane_replay_of_a_recorded_answer_is_not_a_preview() {
        let mut chat = Chat::default();
        chat.update_state(SessionState { state: "working".into(), ..Default::default() });
        chat.apply(event("assistant_msg", "said", "Uso o **despachante** Lua do Hyprland para aumentar a janela."));
        chat.apply(ChatEvent { kind: "tool_use".into(), id: "u".into(), tool_name: Some("Bash".into()), ..Default::default() });
        assert!(!chat.update_preview(Preview { text: "Uso o despachante Lua do Hyprland para aumentar a janela.".into(), ..Default::default() }));
        assert!(chat.preview.text.is_empty());
        assert!(chat.update_preview(Preview { text: "Próxima resposta, ainda em voo.".into(), ..Default::default() }));
    }

    #[test]
    fn history_that_records_the_preview_drops_it() {
        let mut chat = Chat::default();
        chat.update_preview(Preview { text: "Resposta longa o bastante. É a mesma do histórico.".into(), ..Default::default() });
        chat.merge_history(vec![event("assistant_msg", "said", "Resposta longa o bastante. É a mesma do histórico.")]);
        assert!(chat.preview.text.is_empty());
    }

    #[test]
    fn shorter_preview_from_a_new_source_is_accepted() {
        let mut chat = Chat::default();
        chat.update_preview(Preview { text: "long pane preview".into(), ..Default::default() });
        chat.update_preview(Preview { text: "long".into(), md: true, full: true, vivo: true });
        assert_eq!(chat.preview.text, "long");
    }

    #[test]
    fn durable_thinking_and_tool_replace_live_rows() {
        let mut chat = Chat::default();
        chat.live_thinking = "draft".into();
        chat.live_tool = Some(LiveTool { name: "Bash".into(), input: Value::Null });
        chat.apply(event("thinking", "t", "draft"));
        assert!(chat.live_thinking.is_empty());
        assert!(chat.live_tool.is_some());
        chat.apply(ChatEvent { kind: "tool_use".into(), id: "u".into(), tool_name: Some("Bash".into()), ..Default::default() });
        assert!(chat.live_tool.is_none());
    }

    #[test]
    fn replay_and_history_of_other_items_keep_live_rows() {
        let mut chat = Chat::default();
        let a = event("thinking", "a", "older thought A");
        let call_a = ChatEvent { kind: "tool_use".into(), id: "ua".into(), tool_name: Some("Read".into()), ..Default::default() };
        chat.apply(a.clone());
        chat.apply(call_a.clone());
        assert!(chat.update_live_thinking("thought B".into()));
        assert!(chat.update_live_tool(LiveTool { name: "Bash".into(), input: serde_json::json!({"command": "ls"}) }));
        chat.apply(a.clone());
        chat.apply(call_a.clone());
        chat.merge_history(vec![event("user_msg", "q", "question"), a, call_a]);
        assert_eq!(chat.live_thinking, "thought B");
        assert!(chat.live_tool.is_some());
        chat.apply(event("thinking", "b", "thought B, finished"));
        assert!(chat.live_thinking.is_empty());
        assert!(!chat.update_live_thinking("thought B".into()));
        assert!(chat.live_thinking.is_empty());
        assert!(chat.update_live_thinking("thought C".into()));
    }

    fn ask(provider: Option<&str>, question: &str) -> Ask {
        let raw = serde_json::json!({"provider": provider, "request_id": 1, "questions": [{"question": question}]});
        Ask::new(serde_json::from_value(raw.clone()).unwrap(), &raw)
    }

    #[test]
    fn identical_ask_snapshot_keeps_form_and_claude_ask_closes_when_pane_leaves_waiting() {
        let mut chat = Chat::default();
        assert!(chat.update_ask(Some(ask(None, "a?"))));
        assert!(!chat.update_ask(Some(ask(None, "a?"))));
        assert!(chat.update_ask(Some(ask(None, "b?"))));
        chat.update_state(SessionState { state: "working".into(), ..Default::default() });
        assert!(chat.ask.is_none());
        chat.update_ask(Some(ask(Some("codex"), "c?")));
        chat.update_state(SessionState { state: "working".into(), ..Default::default() });
        assert!(chat.ask.is_some());
        assert!(chat.update_ask(None));
    }

    #[test]
    fn steer_marks_only_listed_entries_unless_promoted() {
        let mut chat = Chat::default();
        chat.apply(event("user_msg", "queued-a", "um"));
        chat.apply(event("user_msg", "queued-b", "dois"));
        chat.steered(false, &["queued-a".into()]);
        assert_eq!(chat.events[0].queued_delivered, Some(true));
        assert_eq!(chat.events[1].queued_delivered, None);
        chat.steered(true, &[]);
        assert!(chat.events.is_empty());
        chat.apply(event("user_msg", "queued-a", "um"));
        assert!(chat.events.is_empty());
    }

    #[test]
    fn delivered_entry_waits_only_in_terminal_queues_and_abandoned_never_counts() {
        let mut chat = Chat::default();
        let mut delivered = event("user_msg", "queued-a", "um");
        delivered.queued_delivered = Some(true);
        chat.apply(delivered);
        chat.apply(event("user_msg", "queued-b", "dois"));
        let mut abandoned = event("user_msg", "queued-c", "três");
        abandoned.desistiu = Some(true);
        chat.apply(abandoned);
        assert_eq!(chat.waiting("codex", false), 1);
        assert_eq!(chat.waiting("claude", true), 1);
        assert_eq!(chat.waiting("kimi", false), 2);
    }

    #[test]
    fn late_tool_frame_after_record_is_ignored_until_its_result() {
        let mut chat = Chat::default();
        let tool = LiveTool { name: "Bash".into(), input: serde_json::json!({"command": "ls"}) };
        chat.update_live_tool(tool.clone());
        chat.apply(ChatEvent { kind: "tool_use".into(), id: "u".into(), tool_name: Some("Bash".into()),
            tool_input: Some(serde_json::json!({"command": "ls"})), tool_use_id: Some("x".into()), ..Default::default() });
        assert!(chat.live_tool.is_none());
        assert!(!chat.update_live_tool(tool.clone()));
        chat.apply(ChatEvent { kind: "tool_result".into(), id: "r".into(), tool_use_id: Some("x".into()), ..Default::default() });
        assert!(chat.update_live_tool(tool));
    }
}
