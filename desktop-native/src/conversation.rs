use std::collections::{HashMap, HashSet};
use serde_json::Value;
use crate::{api::dto::ChatEvent, appearance::ThinkingTools};

/// One visible row. Holds indices into `Chat::events`, never the row's own position in the list.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Event(usize),
    Tool(Tool),
    Group { id: String, tools: Vec<Tool> },
    Thinking { id: String, parts: Vec<usize> },
    Orphan(usize),
    /// Bloco de progresso da lista de tarefas, no lugar da última chamada TaskCreate/TaskUpdate.
    /// A lista vem dobrada aqui: o desenho só a lê.
    Tasks { id: String, tasks: Vec<Task> },
}

/// Escolhas de Aparência que mudam quais linhas a conversa tem.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View { pub thinking: ThinkingTools, pub tasks: bool }

impl Default for View {
    fn default() -> Self { Self { thinking: ThinkingTools::Search, tasks: false } }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tool { pub call: usize, pub result: Option<usize> }

impl Item {
    pub fn id(&self, events: &[ChatEvent]) -> String {
        match self {
            Item::Event(i) | Item::Orphan(i) | Item::Tool(Tool { call: i, .. }) => events[*i].id.clone(),
            Item::Group { id, .. } | Item::Thinking { id, .. } | Item::Tasks { id, .. } => id.clone(),
        }
    }
}

const GROUP_MIN: usize = 3;

fn tool_key(event: &ChatEvent) -> Option<&str> {
    event.tool_use_id.as_deref().map(str::trim).filter(|id| !id.is_empty())
}

/// Busca na web e o carregador de ferramentas que costuma vir antes dela.
pub fn is_search(name: Option<&str>) -> bool {
    matches!(name, Some("WebSearch" | "WebFetch" | "ToolSearch"))
}

fn is_task_call(name: Option<&str>) -> bool { matches!(name, Some("TaskCreate" | "TaskUpdate")) }

// Como no web: as chamadas de tarefa nunca entram no pensamento, nem no "Tudo"; o bloco de tarefas as substitui.
fn joins_thinking(mode: ThinkingTools, name: Option<&str>) -> bool {
    match mode {
        ThinkingTools::None => false,
        _ if is_task_call(name) => false,
        ThinkingTools::All => true,
        ThinkingTools::Search => is_search(name),
    }
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

pub fn build(events: &[ChatEvent], view: View) -> Vec<Item> {
    let (paired, orphans) = pair_results(events);
    let mut items = Vec::new();
    let mut run: Vec<Tool> = Vec::new();
    let mut thinking: Vec<usize> = Vec::new();
    // Posição e id do bloco de tarefas: onde estava a última chamada de tarefa, com o id da primeira.
    let mut tasks: Option<(usize, String)> = None;
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
            "tool_use" if view.tasks && is_task_call(event.tool_name.as_deref()) => {
                flush_thinking(&mut thinking, &mut items);
                flush_run(&mut run, &mut items);
                let id = tasks.take().map_or_else(|| format!("tasks-{}", event.id), |(_, id)| id);
                tasks = Some((items.len(), id));
                continue;
            }
            "tool_use" if !thinking.is_empty() && joins_thinking(view.thinking, event.tool_name.as_deref()) => { thinking.push(i); continue; }
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
    if let Some((at, id)) = tasks {
        let tasks = fold_tasks(events, &paired);
        if !tasks.is_empty() { items.insert(at, Item::Tasks { id, tasks }); }
    }
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.id(events)));
    items
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus { Pending, InProgress, Completed }

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    /// Identidade estável do passo: o número do "Task #N", ou o id da chamada enquanto o resultado não chegou.
    /// A posição na lista muda quando uma tarefa sai; esta chave não.
    pub key: String,
    pub subject: String, pub description: String, pub active_form: String, pub status: TaskStatus,
}

/// A lista de tarefas do agente, dobrando os TaskCreate/TaskUpdate na ordem. O id nasce no resultado
/// do create ("Task #2 created successfully"); update de id desconhecido é de outra sessão e fica de fora.
pub fn fold_tasks(events: &[ChatEvent], paired: &HashMap<usize, usize>) -> Vec<Task> {
    // `true` quando a chave é o número da tarefa: só essas recebem TaskUpdate.
    let mut order: Vec<(bool, Task)> = Vec::new();
    for (i, event) in events.iter().enumerate().filter(|(_, e)| e.kind == "tool_use") {
        let text = |key: &str| event.tool_input.as_ref().and_then(|v| v.get(key)).and_then(Value::as_str).unwrap_or("").to_owned();
        match event.tool_name.as_deref() {
            Some("TaskCreate") => {
                let result = paired.get(&i).and_then(|&r| events[r].result.as_deref()).unwrap_or("");
                let id = created_task_id(result);
                let known = !id.is_empty();
                let key = if known { id } else { format!("call:{}", event.id) };
                order.push((known, Task { key, subject: text("subject"), description: text("description"), active_form: text("activeForm"), status: TaskStatus::Pending }));
            }
            Some("TaskUpdate") => {
                let id = text("taskId");
                let Some(at) = order.iter().position(|(known, task)| *known && task.key == id) else { continue };
                let status = text("status");
                if status == "deleted" { order.remove(at); continue; }
                let task = &mut order[at].1;
                match status.as_str() {
                    "pending" => task.status = TaskStatus::Pending,
                    "in_progress" => task.status = TaskStatus::InProgress,
                    "completed" => task.status = TaskStatus::Completed,
                    _ => {}
                }
                for (key, field) in [("subject", &mut task.subject), ("description", &mut task.description), ("activeForm", &mut task.active_form)] {
                    let value = text(key);
                    if !value.is_empty() { *field = value; }
                }
            }
            _ => {}
        }
    }
    order.into_iter().map(|(_, task)| task).collect()
}

/// O número depois de "Task #" (com espaço à vontade e sem caixa), como a regra do web; outro "#" do texto não conta.
fn created_task_id(result: &str) -> String {
    result.match_indices('#').find_map(|(at, _)| {
        let before = result[..at].trim_end();
        let word = before.get(before.len().saturating_sub(4)..)?;
        let digits: String = result[at + 1..].chars().take_while(char::is_ascii_digit).collect();
        (word.eq_ignore_ascii_case("task") && before.len() < result[..at].len() && !digits.is_empty()).then_some(digits)
    }).unwrap_or_default()
}

/// Família da chamada no modo Chips: dá o verbo da linha e a contagem do título do grupo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family { Read, Search, Edit, Create, Run, Other }

pub fn family(name: Option<&str>) -> Family {
    match name.unwrap_or("") {
        "Read" | "NotebookRead" => Family::Read,
        // Mesma lista do `is_search`, mais Grep e Glob: o carregador de ferramentas é busca também aqui.
        "Grep" | "Glob" | "WebSearch" | "WebFetch" | "ToolSearch" => Family::Search,
        "Edit" | "MultiEdit" | "NotebookEdit" => Family::Edit,
        "Write" => Family::Create,
        "Bash" | "exec" | "exec_command" => Family::Run,
        _ => Family::Other,
    }
}

/// Contagem por família na ordem do título do grupo ("Leu 2 arquivos · fez 1 busca · …"). O título nasce
/// sempre daqui: o `description` do Bash não o substitui.
pub fn family_counts(events: &[ChatEvent], tools: &[Tool]) -> Vec<(Family, usize)> {
    const ORDER: [Family; 6] = [Family::Read, Family::Search, Family::Edit, Family::Create, Family::Run, Family::Other];
    ORDER.iter().map(|&f| (f, tools.iter().filter(|t| family(events[t.call].tool_name.as_deref()) == f).count()))
        .filter(|&(_, n)| n > 0).collect()
}

/// `mcp__hangar-computer-control__objetivo` vira "computer-control · objetivo"; o resto fica como veio.
pub fn tool_display_name(name: &str) -> String {
    let parts: Vec<&str> = name.split("__").collect();
    if parts.len() < 3 || parts[0] != "mcp" { return name.to_owned(); }
    format!("{} · {}", parts[1].trim_start_matches("hangar-"), parts[2..].join("__"))
}

/// Linhas que a edição põe e tira, contadas pelo texto novo e o antigo da chamada.
// ponytail: conta linhas inteiras, não um diff; um diff de verdade entra se a contagem enganar.
pub fn edit_counts(name: Option<&str>, input: Option<&Value>) -> Option<(usize, usize)> {
    let input = input?;
    let lines = |v: Option<&Value>| v.and_then(Value::as_str).map_or(0, |s| s.lines().count());
    match name? {
        "Edit" => Some((lines(input.get("new_string")), lines(input.get("old_string")))),
        "MultiEdit" => Some(input.get("edits")?.as_array()?.iter()
            .fold((0, 0), |(a, r), e| (a + lines(e.get("new_string")), r + lines(e.get("old_string"))))),
        "Write" => Some((lines(input.get("content")), 0)),
        _ => None,
    }
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
    // Os testes antigos valem para o padrão da Aparência.
    fn build(events: &[ChatEvent]) -> Vec<Item> { super::build(events, View::default()) }
    fn with_input(mut event: ChatEvent, input: Value) -> ChatEvent { event.tool_input = Some(input); event }
    fn answered(id: &str, tool: &str, text: &str) -> ChatEvent { ChatEvent { result: Some(text.into()), ..result(id, tool) } }

    #[test]
    fn thinking_mode_decides_which_calls_fold_in() {
        let events = vec![ev("thinking", "t1"), call("s", "1", "WebSearch"), call("b", "2", "Bash"), call("k", "3", "TaskCreate")];
        let parts = |mode| match &super::build(&events, View { thinking: mode, tasks: false })[0] {
            Item::Thinking { parts, .. } => parts.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(parts(ThinkingTools::None), vec![0]);
        assert_eq!(parts(ThinkingTools::Search), vec![0, 1]);
        // Tudo leva o Bash, mas nunca a chamada de tarefa.
        assert_eq!(parts(ThinkingTools::All), vec![0, 1, 2]);
    }

    #[test]
    fn task_calls_become_one_block_where_the_last_one_was() {
        let events = vec![
            ev("user_msg", "u"),
            with_input(call("c1", "1", "TaskCreate"), json!({"subject": "Ler", "activeForm": "Lendo"})), answered("r1", "1", "Task #1 created successfully: Ler"),
            with_input(call("c2", "2", "TaskCreate"), json!({"subject": "Trocar"})), answered("r2", "2", "Task #2 created successfully: Trocar"),
            ev("assistant_msg", "a"),
            with_input(call("up", "3", "TaskUpdate"), json!({"taskId": "1", "status": "completed"})),
            with_input(call("x", "4", "TaskUpdate"), json!({"taskId": "9", "status": "completed"})),
            ev("assistant_msg", "b"),
        ];
        let items = super::build(&events, View { tasks: true, ..View::default() });
        let tasks = fold_tasks(&events, &pair_results(&events).0);
        assert_eq!(items, vec![Item::Event(0), Item::Event(5), Item::Tasks { id: "tasks-c1".into(), tasks: tasks.clone() }, Item::Event(8)]);
        assert_eq!(tasks.iter().map(|t| (t.key.as_str(), t.subject.as_str(), t.status)).collect::<Vec<_>>(),
            [("1", "Ler", TaskStatus::Completed), ("2", "Trocar", TaskStatus::Pending)]);
        // Sem a opção, as chamadas de tarefa continuam como ferramentas.
        assert!(build(&events).iter().any(|item| matches!(item, Item::Tool(Tool { call: 1, .. }))));
    }

    #[test]
    fn deleted_task_leaves_the_list_and_chips_helpers_read_the_input() {
        let events = vec![with_input(call("c", "1", "TaskCreate"), json!({"subject": "A"})), answered("r", "1", "Task #7 created"),
            with_input(call("d", "2", "TaskUpdate"), json!({"taskId": "7", "status": "deleted"}))];
        assert!(fold_tasks(&events, &pair_results(&events).0).is_empty());
        assert!(!super::build(&events, View { tasks: true, ..View::default() }).iter().any(|i| matches!(i, Item::Tasks { .. })));
        assert_eq!(edit_counts(Some("Edit"), Some(&json!({"old_string": "a\nb", "new_string": "a\nb\nc"}))), Some((3, 2)));
        assert_eq!(tool_display_name("mcp__hangar-computer-control__objetivo"), "computer-control · objetivo");
        assert_eq!(family(Some("Glob")), Family::Search);
    }

    #[test]
    fn task_key_survives_the_deletion_of_an_earlier_task() {
        let create = |n: &str, subject: &str| [with_input(call(&format!("c{n}"), &format!("t{n}"), "TaskCreate"), json!({"subject": subject})),
            answered(&format!("r{n}"), &format!("t{n}"), &format!("Task #{n} created successfully"))];
        let mut events: Vec<ChatEvent> = create("1", "A").into_iter().chain(create("2", "B")).collect();
        let keys = |events: &[ChatEvent]| fold_tasks(events, &pair_results(events).0).into_iter().map(|t| (t.key, t.subject)).collect::<Vec<_>>();
        assert_eq!(keys(&events), [("1".to_owned(), "A".to_owned()), ("2".to_owned(), "B".to_owned())]);
        // A tarefa 1 sai: a B muda de posição, mas continua com a chave 2 (é dela o estado aberto na tela).
        events.push(with_input(call("d1", "u1", "TaskUpdate"), json!({"taskId": "1", "status": "deleted"})));
        assert_eq!(keys(&events), [("2".to_owned(), "B".to_owned())]);
        // Sem resultado ainda, a chave é a da chamada, e nenhum TaskUpdate a alcança.
        let pending = vec![with_input(call("c9", "t9", "TaskCreate"), json!({"subject": "C"})),
            with_input(call("u9", "u9", "TaskUpdate"), json!({"taskId": "", "status": "completed"}))];
        let folded = fold_tasks(&pending, &pair_results(&pending).0);
        assert_eq!((folded[0].key.as_str(), folded[0].status), ("call:c9", TaskStatus::Pending));
    }

    #[test]
    fn group_title_counts_families_even_with_a_bash_description() {
        let events = vec![call("a", "1", "ToolSearch"), call("b", "2", "WebSearch"), call("c", "3", "Grep"),
            with_input(call("d", "4", "Bash"), json!({"command": "ls", "description": "Listar"}))];
        let tools: Vec<Tool> = (0..4).map(|call| Tool { call, result: None }).collect();
        assert_eq!(family_counts(&events, &tools), [(Family::Search, 3), (Family::Run, 1)]);
    }

    #[test]
    fn task_id_comes_from_task_number_not_the_first_hash() {
        assert_eq!(created_task_id("Task list #3 has 2 items; Task #4 created successfully: X"), "4");
        assert_eq!(created_task_id("task   #12 created"), "12");
        assert_eq!(created_task_id("#5 sem a palavra"), "");
        assert_eq!(created_task_id("Task#7"), "");
    }

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
