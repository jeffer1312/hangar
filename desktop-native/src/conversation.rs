use std::collections::{HashMap, HashSet};
use serde_json::Value;
use crate::api::dto::ChatEvent;

/// One visible row. Holds indices into `Chat::events`, never the row's own position in the list.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Event(usize),
    Tool(Tool),
    Group { id: String, tools: Vec<Tool> },
    Thinking { id: String, parts: Vec<usize> },
    Orphan(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tool { pub call: usize, pub result: Option<usize> }

impl Item {
    pub fn id(&self, events: &[ChatEvent]) -> String {
        match self {
            Item::Event(i) | Item::Orphan(i) | Item::Tool(Tool { call: i, .. }) => events[*i].id.clone(),
            Item::Group { id, .. } | Item::Thinking { id, .. } => id.clone(),
        }
    }
}

const GROUP_MIN: usize = 3;

fn tool_key(event: &ChatEvent) -> Option<&str> {
    event.tool_use_id.as_deref().map(str::trim).filter(|id| !id.is_empty())
}

// Pesquisa feita no meio do raciocínio pertence a ele, como no chat web; o resto continua visível.
fn joins_thinking(name: Option<&str>) -> bool {
    matches!(name, Some("WebSearch" | "WebFetch" | "ToolSearch"))
}

/// Pairs each tool_result with the earliest earlier tool_use still without a result under the
/// same tool_use_id; a result never binds to a call that comes after it.
pub fn pair_results(events: &[ChatEvent]) -> (HashMap<usize, usize>, HashSet<usize>) {
    let mut open: HashMap<&str, std::collections::VecDeque<usize>> = HashMap::new();
    let mut paired = HashMap::new();
    let mut orphans = HashSet::new();
    for (i, event) in events.iter().enumerate() {
        match event.kind.as_str() {
            "tool_use" => if let Some(key) = tool_key(event) { open.entry(key).or_default().push_back(i); },
            "tool_result" => match tool_key(event).and_then(|key| open.get_mut(key)).and_then(|calls| calls.pop_front()) {
                Some(call) => { paired.insert(call, i); }
                None => { orphans.insert(i); }
            },
            _ => {}
        }
    }
    (paired, orphans)
}

pub fn build(events: &[ChatEvent]) -> Vec<Item> {
    let (paired, orphans) = pair_results(events);
    let mut items = Vec::new();
    let mut run: Vec<Tool> = Vec::new();
    let mut thinking: Vec<usize> = Vec::new();
    let flush_run = |run: &mut Vec<Tool>, items: &mut Vec<Item>| {
        if run.len() >= GROUP_MIN {
            let id = format!("g-{}", events[run[0].call].id);
            items.push(Item::Group { id, tools: std::mem::take(run) });
        } else { items.extend(run.drain(..).map(Item::Tool)); }
    };
    let flush_thinking = |thinking: &mut Vec<usize>, items: &mut Vec<Item>| {
        if let Some(&first) = thinking.first() {
            items.push(Item::Thinking { id: format!("p-{}", events[first].id), parts: std::mem::take(thinking) });
        }
    };
    for (i, event) in events.iter().enumerate() {
        match event.kind.as_str() {
            "tool_result" if !orphans.contains(&i) => continue,
            // Sinal sintético de fim de tarefa em segundo plano: não é saída de ferramenta.
            "tool_result" if tool_key(event).is_some_and(|key| key.starts_with("task:")) => continue,
            "thinking" => { flush_run(&mut run, &mut items); thinking.push(i); continue; }
            "tool_use" if !thinking.is_empty() && joins_thinking(event.tool_name.as_deref()) => { thinking.push(i); continue; }
            _ => {}
        }
        flush_thinking(&mut thinking, &mut items);
        if event.kind == "tool_use" {
            run.push(Tool { call: i, result: paired.get(&i).copied() });
            continue;
        }
        flush_run(&mut run, &mut items);
        items.push(if event.kind == "tool_result" { Item::Orphan(i) } else { Item::Event(i) });
    }
    flush_run(&mut run, &mut items);
    flush_thinking(&mut thinking, &mut items);
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.id(events)));
    items
}

const SUMMARY_MAX: usize = 96;

pub fn one_line(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max { return line; }
    let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

pub fn summarize_input(name: Option<&str>, input: Option<&Value>) -> String {
    let Some(Value::Object(map)) = input else { return String::new(); };
    let text = |key: &str| match map.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(Value::Array(list)) => list.iter().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()))
            .collect::<Vec<_>>().join(", "),
        Some(other) => other.to_string(),
    };
    let first = |keys: &[&str]| keys.iter().map(|k| text(k)).find(|v| !v.is_empty()).unwrap_or_default();
    let found = match name.unwrap_or("") {
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => first(&["file_path", "path", "notebook_path"]),
        "Bash" | "BashOutput" => first(&["command"]),
        "exec" | "exec_command" | "write_stdin" => first(&["command", "cmd", "code"]),
        "Grep" | "Glob" => {
            let pattern = text("pattern");
            let path = text("path");
            if pattern.is_empty() { String::new() } else if path.is_empty() { format!("\"{pattern}\"") } else { format!("\"{pattern}\" {path}") }
        }
        "WebSearch" => first(&["query", "queries"]),
        "WebFetch" => first(&["url", "urls"]),
        "update_plan" => map.get("plan").and_then(Value::as_array).and_then(|plan| {
            plan.iter().find(|step| step.get("status").and_then(Value::as_str) == Some("in_progress")).or(plan.first())
        }).and_then(|step| step.get("step")).and_then(Value::as_str).unwrap_or_default().to_owned(),
        _ => String::new(),
    };
    let found = if found.is_empty() {
        first(&["file_path", "path", "command", "query", "url", "pattern", "name", "description", "prompt"])
    } else { found };
    let found = if found.is_empty() { map.values().next().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).unwrap_or_default() } else { found };
    one_line(&found, SUMMARY_MAX)
}

pub fn pretty_input(input: Option<&Value>) -> String {
    match input {
        None | Some(Value::Null) => String::new(),
        Some(Value::Object(map)) if map.is_empty() => String::new(),
        Some(value) => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}

/// First sentence of the thought when it is short, otherwise the flattened start.
pub fn thought_summary(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = flat.char_indices().find(|&(i, ch)| matches!(ch, '.' | '!' | '?')
        && flat[i + ch.len_utf8()..].chars().next().is_none_or(char::is_whitespace)).map(|(i, _)| i + 1);
    match end { Some(end) if flat[..end].chars().count() < 140 => flat[..end].to_owned(), _ => one_line(&flat, 140) }
}

/// Code fence that survives backtick runs inside the content.
pub fn fenced(source: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in source.chars() {
        if ch == '`' { run += 1; longest = longest.max(run); } else { run = 0; }
    }
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}text\n{source}\n{fence}")
}

/// Bounded view of a long text; the full text stays in the event and in the copy action.
pub fn clip(source: &str, max: usize) -> (&str, bool) {
    match source.char_indices().nth(max) {
        Some((cut, _)) => (&source[..cut], true),
        None => (source, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(kind: &str, id: &str) -> ChatEvent { ChatEvent { kind: kind.into(), id: id.into(), ..Default::default() } }
    fn call(id: &str, tool: &str, name: &str) -> ChatEvent {
        ChatEvent { tool_use_id: Some(tool.into()), tool_name: Some(name.into()), ..ev("tool_use", id) }
    }
    fn result(id: &str, tool: &str) -> ChatEvent { ChatEvent { tool_use_id: Some(tool.into()), ..ev("tool_result", id) } }

    #[test]
    fn result_pairs_by_tool_use_id_not_event_id() {
        let events = vec![call("evt-a", "toolu_1", "Bash"), result("evt-b", " toolu_1 ")];
        assert_eq!(build(&events), vec![Item::Tool(Tool { call: 0, result: Some(1) })]);
    }

    #[test]
    fn repeated_tool_id_pairs_in_order_and_extra_result_stays_visible() {
        let events = vec![call("a", "same", "Read"), call("b", "same", "Read"), result("r1", "same"), result("r2", "same"), result("r3", "same")];
        let (paired, orphans) = pair_results(&events);
        assert_eq!(paired.get(&0), Some(&2));
        assert_eq!(paired.get(&1), Some(&3));
        assert!(orphans.contains(&4));
        assert!(build(&events).contains(&Item::Orphan(4)));
    }

    #[test]
    fn earlier_result_never_binds_to_a_later_call() {
        let events = vec![result("old", "x"), call("new", "x", "Bash"), result("new-r", "x")];
        let (paired, orphans) = pair_results(&events);
        assert!(orphans.contains(&0));
        assert_eq!(paired.get(&1), Some(&2));
    }

    #[test]
    fn page_that_starts_mid_pair_pairs_after_older_page_arrives() {
        let tail = vec![result("r", "x"), ev("assistant_msg", "m")];
        assert_eq!(build(&tail)[0], Item::Orphan(0));
        let full = vec![call("c", "x", "Read"), result("r", "x"), ev("assistant_msg", "m")];
        assert_eq!(build(&full)[0], Item::Tool(Tool { call: 0, result: Some(1) }));
    }

    #[test]
    fn three_consecutive_calls_group_under_stable_first_id() {
        let events = vec![call("a", "1", "Read"), result("ra", "1"), call("b", "2", "Read"), call("c", "3", "Bash"), ev("assistant_msg", "m")];
        let items = build(&events);
        assert!(matches!(&items[0], Item::Group { id, tools } if id == "g-a" && tools.len() == 3 && tools[0].result == Some(1)));
        assert_eq!(items[1], Item::Event(4));
    }

    #[test]
    fn thinking_swallows_search_but_not_bash() {
        let events = vec![ev("thinking", "t1"), call("s", "1", "WebSearch"), ev("thinking", "t2"), call("b", "2", "Bash")];
        let items = build(&events);
        assert_eq!(items[0], Item::Thinking { id: "p-t1".into(), parts: vec![0, 1, 2] });
        assert_eq!(items[1], Item::Tool(Tool { call: 3, result: None }));
    }

    #[test]
    fn synthetic_task_signal_is_not_an_orphan_row() {
        let events = vec![result("x", "task:abc"), ev("assistant_msg", "m")];
        assert_eq!(build(&events), vec![Item::Event(1)]);
    }

    #[test]
    fn summaries_pick_salient_field() {
        assert_eq!(summarize_input(Some("Bash"), Some(&json!({"command": "ls   -la\n/tmp"}))), "ls -la /tmp");
        assert_eq!(summarize_input(Some("Grep"), Some(&json!({"pattern": "foo", "path": "src"}))), "\"foo\" src");
        assert_eq!(summarize_input(Some("mcp_x"), Some(&json!({"other": 3}))), "3");
    }

    #[test]
    fn fence_outgrows_backticks_and_clip_keeps_char_boundaries() {
        assert!(fenced("a ``` b").starts_with("````text"));
        assert_eq!(clip("ação", 2), ("aç", true));
        assert_eq!(clip("ab", 5), ("ab", false));
    }
}
