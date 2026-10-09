use std::collections::{HashMap, HashSet, VecDeque};
use serde_json::Value;
use super::{Activity, ActivityTask, AgentRun, Item, ShellRun, Task, TaskStatus, Tool, View,
    command_label, fold_tasks, is_agent_call, is_page_call, is_task_call, joins_thinking, loose_id,
    task_status, tool_key, whole_list, word_after, shown_agent_id, GROUP_MIN};
use crate::api::dto::ChatEvent;

/// A região anterior a `from` conserva linhas e conteúdo, não apenas os eventos.
#[derive(Clone, Copy, Debug)]
pub struct Delta {
    pub from: usize,
    pub previous_len: usize,
    pub stable_events: usize,
    pub full: bool,
    pub activity_changed: bool,
}

#[derive(Default)]
pub struct Incremental {
    pub items: Vec<Item>,
    pub paired: HashMap<usize, usize>,
    pub orphans: HashSet<usize>,
    pub activity: Activity,
    pub pinned: HashSet<usize>,
    running_count: usize,
    activity_badge: usize,
    cursor: usize,
    view: Option<View>,
    open: HashMap<String, VecDeque<usize>>,
    activity_fold: ActivityFold,
    /// Um evento comum fecha a sequência estrutural anterior, mesmo que sua linha seja deduplicada.
    frontier: usize,
    segments: Vec<usize>,
    starts: Vec<usize>,
    ids: Vec<String>,
    seen: HashSet<String>,
    first_task: Option<String>,
    task_frontier: Option<usize>,
    tasks: Vec<Task>,
}

impl Incremental {
    // O retrato ordena os agentes ativos primeiro; a cauda não visita os já concluídos.
    pub fn running_agents(&self) -> impl Iterator<Item = &AgentRun> { self.activity.agents[..self.running_count].iter() }
    pub fn activity_badge(&self) -> usize { self.activity_badge }

    pub fn update(&mut self, events: &[ChatEvent], stable: usize, view: View) -> Delta {
        let previous_len = self.items.len();
        let full = self.view != Some(view) || stable < self.cursor || events.len() < self.cursor;
        if full { *self = Self::default(); }
        if !full && self.cursor == events.len() {
            return Delta { from: previous_len, previous_len, stable_events: stable, full: false, activity_changed: false };
        }
        let mut boundary = self.frontier;
        let previous_task_frontier = self.task_frontier;
        let mut tasks_changed = full;
        let mut activity_changed = full;
        for (i, event) in events.iter().enumerate().skip(self.cursor) {
            self.segments.push(self.frontier);
            if !matches!(event.kind.as_str(), "tool_use" | "tool_result" | "thinking") {
                self.frontier = i + 1;
            }
            if event.kind == "tool_use" {
                if let Some(key) = tool_key(event) { self.open.entry(key.to_owned()).or_default().push_back(i); }
                if is_task_call(event.tool_name.as_deref()) {
                    self.first_task.get_or_insert_with(|| format!("tasks-{}", event.id));
                    self.task_frontier = Some(self.segments[i]);
                    tasks_changed = true;
                }
            } else if event.kind == "tool_result" {
                let call = tool_key(event).and_then(|key| self.open.get_mut(key)).and_then(VecDeque::pop_front);
                if let Some(call) = call {
                    self.paired.insert(call, i);
                    boundary = boundary.min(self.segments[call]);
                    tasks_changed |= is_task_call(events[call].tool_name.as_deref());
                } else { self.orphans.insert(i); }
            }
            activity_changed |= self.activity_fold.push(i, event);
        }
        if activity_changed {
            self.activity = self.activity_fold.snapshot();
            self.running_count = self.activity.agents.iter().take_while(|agent| agent.running).count();
            self.activity_badge = self.activity.badge();
            let pinned: HashSet<_> = self.running_agents().map(|agent| agent.call).collect();
            for call in pinned.symmetric_difference(&self.pinned) {
                boundary = boundary.min(self.segments[*call]);
            }
            self.pinned = pinned;
        }
        // O resultado de um create pode validar updates antigos e mover o bloco único de tarefas.
        if view.tasks && tasks_changed {
            self.tasks = fold_tasks(events, &self.paired);
            if let Some(start) = previous_task_frontier { boundary = boundary.min(start); }
        }
        let from = self.starts.partition_point(|start| *start < boundary);
        for id in self.ids.drain(from..) { self.seen.remove(&id); }
        self.items.truncate(from);
        self.starts.truncate(from);
        for (start, item) in self.build_suffix(events, boundary, view) {
            let id = item.id(events);
            if !self.seen.insert(id.clone()) { continue; }
            self.starts.push(start);
            self.ids.push(id);
            self.items.push(item);
        }
        self.cursor = events.len();
        self.view = Some(view);
        Delta { from, previous_len, stable_events: if full { 0 } else { stable }, full, activity_changed }
    }

    fn build_suffix(&self, events: &[ChatEvent], start: usize, view: View) -> Vec<(usize, Item)> {
        let mut items = Vec::new();
        let mut run = Vec::new();
        let mut thinking = Vec::new();
        let mut tasks_at = None;
        let flush_run = |run: &mut Vec<Tool>, items: &mut Vec<(usize, Item)>| {
            if run.len() >= GROUP_MIN || (view.merge_thinking || view.every_run_groups) && !run.is_empty() {
                let first = run[0].call;
                items.push((first, Item::Group { id: format!("g-{}", events[first].id), tools: std::mem::take(run) }));
            } else { items.extend(run.drain(..).map(|tool| (tool.call, Item::Tool(tool)))); }
        };
        let flush_thinking = |thinking: &mut Vec<usize>, items: &mut Vec<(usize, Item)>| {
            if let Some(&first) = thinking.first() {
                items.push((first, Item::Thinking { id: format!("p-{}", events[first].id), parts: std::mem::take(thinking) }));
            }
        };
        for (i, event) in events.iter().enumerate().skip(start) {
            if self.pinned.contains(&i) { continue; }
            match event.kind.as_str() {
                "tool_result" if !self.orphans.contains(&i) => continue,
                "tool_result" if tool_key(event).is_some_and(|key| key.starts_with("task:")) => continue,
                "thinking" if view.merge_thinking => { run.push(Tool { call: i, result: None }); continue; }
                "thinking" => { flush_run(&mut run, &mut items); thinking.push(i); continue; }
                "tool_use" if view.tasks && is_task_call(event.tool_name.as_deref()) => {
                    flush_thinking(&mut thinking, &mut items);
                    flush_run(&mut run, &mut items);
                    tasks_at = Some((items.len(), i));
                    continue;
                }
                "tool_use" if !thinking.is_empty() && joins_thinking(view.thinking, event.tool_name.as_deref()) => { thinking.push(i); continue; }
                _ => {}
            }
            flush_thinking(&mut thinking, &mut items);
            if event.kind == "tool_use" {
                let tool = Tool { call: i, result: self.paired.get(&i).copied() };
                // A página publicada fica fora do grupo como o subagente: ela existe para ser vista.
                if is_agent_call(event.tool_name.as_deref()) || is_page_call(event.tool_name.as_deref()) {
                    flush_run(&mut run, &mut items);
                    items.push((i, Item::Tool(tool)));
                } else { run.push(tool); }
                continue;
            }
            flush_run(&mut run, &mut items);
            items.push((i, if event.kind == "tool_result" { Item::Orphan(i) } else { Item::Event(i) }));
        }
        flush_run(&mut run, &mut items);
        flush_thinking(&mut thinking, &mut items);
        if let Some((at, start)) = tasks_at && !self.tasks.is_empty() {
            items.insert(at, (start, Item::Tasks { id: self.first_task.clone().expect("chamada de tarefa registrada"), tasks: self.tasks.clone() }));
        }
        items
    }
}

/// Estado de redução da atividade; as listas visuais não guardam vínculos pendentes.
#[derive(Default)]
struct ActivityFold {
    resulted: HashSet<String>,
    background: HashMap<String, String>,
    finished_early: HashSet<String>,
    shell_pending: HashSet<String>,
    shell_calls: HashSet<String>,
    agent_calls: HashSet<String>,
    agent_ids: HashMap<String, String>,
    agents: Vec<AgentRun>,
    shells: Vec<ShellRun>,
    created: Vec<(ActivityTask, bool)>,
    whole: Option<Vec<(ActivityTask, bool)>>,
}

impl ActivityFold {
    fn finish(&mut self, id: &str) -> bool {
        if let Some(call) = self.background.get(id) { self.resulted.insert(call.clone()) }
        else { self.finished_early.insert(id.to_owned()); false }
    }

    fn push(&mut self, i: usize, event: &ChatEvent) -> bool {
        match event.kind.as_str() {
            "tool_result" => {
                let Some(id) = tool_key(event) else { return false };
                if let Some(task) = id.strip_prefix("task:") { return self.finish(task); }
                let text = event.result.as_deref().unwrap_or("");
                let mut changed = false;
                if self.agent_calls.contains(id) {
                    // O campo estruturado vence o texto, que muda entre versões do Claude Code.
                    let text_launch = text.to_lowercase().contains("async agent launched");
                    let launched = event.bg_agent_id.as_deref().or_else(|| if text_launch { word_after(text, "agentId:") } else { None });
                    if let Some(agent) = shown_agent_id(launched, text) {
                        changed = self.agent_ids.get(id).map(String::as_str) != Some(agent);
                        if changed { self.agent_ids.insert(id.to_owned(), agent.to_owned()); }
                    }
                    if text_launch || launched.is_some() {
                        if let Some(agent) = launched {
                            if self.finished_early.remove(agent) { changed |= self.resulted.insert(id.to_owned()); }
                            self.background.insert(agent.to_owned(), id.to_owned());
                        }
                        return changed;
                    }
                }
                if self.shell_pending.remove(id) {
                    if let Some(shell) = word_after(text, "Command running in background with ID:") {
                        if self.finished_early.remove(shell) { changed |= self.resulted.insert(id.to_owned()); }
                        self.background.insert(shell.to_owned(), id.to_owned());
                        return changed;
                    }
                }
                // Mesmo um órfão pode concluir uma chamada futura com o mesmo ID.
                let inserted = self.resulted.insert(id.to_owned());
                changed || inserted && (self.agent_calls.contains(id) || self.shell_calls.contains(id))
            }
            "user_msg" => {
                let text = event.text.as_deref().unwrap_or("");
                if !text.contains("<task-notification>") { return false; }
                let Some(task) = text.split_once("<task-id>").and_then(|(_, rest)| rest.split_once("</task-id>")).map(|(id, _)| id.trim()) else { return false };
                self.finish(task)
            }
            "tool_use" => {
                let input = event.tool_input.as_ref();
                let text = |key: &str| input.and_then(|v| v.get(key)).and_then(Value::as_str).map(str::to_owned);
                match event.tool_name.as_deref() {
                    Some("TodoWrite") => { if let Some(list) = whole_list(input, "todos", "content") { self.whole = Some(list); } return true; }
                    Some("update_plan") => { if let Some(list) = whole_list(input, "plan", "step") { self.whole = Some(list); } return true; }
                    Some("TaskCreate") => {
                        let title = text("subject").or_else(|| text("content"))
                            .unwrap_or_else(|| crate::i18n::tr_web("atividade_tarefa_fallback", &HashMap::new()).unwrap_or_else(|| "Tarefa".into()));
                        self.created.push((ActivityTask { title, active_form: text("activeForm"), status: TaskStatus::Pending }, false));
                        return true;
                    }
                    Some(name @ ("TaskUpdate" | "TaskStop")) => {
                        let keys: &[&str] = if name == "TaskStop" { &["task_id", "taskId", "id"] } else { &["taskId", "id"] };
                        let id = loose_id(input, keys);
                        if let Some(task) = self.created.iter_mut().enumerate().find(|(k, _)| (k + 1).to_string() == id).map(|(_, task)| task) {
                            match (name, task_status(input.and_then(|v| v.get("status")))) {
                                ("TaskStop", _) | (_, None) => task.1 = true,
                                (_, Some(status)) => (task.0.status, task.1) = (status, false),
                            }
                        }
                        return true;
                    }
                    _ => {}
                }
                let Some(id) = tool_key(event) else { return false };
                match event.tool_name.as_deref() {
                    Some("Agent" | "AgentSwarm") => {
                        let items = input.and_then(|v| v.get("items")).and_then(Value::as_array).map_or(0, Vec::len);
                        let desc = text("description").or_else(|| text("subagent_type"))
                            .unwrap_or_else(|| crate::i18n::tr_web("atividade_agente", &HashMap::new()).unwrap_or_else(|| "Agente".into()));
                        let description = if items > 0 {
                            let params = HashMap::from([("desc".to_owned(), desc.clone()), ("n".to_owned(), items.to_string())]);
                            crate::i18n::tr_web("atividade_swarm_itens", &params).unwrap_or(desc)
                        } else { desc };
                        self.agent_calls.insert(id.to_owned());
                        self.agents.push(AgentRun { call: i, id: id.to_owned(), description, subagent_type: text("subagent_type"),
                            model: text("model"), prompt: text("prompt"), running: false, agent_id: None });
                        true
                    }
                    Some("Bash") if input.and_then(|v| v.get("run_in_background")).and_then(Value::as_bool) == Some(true) => {
                        self.shell_pending.insert(id.to_owned());
                        self.shell_calls.insert(id.to_owned());
                        let command = text("command").unwrap_or_default();
                        self.shells.push(ShellRun { id: id.to_owned(), label: command_label(&command), command,
                            description: text("description"), ts: event.ts, running: false });
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn snapshot(&self) -> Activity {
        let mut agents = self.agents.clone();
        let mut shells = self.shells.clone();
        for agent in &mut agents {
            agent.running = !self.resulted.contains(&agent.id);
            agent.agent_id = self.agent_ids.get(&agent.id).cloned();
        }
        for shell in &mut shells { shell.running = !self.resulted.contains(&shell.id); }
        agents.sort_by_key(|agent| !agent.running);
        shells.sort_by_key(|shell| !shell.running);
        let tasks = self.whole.as_ref().unwrap_or(&self.created).iter().filter(|(_, deleted)| !deleted).map(|(task, _)| task.clone()).collect();
        Activity { agents, shells, tasks }
    }
}

#[cfg(test)]
mod tests;
