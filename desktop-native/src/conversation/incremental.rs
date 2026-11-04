use std::collections::{HashMap, HashSet, VecDeque};
use serde_json::Value;
use super::{Activity, ActivityTask, AgentRun, Item, ShellRun, Task, TaskStatus, Tool, View,
    command_label, fold_tasks, is_agent_call, is_page_call, is_task_call, joins_thinking, loose_id,
    task_status, tool_key, whole_list, word_after, shown_agent_id, agent_run, codex_agent_identity,
    is_codex_agent_spawn, is_codex_agent_followup, GROUP_MIN};
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
    agent_states: HashMap<String, (bool, Option<f64>)>,
    agent_states_required: bool,
    agents_dirty: bool,
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

    pub fn require_agent_states(&mut self, required: bool) {
        if self.agent_states_required != required {
            self.agent_states_required = required;
            self.agents_dirty = true;
        }
    }

    pub fn set_agent_states(&mut self, states: impl IntoIterator<Item = (String, bool, Option<f64>)>) {
        let states = states.into_iter().map(|(id, running, ended)| (id, (running, ended))).collect();
        if self.agent_states != states {
            self.agent_states = states;
            self.agents_dirty = true;
        }
    }

    pub fn update(&mut self, events: &[ChatEvent], stable: usize, view: View) -> Delta {
        let previous_len = self.items.len();
        let full = self.view != Some(view) || stable < self.cursor || events.len() < self.cursor;
        if full {
            let states = std::mem::take(&mut self.agent_states);
            let required = self.agent_states_required;
            *self = Self::default();
            self.agent_states = states;
            self.agent_states_required = required;
        }
        if !full && self.cursor == events.len() && !self.agents_dirty {
            return Delta { from: previous_len, previous_len, stable_events: stable, full: false, activity_changed: false };
        }
        let mut boundary = self.frontier;
        let previous_task_frontier = self.task_frontier;
        let mut tasks_changed = full;
        let mut activity_changed = full || self.agents_dirty;
        self.agents_dirty = false;
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
            for agent in &mut self.activity.agents {
                if let Some(&(running, ended)) = self.agent_states.get(&agent.id)
                    .or_else(|| match &agent.agent_id {
                        Some(id) => self.agent_states.get(id),
                        None => agent.agent_path.as_ref().and_then(|path| self.agent_states.get(path)),
                    }) {
                    agent.running = running;
                    agent.finished_at = if running { None } else { ended.or(agent.finished_at) };
                } else if self.agent_states_required {
                    agent.running = false;
                }
            }
            self.activity.agents.sort_by_key(|agent| !agent.running);
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
            if !super::hidden_message(event) {
                items.push((i, if event.kind == "tool_result" { Item::Orphan(i) } else { Item::Event(i) }));
            }
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
    resulted: HashMap<String, Option<f64>>,
    background: HashMap<String, String>,
    finished_early: HashMap<String, Option<f64>>,
    // Colega de equipe -> hora do último SendMessage para ele; aviso de ocioso anterior chegou atrasado.
    woken: HashMap<String, f64>,
    // agentId parado esperando o próprio trabalho de fundo; sai no fim ou na retomada.
    waiting: HashSet<String>,
    shell_pending: HashSet<String>,
    shell_calls: HashSet<String>,
    agent_calls: HashSet<String>,
    codex_calls: HashSet<String>,
    codex_resume: HashMap<String, String>,
    agent_ids: HashMap<String, String>,
    agent_descs: HashMap<String, String>,
    agents: Vec<AgentRun>,
    shells: Vec<ShellRun>,
    created: Vec<(ActivityTask, bool)>,
    whole: Option<Vec<(ActivityTask, bool)>>,
}

impl ActivityFold {
    fn finish(&mut self, id: &str, at: Option<f64>) -> bool {
        let waited = self.waiting.remove(id);
        if let Some(call) = self.background.get(id) {
            super::record_completion(&mut self.resulted, call.clone(), at) || waited
        } else { super::record_completion(&mut self.finished_early, id.to_owned(), at); false }
    }

    fn push(&mut self, i: usize, event: &ChatEvent) -> bool {
        match event.kind.as_str() {
            "tool_result" => {
                let Some(id) = tool_key(event) else { return false };
                if let Some(target) = self.codex_resume.remove(id) {
                    if event.is_error == Some(true) { return false; }
                    return self.background.get(&target).is_some_and(|call| self.resulted.remove(call).is_some());
                }
                // Parada pelo usuário: o aviso do Claude Code traz só a descrição, sem task-id. Cada citação fecha o
                // lançamento aberto mais antigo com ela; dois com a mesma descrição não saem juntos.
                if let Some(stopped) = id.strip_prefix("task:stopped:") {
                    let open = self.agents.iter().map(|a| a.id.clone()).find(|call| self.agent_descs.get(call).is_some_and(|d| d == stopped)
                        && self.background.values().any(|c| c == call) && !self.resulted.contains_key(call));
                    return open.is_some_and(|call| super::record_completion(&mut self.resulted, call, event.ts));
                }
                if let Some(task) = id.strip_prefix("task:waiting:") {
                    if matches!((self.woken.get(task), event.ts), (Some(&since), Some(at)) if at < since) { return false; }
                    return self.waiting.insert(task.to_owned());
                }
                if let Some(task) = id.strip_prefix("task:") {
                    if matches!((self.woken.get(task), event.ts), (Some(&since), Some(at)) if at < since) { return false; }
                    return self.finish(task, event.ts);
                }
                let text = event.result.as_deref().unwrap_or("");
                if self.codex_calls.contains(id) && event.is_error != Some(true) {
                    let (agent_id, agent_path) = codex_agent_identity(text);
                    let key = agent_id.as_ref().or(agent_path.as_ref()).cloned();
                    if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
                        agent.agent_id = agent_id;
                        agent.agent_path = agent_path;
                    }
                    if let Some(key) = key {
                        if let Some(at) = self.finished_early.remove(&key) { super::record_completion(&mut self.resulted, id.to_owned(), at); }
                        self.background.insert(key, id.to_owned());
                    } else {
                        super::record_completion(&mut self.resulted, id.to_owned(), event.ts);
                    }
                    return true;
                }
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
                            if let Some(at) = self.finished_early.remove(agent) {
                                changed |= super::record_completion(&mut self.resulted, id.to_owned(), at);
                            }
                            self.background.insert(agent.to_owned(), id.to_owned());
                        }
                        return changed;
                    }
                }
                if self.shell_pending.remove(id) {
                    if let Some(shell) = word_after(text, "Command running in background with ID:") {
                        if let Some(at) = self.finished_early.remove(shell) {
                            changed |= super::record_completion(&mut self.resulted, id.to_owned(), at);
                        }
                        self.background.insert(shell.to_owned(), id.to_owned());
                        return changed;
                    }
                }
                // Mesmo um órfão pode concluir uma chamada futura com o mesmo ID.
                let inserted = super::record_completion(&mut self.resulted, id.to_owned(), event.ts);
                changed || inserted && (self.agent_calls.contains(id) || self.shell_calls.contains(id))
            }
            "user_msg" => {
                let text = event.text.as_deref().unwrap_or("");
                if let Some(notice) = crate::cards::codex_subagent(text) {
                    return matches!(notice.status.as_str(), "completed" | "errored" | "failed" | "shutdown" | "not_found")
                        && self.finish(&notice.agent_path, event.ts);
                }
                if !text.contains("<task-notification>") || super::interim_notification(text) { return false; }
                let Some(task) = text.split_once("<task-id>").and_then(|(_, rest)| rest.split_once("</task-id>")).map(|(id, _)| id.trim()) else { return false };
                self.finish(task, event.ts)
            }
            "tool_use" => {
                let input = event.tool_input.as_ref();
                let text = |key: &str| input.and_then(|v| v.get(key)).and_then(Value::as_str).map(str::to_owned);
                if is_codex_agent_followup(event.tool_name.as_deref()) {
                    if let Some(target) = text("target").or_else(|| text("agent_id"))
                        && self.background.contains_key(&target) && let Some(send) = tool_key(event) {
                        self.codex_resume.insert(send.to_owned(), target);
                    }
                    return false;
                }
                match event.tool_name.as_deref() {
                    Some("TodoWrite") => { if let Some(list) = whole_list(input, "todos", "content") { self.whole = Some(list); } return true; }
                    Some("update_plan") => { if let Some(list) = whole_list(input, "plan", "step") { self.whole = Some(list); } return true; }
                    // Colega de equipe trabalha em rodadas: a mensagem reabre o que o aviso de ocioso fechou.
                    Some("SendMessage") => {
                        let mut changed = false;
                        for (key, call) in &self.background {
                            if !super::wakes(input, key) { continue; }
                            changed |= self.resulted.remove(call).is_some() | self.waiting.remove(key);
                            if let Some(at) = event.ts { self.woken.insert(key.clone(), at); }
                        }
                        return changed;
                    }
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
                    name if is_agent_call(name) => {
                        self.agent_calls.insert(id.to_owned());
                        if is_codex_agent_spawn(name) { self.codex_calls.insert(id.to_owned()); }
                        if let Some(raw) = text("description") { self.agent_descs.insert(id.to_owned(), raw); }
                        self.agents.push(agent_run(event, i, id));
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
        let waiting: HashSet<&str> = self.waiting.iter().filter_map(|id| self.background.get(id).map(String::as_str)).collect();
        for agent in &mut agents {
            agent.running = !self.resulted.contains_key(&agent.id);
            agent.waiting = agent.running && waiting.contains(agent.id.as_str());
            agent.finished_at = self.resulted.get(&agent.id).copied().flatten();
            if let Some(id) = self.agent_ids.get(&agent.id) { agent.agent_id = Some(id.clone()); }
        }
        for shell in &mut shells { shell.running = !self.resulted.contains_key(&shell.id); }
        agents.sort_by_key(|agent| !agent.running);
        shells.sort_by_key(|shell| !shell.running);
        let tasks = self.whole.as_ref().unwrap_or(&self.created).iter().filter(|(_, deleted)| !deleted).map(|(task, _)| task.clone()).collect();
        Activity { agents, shells, tasks }
    }
}

#[cfg(test)]
mod tests;
