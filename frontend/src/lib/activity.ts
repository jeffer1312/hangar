import type { ChatEvent } from './types';

// Painel de atividade: deriva, SÓ no cliente, a lista de tarefas (TaskCreate/TaskUpdate/TaskStop,
// ou TodoWrite stock) + os subagentes rodando (Agent/Workflow) a partir dos eventos que já chegam
// no app via SSE. Zero backend — os tool_use já estão no transcript que o app taila.

export type TaskStatus = 'pending' | 'in_progress' | 'completed' | 'deleted';

export interface TaskItem {
  id: string;
  title: string; // subject (TaskCreate) ou content (TodoWrite)
  activeForm?: string; // rótulo no gerúndio (ex: "Lendo o tool-loop")
  status: TaskStatus;
}

export interface AgentRun {
  id: string; // tool_use_id
  kind: 'agent' | 'workflow';
  description: string;
  running: boolean; // Agent é bloqueante: sem tool_result = ainda rodando
}

export interface Activity {
  tasks: TaskItem[]; // não-deletadas, em ordem
  agents: AgentRun[]; // rodando primeiro
  total: number;
  done: number;
  inProgress: number;
  runningAgents: number;
}

function normStatus(s: unknown): TaskStatus {
  return s === 'in_progress' || s === 'completed' || s === 'deleted' ? s : 'pending';
}

function workflowName(script: unknown): string | null {
  if (typeof script !== 'string') return null;
  const m = script.match(/name:\s*['"]([^'"]+)['"]/);
  return m ? m[1] : null;
}

export function deriveActivity(events: ChatEvent[]): Activity {
  // tool_use_id que já tem tool_result -> usado pra detectar agente ainda rodando.
  const resulted = new Set<string>();
  for (const e of events) {
    if (e.kind === 'tool_result' && e.tool_use_id) resulted.add(e.tool_use_id);
  }

  // ── Tarefas ──────────────────────────────────────────────────────────────
  // Este build usa TaskCreate/TaskUpdate (incremental, event-sourced) — é preciso FOLDAR o
  // stream, não "pegar o último". Suporta também TodoWrite stock (lista inteira por chamada) como
  // fallback: se houver, a última vence (é canônica).
  const byId = new Map<string, TaskItem>();
  const order: string[] = [];
  let todoWrite: TaskItem[] | null = null;
  let createSeq = 0;

  for (const e of events) {
    if (e.kind !== 'tool_use' || !e.tool_name) continue;
    const input = (e.tool_input ?? {}) as Record<string, unknown>;

    switch (e.tool_name) {
      case 'TodoWrite': {
        let todos: unknown = input.todos;
        if (typeof todos === 'string') {
          try { todos = JSON.parse(todos); } catch { todos = null; }
        }
        if (Array.isArray(todos)) {
          todoWrite = todos
            .filter((t): t is Record<string, unknown> => !!t && typeof t === 'object' && typeof (t as Record<string, unknown>).content === 'string')
            .map((t, i) => ({
              id: String(i),
              title: String(t.content),
              activeForm: typeof t.activeForm === 'string' ? t.activeForm : undefined,
              status: normStatus(t.status),
            }));
        }
        break;
      }
      case 'TaskCreate': {
        createSeq += 1;
        const id = String(createSeq); // TaskUpdate.taskId é o id sequencial "1","2",...
        byId.set(id, {
          id,
          title: String(input.subject ?? input.content ?? 'Tarefa'),
          activeForm: typeof input.activeForm === 'string' ? input.activeForm : undefined,
          status: 'pending',
        });
        order.push(id);
        break;
      }
      case 'TaskUpdate': {
        const id = String(input.taskId ?? input.id ?? '');
        const item = byId.get(id);
        if (item) item.status = normStatus(input.status);
        break;
      }
      case 'TaskStop': {
        const id = String(input.task_id ?? input.taskId ?? input.id ?? '');
        const item = byId.get(id);
        if (item) item.status = 'deleted';
        break;
      }
    }
  }

  const all = todoWrite ?? order.map((id) => byId.get(id)).filter((t): t is TaskItem => !!t);
  const tasks = all.filter((t) => t.status !== 'deleted');

  // ── Agentes / Workflows ────────────────────────────────────────────────────
  const agents: AgentRun[] = [];
  for (const e of events) {
    if (e.kind !== 'tool_use' || !e.tool_use_id) continue;
    const input = (e.tool_input ?? {}) as Record<string, unknown>;
    if (e.tool_name === 'Agent') {
      agents.push({
        id: e.tool_use_id,
        kind: 'agent',
        description: String(input.description ?? input.subagent_type ?? 'Agente'),
        running: !resulted.has(e.tool_use_id), // bloqueante: sem result = rodando
      });
    } else if (e.tool_name === 'Workflow') {
      // Workflow roda em background e devolve o tool_result na hora -> não dá pra inferir o
      // término só pelos eventos (precisaria ler os arquivos do run no disco; fica pra depois).
      agents.push({
        id: e.tool_use_id,
        kind: 'workflow',
        description: workflowName(input.script) ?? 'Workflow',
        running: false,
      });
    }
  }
  agents.sort((a, b) => Number(b.running) - Number(a.running));

  const done = tasks.filter((t) => t.status === 'completed').length;
  const inProgress = tasks.filter((t) => t.status === 'in_progress').length;
  const runningAgents = agents.filter((a) => a.running).length;

  return { tasks, agents, total: tasks.length, done, inProgress, runningAgents };
}
