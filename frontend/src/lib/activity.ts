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

export interface ActivityFolder {
  push(e: ChatEvent): void;
  reset(events: ChatEvent[]): void;
  snapshot(): Activity;
}

// Fold INCREMENTAL: o Chat alimenta evento a evento (push) conforme chegam do SSE, em vez de
// re-varrer o array inteiro a cada mensagem (deriveActivity como $derived era O(n) por evento e
// crescia com o historico). reset() refaz do zero (reseed do history / /clear); snapshot()
// materializa o Activity (custo O(tasks+agentes), pequeno).
export function createActivityFolder(): ActivityFolder {
  let resulted = new Set<string>();      // tool_use_id que ja tem tool_result (agente terminou)
  let byId = new Map<string, TaskItem>();
  let order: string[] = [];
  let todoWrite: TaskItem[] | null = null;
  let createSeq = 0;
  let agents: { id: string; kind: 'agent' | 'workflow'; description: string }[] = [];
  // Agent em BACKGROUND: o tool_result chega NA HORA ("Async agent launched...") com o agentId no
  // texto — o agente segue rodando e o fim real chega como <task-notification> numa user_msg.
  // Sem tratar isso, todo agent background aparecia como terminado (painel dizia "Nada rolando"
  // com agents ativos). Mapa: agentId (= task-id da notificação) -> tool_use_id do launch.
  let bgAgent = new Map<string, string>();

  function push(e: ChatEvent): void {
    if (e.kind === 'tool_result' && e.tool_use_id) {
      // tool_result sintetico do backend (transcript.py): <task-notification> virou "task:<id>".
      // Resolve o launch background correspondente e marca como terminado.
      if (e.tool_use_id.startsWith('task:')) {
        const tuid = bgAgent.get(e.tool_use_id.slice(5));
        if (tuid) resulted.add(tuid);
        return;
      }
      const r = e.result ?? '';
      if (/Async agent launched/i.test(r)) {
        const m = r.match(/agentId:\s*([A-Za-z0-9_-]+)/);
        if (m) bgAgent.set(m[1], e.tool_use_id);
        return; // launch imediato: NAO marca resulted — o agente continua rodando
      }
      resulted.add(e.tool_use_id);
      return;
    }
    if (e.kind === 'user_msg' && e.text && e.text.includes('<task-notification>')) {
      const m = e.text.match(/<task-id>([^<]+)<\/task-id>/);
      const tuid = m ? bgAgent.get(m[1].trim()) : undefined;
      if (tuid) resulted.add(tuid);
      return;
    }
    if (e.kind !== 'tool_use' || !e.tool_name) return;
    const input = (e.tool_input ?? {}) as Record<string, unknown>;

    // Tarefas: este build usa TaskCreate/TaskUpdate (incremental, event-sourced) — é preciso FOLDAR
    // o stream, não "pegar o último". Suporta também TodoWrite stock (lista inteira por chamada)
    // como fallback: se houver, a última vence (é canônica).
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
      case 'Agent': {
        if (e.tool_use_id) {
          agents.push({
            id: e.tool_use_id,
            kind: 'agent',
            description: String(input.description ?? input.subagent_type ?? 'Agente'),
          });
        }
        break;
      }
      case 'Workflow': {
        // Workflow roda em background e devolve o tool_result na hora -> não dá pra inferir o
        // término só pelos eventos (o Chat polla o backend enquanto houver razão).
        if (e.tool_use_id) {
          agents.push({
            id: e.tool_use_id,
            kind: 'workflow',
            description: workflowName(input.script) ?? 'Workflow',
          });
        }
        break;
      }
    }
  }

  function reset(events: ChatEvent[]): void {
    resulted = new Set();
    byId = new Map();
    order = [];
    todoWrite = null;
    createSeq = 0;
    agents = [];
    bgAgent = new Map();
    for (const e of events) push(e);
  }

  function snapshot(): Activity {
    // Copia os itens: o fold MUTA os TaskItem internos (TaskUpdate) — sem copiar, um snapshot
    // antigo mudaria por baixo do estado que a UI segura.
    const all = (todoWrite ?? order.map((id) => byId.get(id)).filter((t): t is TaskItem => !!t))
      .map((t) => ({ ...t }));
    const tasks = all.filter((t) => t.status !== 'deleted');
    // running calculado AQUI (não no push): o tool_result do agente chega DEPOIS do tool_use.
    // Agent é bloqueante (sem result = rodando); Workflow devolve result imediato -> nunca "rodando".
    const runs: AgentRun[] = agents.map((a) => ({
      ...a,
      running: a.kind === 'agent' && !resulted.has(a.id),
    }));
    runs.sort((a, b) => Number(b.running) - Number(a.running));
    const done = tasks.filter((t) => t.status === 'completed').length;
    const inProgress = tasks.filter((t) => t.status === 'in_progress').length;
    const runningAgents = runs.filter((a) => a.running).length;
    return { tasks, agents: runs, total: tasks.length, done, inProgress, runningAgents };
  }

  return { push, reset, snapshot };
}

export function deriveActivity(events: ChatEvent[]): Activity {
  const f = createActivityFolder();
  f.reset(events);
  return f.snapshot();
}
