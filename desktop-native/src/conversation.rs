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

/// `pinned`: chamadas de agente ainda rodando. Saem do meio da conversa (e do grupo) porque o cartão delas fica fixo no fim.
pub fn build(events: &[ChatEvent], view: View, pinned: &HashSet<usize>) -> Vec<Item> {
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
        if pinned.contains(&i) { continue; }
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

/// Subagente lançado por Agent/AgentSwarm. `call` é o índice do tool_use nos eventos.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRun {
    pub call: usize, pub id: String, pub description: String,
    pub subagent_type: Option<String>, pub model: Option<String>, pub prompt: Option<String>, pub running: bool,
}

/// Bash com `run_in_background: true`: o comando cru e o rótulo sem o encanamento.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellRun { pub id: String, pub command: String, pub label: String, pub description: Option<String>, pub ts: Option<f64>, pub running: bool }

/// Tarefa da aba Atividade: TaskCreate/TaskUpdate/TaskStop pelo número sequencial, ou a lista inteira do último
/// TodoWrite/`update_plan`, como o fold do web (não o da lista de tarefas da conversa, que lê o id no resultado).
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityTask { pub title: String, pub active_form: Option<String>, pub status: TaskStatus }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity { pub agents: Vec<AgentRun>, pub shells: Vec<ShellRun>, pub tasks: Vec<ActivityTask> }

impl Activity {
    pub fn running_agents(&self) -> impl Iterator<Item = &AgentRun> { self.agents.iter().filter(|a| a.running) }
    pub fn running_shells(&self) -> impl Iterator<Item = &ShellRun> { self.shells.iter().filter(|s| s.running) }
    /// O número do botão Atividade: tarefas em andamento, agentes e shells rodando.
    pub fn badge(&self) -> usize {
        self.tasks.iter().filter(|t| t.status == TaskStatus::InProgress).count() + self.running_agents().count() + self.running_shells().count()
    }
}

/// `pending` para o que não for um dos três outros, como o `normStatus` do web; `None` é a tarefa apagada.
fn task_status(value: Option<&Value>) -> Option<TaskStatus> {
    match value.and_then(Value::as_str) {
        Some("in_progress") => Some(TaskStatus::InProgress),
        Some("completed") => Some(TaskStatus::Completed),
        Some("deleted") => None,
        _ => Some(TaskStatus::Pending),
    }
}

/// O `String(x ?? y ?? '')` do web para ids que chegam como texto ou número.
fn loose_id(input: Option<&Value>, keys: &[&str]) -> String {
    let value = keys.iter().find_map(|key| input.and_then(|v| v.get(*key)).filter(|v| !v.is_null()));
    match value { Some(Value::String(s)) => s.clone(), Some(other) => other.to_string(), None => String::new() }
}

/// Lista inteira de um TodoWrite (`todos[].content`) ou `update_plan` (`plan[].step`); `None` quando o campo não é lista.
fn whole_list(input: Option<&Value>, list: &str, title: &str) -> Option<Vec<(ActivityTask, bool)>> {
    let raw = input.and_then(|v| v.get(list))?;
    let parsed = raw.as_str().and_then(|s| serde_json::from_str::<Value>(s).ok());
    let items = parsed.as_ref().unwrap_or(raw).as_array()?;
    Some(items.iter().filter_map(|item| {
        let title = item.get(title)?.as_str()?.to_owned();
        let status = task_status(item.get("status"));
        let active_form = item.get("activeForm").and_then(Value::as_str).map(str::to_owned);
        Some((ActivityTask { title, active_form, status: status.unwrap_or(TaskStatus::Pending) }, status.is_none()))
    }).collect())
}

/// Agentes e shells de fundo, dobrados dos eventos como o `createActivityFolder` do web. Refeito a cada troca do
/// conjunto de eventos, então `/clear` e histórico novo não carregam nada do anterior.
pub fn fold_activity(events: &[ChatEvent]) -> Activity {
    let mut resulted: HashSet<&str> = HashSet::new();
    // Id do trabalho em segundo plano (agentId, id do shell) -> tool_use_id do lançamento.
    let mut background: HashMap<String, &str> = HashMap::new();
    // Fim que chegou antes do lançamento: o par fecha quando ele aparecer.
    let mut finished_early: HashSet<String> = HashSet::new();
    // Bash de fundo esperando a resposta de lançamento; outro resultado com a mesma frase não conta.
    let mut shell_pending: HashSet<&str> = HashSet::new();
    let mut agent_calls: HashSet<&str> = HashSet::new();
    let mut agents = Vec::new();
    let mut shells = Vec::new();
    // Tarefas pelo número sequencial do TaskCreate; o `bool` marca a apagada. A lista do último TodoWrite vence.
    let mut created: Vec<(ActivityTask, bool)> = Vec::new();
    let mut whole: Option<Vec<(ActivityTask, bool)>> = None;
    fn finish<'a>(id: String, background: &HashMap<String, &'a str>, resulted: &mut HashSet<&'a str>, early: &mut HashSet<String>) {
        match background.get(&id) { Some(&call) => { resulted.insert(call); } None => { early.insert(id); } }
    }
    for (i, event) in events.iter().enumerate() {
        match event.kind.as_str() {
            "tool_result" => {
                let Some(id) = tool_key(event) else { continue };
                if let Some(task) = id.strip_prefix("task:") { finish(task.to_owned(), &background, &mut resulted, &mut finished_early); continue; }
                let text = event.result.as_deref().unwrap_or("");
                // Só o resultado de um Agent é lido: a conta é refeita a cada evento e as saídas de ferramenta são grandes.
                if agent_calls.contains(id) && text.to_lowercase().contains("async agent launched") {
                    if let Some(agent) = word_after(text, "agentId:") {
                        if finished_early.remove(agent) { resulted.insert(id); }
                        background.insert(agent.to_owned(), id);
                    }
                    continue;
                }
                if shell_pending.remove(id) {
                    if let Some(shell) = word_after(text, "Command running in background with ID:") {
                        if finished_early.remove(shell) { resulted.insert(id); }
                        background.insert(shell.to_owned(), id);
                        continue;
                    }
                    // Pediu fundo e não foi: este resultado já é o final.
                }
                resulted.insert(id);
            }
            "user_msg" => {
                let text = event.text.as_deref().unwrap_or("");
                if !text.contains("<task-notification>") { continue; }
                let Some(task) = text.split_once("<task-id>").and_then(|(_, rest)| rest.split_once("</task-id>")).map(|(id, _)| id.trim()) else { continue };
                finish(task.to_owned(), &background, &mut resulted, &mut finished_early);
            }
            "tool_use" => {
                let input = event.tool_input.as_ref();
                let text = |key: &str| input.and_then(|v| v.get(key)).and_then(Value::as_str).map(str::to_owned);
                // Tarefas não dependem do id da chamada.
                match event.tool_name.as_deref() {
                    Some("TodoWrite") => { if let Some(list) = whole_list(input, "todos", "content") { whole = Some(list); } continue; }
                    Some("update_plan") => { if let Some(list) = whole_list(input, "plan", "step") { whole = Some(list); } continue; }
                    Some("TaskCreate") => {
                        let title = text("subject").or_else(|| text("content"))
                            .unwrap_or_else(|| crate::i18n::tr_web("atividade_tarefa_fallback", &HashMap::new()).unwrap_or_else(|| "Tarefa".into()));
                        created.push((ActivityTask { title, active_form: text("activeForm"), status: TaskStatus::Pending }, false));
                        continue;
                    }
                    Some(name @ ("TaskUpdate" | "TaskStop")) => {
                        let keys: &[&str] = if name == "TaskStop" { &["task_id", "taskId", "id"] } else { &["taskId", "id"] };
                        let id = loose_id(input, keys);
                        if let Some(task) = created.iter_mut().enumerate().find(|(k, _)| (k + 1).to_string() == id).map(|(_, t)| t) {
                            match (name, task_status(input.and_then(|v| v.get("status")))) {
                                ("TaskStop", _) | (_, None) => task.1 = true,
                                (_, Some(status)) => (task.0.status, task.1) = (status, false),
                            }
                        }
                        continue;
                    }
                    _ => {}
                }
                let Some(id) = tool_key(event) else { continue };
                match event.tool_name.as_deref() {
                    Some("Agent" | "AgentSwarm") => {
                        let items = input.and_then(|v| v.get("items")).and_then(Value::as_array).map_or(0, Vec::len);
                        let desc = text("description").or_else(|| text("subagent_type"))
                            .unwrap_or_else(|| crate::i18n::tr_web("atividade_agente", &HashMap::new()).unwrap_or_else(|| "Agente".into()));
                        let description = if items > 0 {
                            let params = HashMap::from([("desc".to_owned(), desc.clone()), ("n".to_owned(), items.to_string())]);
                            crate::i18n::tr_web("atividade_swarm_itens", &params).unwrap_or(desc)
                        } else { desc };
                        agent_calls.insert(id);
                        agents.push(AgentRun { call: i, id: id.to_owned(), description, subagent_type: text("subagent_type"),
                            model: text("model"), prompt: text("prompt"), running: false });
                    }
                    Some("Bash") if input.and_then(|v| v.get("run_in_background")).and_then(Value::as_bool) == Some(true) => {
                        shell_pending.insert(id);
                        let command = text("command").unwrap_or_default();
                        shells.push(ShellRun { id: id.to_owned(), label: command_label(&command), command,
                            description: text("description"), ts: event.ts, running: false });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    for agent in &mut agents { agent.running = !resulted.contains(agent.id.as_str()); }
    for shell in &mut shells { shell.running = !resulted.contains(shell.id.as_str()); }
    agents.sort_by_key(|a| !a.running);
    shells.sort_by_key(|s| !s.running);
    let tasks = whole.unwrap_or(created).into_iter().filter(|(_, deleted)| !deleted).map(|(task, _)| task).collect();
    Activity { agents, shells, tasks }
}

/// A palavra `[A-Za-z0-9_-]+` logo depois do marcador (espaços no meio à vontade).
fn word_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let rest = text[text.find(marker)? + marker.len()..].trim_start();
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// O comando sem o encanamento que não diz o que ele faz: o `cd … &&` da frente, as redireções e o `| tail -N` do fim.
/// É poda, não interpretação; comando que era só encanamento volta cru.
pub fn command_label(command: &str) -> String {
    let mut s = command.to_owned();
    // cd <caminho> && no começo.
    let head = s.trim_start();
    if let Some(after) = head.strip_prefix("cd").filter(|rest| rest.starts_with(char::is_whitespace)) {
        let after = after.trim_start();
        let path_end = match after.chars().next() {
            Some(q @ ('"' | '\'')) => after[1..].find(q).map(|at| at + 2),
            // O `\S+` do web recua até um `&&` colado (`cd /a&&make`) quando não há `&&` depois do espaço.
            Some(_) => {
                let word = after.find(char::is_whitespace).unwrap_or(after.len());
                if after[word..].trim_start().starts_with("&&") { Some(word) } else { after[..word].rfind("&&").filter(|&at| at > 0) }
            }
            None => None,
        };
        if let Some(rest) = path_end.map(|end| after[end..].trim_start()).and_then(|rest| rest.strip_prefix("&&")) {
            s = format!(" {}", rest.trim_start());
        }
    }
    // >/dev/null, 2>&1, 2>/dev/null em qualquer lugar.
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(at) = rest.find('>') {
        let after = rest[at + 1..].trim_start();
        let target = if after.starts_with('&') && after[1..].starts_with(|c: char| c.is_ascii_digit()) { Some(2) }
            else if after.starts_with("/dev/null") { Some("/dev/null".len()) } else { None };
        let Some(len) = target else { out.push_str(&rest[..=at]); rest = &rest[at + 1..]; continue };
        let mut start = at;
        if rest[..start].ends_with(|c: char| c.is_ascii_digit()) { start -= 1; }
        out.push_str(rest[..start].trim_end());
        out.push(' ');
        rest = &after[len..];
    }
    out.push_str(rest);
    // | tail -N / | head -N no fim.
    let trimmed = out.trim_end();
    if let Some(pipe) = trimmed.rfind('|') {
        let tail = trimmed[pipe + 1..].trim_start();
        let tool = tail.strip_prefix("tail").or_else(|| tail.strip_prefix("head")).filter(|r| r.starts_with(char::is_whitespace));
        let flag = tool.map(str::trim_start).and_then(|r| r.strip_prefix('-'));
        if let Some(flag) = flag {
            let word = flag.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(flag.len());
            let left = flag[word..].trim_start().trim_start_matches(|c: char| c.is_ascii_digit()).trim();
            if word > 0 && left.is_empty() { out.truncate(pipe); }
        }
    }
    let label = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.is_empty() { command.trim().to_owned() } else { label }
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
    fn build(events: &[ChatEvent]) -> Vec<Item> { super::build(events, View::default(), &HashSet::new()) }
    fn with_input(mut event: ChatEvent, input: Value) -> ChatEvent { event.tool_input = Some(input); event }
    fn answered(id: &str, tool: &str, text: &str) -> ChatEvent { ChatEvent { result: Some(text.into()), ..result(id, tool) } }

    #[test]
    fn thinking_mode_decides_which_calls_fold_in() {
        let events = vec![ev("thinking", "t1"), call("s", "1", "WebSearch"), call("b", "2", "Bash"), call("k", "3", "TaskCreate")];
        let parts = |mode| match &super::build(&events, View { thinking: mode, tasks: false }, &HashSet::new())[0] {
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
        let items = super::build(&events, View { tasks: true, ..View::default() }, &HashSet::new());
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
        assert!(!super::build(&events, View { tasks: true, ..View::default() }, &HashSet::new()).iter().any(|i| matches!(i, Item::Tasks { .. })));
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

    fn agent(id: &str, tool: &str, input: Value) -> ChatEvent { with_input(call(id, tool, "Agent"), input) }
    fn note(id: &str, task: &str) -> ChatEvent {
        ChatEvent { text: Some(format!("<task-notification>\n<task-id> {task} </task-id>\n<status>completed</status></task-notification>")), ..ev("user_msg", id) }
    }
    fn running(events: &[ChatEvent]) -> Vec<String> { fold_activity(events).running_agents().map(|a| a.id.clone()).collect() }

    #[test]
    fn foreground_agent_runs_until_its_result() {
        let mut events = vec![agent("a", "t1", json!({"description": "Ler o fold", "subagent_type": "Explore", "model": "haiku", "prompt": "p"}))];
        let run = &fold_activity(&events).agents[0];
        assert_eq!((run.call, run.description.as_str(), run.subagent_type.as_deref(), run.model.as_deref(), run.prompt.as_deref(), run.running),
            (0, "Ler o fold", Some("Explore"), Some("haiku"), Some("p"), true));
        events.push(answered("r", "t1", "pronto"));
        assert!(running(&events).is_empty());
    }

    #[test]
    fn background_agent_survives_its_launch_and_ends_by_either_signal_in_either_order() {
        let launch = |id: &str, tool: &str, agent_id: &str| answered(id, tool, &format!("Async agent launched successfully.\nagentId: {agent_id} (use it)"));
        let start = vec![agent("a", "t1", json!({})), launch("r", "t1", "ag-1")];
        assert_eq!(running(&start), vec!["t1"]);
        // Fim pelo resultado sintético do backend.
        let mut ended = start.clone();
        ended.push(result("x", "task:ag-1"));
        assert!(running(&ended).is_empty());
        // Fim pela notificação na mensagem do usuário.
        let mut ended = start.clone();
        ended.push(note("n", "ag-1"));
        assert!(running(&ended).is_empty());
        // Fim antes do lançamento, pelos dois caminhos.
        for end in [result("x", "task:ag-1"), note("n", "ag-1")] {
            let events = vec![agent("a", "t1", json!({})), end, launch("r", "t1", "ag-1")];
            assert!(running(&events).is_empty());
        }
        // Fim de outro agente não fecha este.
        let mut other = start.clone();
        other.push(result("x", "task:ag-2"));
        assert_eq!(running(&other), vec!["t1"]);
    }

    #[test]
    fn swarm_counts_items_and_description_falls_back_to_the_type() {
        let events = vec![with_input(call("a", "t1", "AgentSwarm"), json!({"description": "Revisar", "items": [1, 2, 3]})),
            agent("b", "t2", json!({"subagent_type": "Explore"})), agent("c", "t3", json!({}))];
        let descriptions: Vec<String> = fold_activity(&events).agents.into_iter().map(|a| a.description).collect();
        let swarm = crate::i18n::tr_web("atividade_swarm_itens", &HashMap::from([("desc".into(), "Revisar".into()), ("n".into(), "3".into())])).unwrap();
        assert_eq!(descriptions, vec![swarm, "Explore".into(), crate::i18n::tr_web("atividade_agente", &HashMap::new()).unwrap()]);
    }

    #[test]
    fn background_shell_ends_by_notification_and_a_refused_one_ends_on_its_result() {
        let shell = |id: &str, tool: &str, command: &str| with_input(call(id, tool, "Bash"), json!({"command": command, "run_in_background": true}));
        let events = vec![
            shell("a", "s1", "cd /tmp/x && cargo build 2>&1 | tail -5"),
            answered("r1", "s1", "Command running in background with ID: bg1"),
            shell("b", "s2", "sleep 9"),
            answered("r2", "s2", "Error: background refused"),
            with_input(call("c", "s3", "Bash"), json!({"command": "grep x"})),
            answered("r3", "s3", "Command running in background with ID: bg9"),
        ];
        let activity = fold_activity(&events);
        let shells: Vec<(&str, &str, bool)> = activity.shells.iter().map(|s| (s.id.as_str(), s.label.as_str(), s.running)).collect();
        assert_eq!(shells, vec![("s1", "cargo build", true), ("s2", "sleep 9", false)]);
        let mut ended = events.clone();
        ended.push(result("x", "task:bg1"));
        assert!(fold_activity(&ended).shells.iter().all(|s| !s.running));
    }

    #[test]
    fn fold_starts_over_on_a_new_event_set() {
        let old = vec![agent("a", "t1", json!({}))];
        assert_eq!(running(&old), vec!["t1"]);
        let fresh = vec![ev("assistant_msg", "m")];
        assert_eq!(fold_activity(&fresh), Activity::default());
    }

    #[test]
    fn running_agent_leaves_the_middle_and_its_group() {
        let events = vec![call("a", "t1", "Read"), agent("b", "t2", json!({})), call("c", "t3", "Read"), call("d", "t4", "Read")];
        let pinned: HashSet<usize> = fold_activity(&events).running_agents().map(|a| a.call).collect();
        let items = super::build(&events, View::default(), &pinned);
        assert_eq!(items, vec![Item::Group { id: "g-a".into(), tools: vec![Tool { call: 0, result: None }, Tool { call: 2, result: None }, Tool { call: 3, result: None }] }]);
    }

    #[test]
    fn command_label_prunes_plumbing_only() {
        assert_eq!(command_label("cd \"/a b\" && npm test >/dev/null 2>&1"), "npm test");
        assert_eq!(command_label("make 2>/dev/null | head -n 20"), "make");
        assert_eq!(command_label("echo a > out.txt | tail -f log"), "echo a > out.txt | tail -f log");
        assert_eq!(command_label("  >/dev/null "), ">/dev/null");
        assert_eq!(command_label("cd /a&&make"), "make");
        assert_eq!(command_label("cd /a && b&&c"), "b&&c");
    }

    #[test]
    fn activity_tasks_follow_the_web_fold() {
        let task = |id: &str, tool: &str, name: &str, input: Value| with_input(call(id, tool, name), input);
        let titles = |events: &[ChatEvent]| fold_activity(events).tasks.into_iter().map(|t| (t.title, t.status)).collect::<Vec<_>>();
        let events = vec![
            task("a", "1", "TaskCreate", json!({"subject": "Ler"})),
            task("b", "2", "TaskCreate", json!({"content": "Codar", "activeForm": "Codando"})),
            task("c", "3", "TaskCreate", json!({})),
            task("d", "4", "TaskUpdate", json!({"taskId": 2, "status": "in_progress"})),
            task("e", "5", "TaskUpdate", json!({"taskId": "1", "status": "completed"})),
            task("f", "6", "TaskStop", json!({"task_id": "3"})),
            task("g", "7", "TaskUpdate", json!({"taskId": "9", "status": "completed"})),
        ];
        assert_eq!(titles(&events), vec![("Ler".into(), TaskStatus::Completed), ("Codar".into(), TaskStatus::InProgress)]);
        let fallback = crate::i18n::tr_web("atividade_tarefa_fallback", &HashMap::new()).unwrap();
        assert_eq!(titles(&events[..3]).last(), Some(&(fallback, TaskStatus::Pending)));
        assert_eq!(fold_activity(&events).badge(), 1);
        // A lista inteira do último TodoWrite (mesmo em texto JSON) ou update_plan vence as TaskCreate.
        let mut todo = events.clone();
        todo.push(task("h", "8", "TodoWrite", json!({"todos": "[{\"content\": \"A\", \"status\": \"completed\"}, {\"content\": \"B\", \"status\": \"deleted\"}]"})));
        assert_eq!(titles(&todo), vec![("A".into(), TaskStatus::Completed)]);
        todo.push(task("i", "9", "update_plan", json!({"plan": [{"step": "P", "status": "in_progress"}, {"nope": 1}]})));
        assert_eq!(titles(&todo), vec![("P".into(), TaskStatus::InProgress)]);
    }

    #[test]
    fn fence_outgrows_backticks_and_clip_keeps_char_boundaries() {
        assert!(fenced("a ``` b").starts_with("````text"));
        assert_eq!(clip("ação", 2), ("aç", true));
        assert_eq!(clip("ab", 5), ("ab", false));
    }
}
