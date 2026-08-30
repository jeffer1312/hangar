import * as m from '../paraglide/messages';
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
  // O que o subagente foi mandado FAZER. A linha "Rodando agora" mostrava só o `description` (3-5
  // palavras) e não dava pra saber o que ele está tocando. O tool_use já traz tudo isto no input —
  // era só não jogar fora. O transcript PRÓPRIO do subagente não vem no jsonl do pai, então isto é
  // o teto do que dá pra mostrar sem backend novo.
  subagentType?: string;
  model?: string;
  prompt?: string;
}

// Shell de fundo (`Bash` com `run_in_background: true`). Fecha pelo MESMO caminho do agente em
// background — a `<task-notification>` que o backend já converte em `tool_result` sintético
// `task:<id>` —, porque no harness os dois são a mesma coisa: um trabalho que continua depois que
// a ferramenta respondeu. O terminal mostra isso como "5 shells still running"; o app não mostrava.
// Limite conhecido, e é o mesmo que já vale pros agentes e pras tarefas: um `/clear` troca o
// transcript e zera esta lista, mas NÃO mata o processo — um build de fundo continua de pé e o
// contador some. Corrigir isso pediria estado fora do transcript (o app inteiro deriva daqui), e
// manter linhas de um transcript descartado traria de volta shells que já terminaram.
export interface ShellRun {
  id: string;        // tool_use_id
  command: string;   // o comando CRU (vai no title, pra quem quiser o texto inteiro)
  rotulo: string;    // o mesmo comando sem o encanamento — é o que a lista mostra
  description?: string;
  ts?: number;       // quando começou (epoch s), pro rótulo de tempo decorrido na folha
  running: boolean;  // sem a notificação de fim = ainda rodando
}

// Encanamento de shell que não diz NADA sobre o que o comando faz, e ocupa a linha inteira numa
// lista estreita: o `cd <caminho> &&` da frente, as redireções, e o `| tail -N` do fim.
// Medido nesta sessão: dos 18 shells de fundo, ZERO tinham o campo `description` preenchido — então
// o comando é a única fonte de rótulo que existe, e limpá-lo é o que dá pra fazer.
// É PODA, não interpretação: nada aqui tenta adivinhar o que o comando significa, e o texto cru
// continua inteiro no `command` (que a linha usa como `title`).
const _RUIDO: RegExp[] = [
  /^\s*cd\s+(?:"[^"]*"|'[^']*'|\S+)\s*&&\s*/,          // "cd /caminho && " da frente
  /\s*\d?>\s*(?:&\d|\/dev\/null)/g,                      // >/dev/null, 2>&1, 2>/dev/null
  /\s*\|\s*(?:tail|head)\s+-\w+\s*\d*\s*$/,              // | tail -4 no fim
];

export function rotuloDeComando(cmd: string): string {
  let s = cmd;
  for (const re of _RUIDO) s = s.replace(re, ' ');
  s = s.replace(/\s+/g, ' ').trim();
  // Poda que comeu tudo (o comando era só redireção) -> devolve o cru: rótulo vazio seria pior.
  return s || cmd.trim();
}

export interface Activity {
  tasks: TaskItem[]; // não-deletadas, em ordem
  agents: AgentRun[]; // rodando primeiro
  shells: ShellRun[]; // rodando primeiro
  total: number;
  done: number;
  inProgress: number;
  runningAgents: number;
  runningShells: number;
}

function normStatus(s: unknown): TaskStatus {
  return s === 'in_progress' || s === 'completed' || s === 'deleted' ? s : 'pending';
}

function workflowName(script: unknown): string | null {
  if (typeof script !== 'string') return null;
  const match = script.match(/name:\s*['"]([^'"]+)['"]/);
  return match ? match[1] : null;
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
  let agents: Omit<AgentRun, 'running'>[] = [];
  // Agent em BACKGROUND: o tool_result chega NA HORA ("Async agent launched...") com o agentId no
  // texto — o agente segue rodando e o fim real chega como <task-notification> numa user_msg.
  // Sem tratar isso, todo agent background aparecia como terminado (painel dizia "Nada rolando"
  // com agents ativos). Mapa: agentId (= task-id da notificação) -> tool_use_id do launch.
  let bgAgent = new Map<string, string>();
  // Shells de fundo, e o conjunto dos tool_use que AINDA esperam o "Command running in background
  // with ID". O conjunto não é zelo: o texto do lançamento é procurado no resultado, e a saída de
  // um `grep`/`cat` qualquer pode conter essa frase (este próprio projeto tem ela em comentário).
  // Sem saber que AQUELE tool_use era um Bash de fundo, um resultado assim viraria um lançamento
  // que nenhuma notificação fecha, e a ferramenta ficaria "rodando" pra sempre.
  let shells: Omit<ShellRun, 'running'>[] = [];
  let bgPendente = new Set<string>();
  // agentId cujo FIM chegou ANTES de o launch ser mapeado (ex: a troca de transcript no /clear reordena
  // o fold, ou o launch cai num transcript e o fim no outro). Sem isto o par nao fechava e o agente
  // ficava "rodando" pra sempre. Guardado aqui e resolvido quando o launch aparecer -> pareamento
  // INDEPENDENTE DE ORDEM.
  let completedIds = new Set<string>();
  // Marca um agente background como terminado pelo agentId, com o launch vindo ANTES ou DEPOIS do fim.
  function completeAgent(agentId: string): void {
    const tuid = bgAgent.get(agentId);
    if (tuid) resulted.add(tuid);
    else completedIds.add(agentId);   // launch ainda nao visto -> resolvido quando ele chegar
  }

  function push(e: ChatEvent): void {
    if (e.kind === 'tool_result' && e.tool_use_id) {
      // tool_result sintetico do backend (transcript.py): <task-notification> virou "task:<id>".
      // Resolve o launch background correspondente e marca como terminado.
      if (e.tool_use_id.startsWith('task:')) {
        completeAgent(e.tool_use_id.slice(5));
        return;
      }
      const r = e.result ?? '';
      if (/Async agent launched/i.test(r)) {
        const match = r.match(/agentId:\s*([A-Za-z0-9_-]+)/);
        if (match) {
          bgAgent.set(match[1], e.tool_use_id);
          // o fim ja tinha chegado antes do launch (reorder do reseed) -> fecha o par agora.
          if (completedIds.delete(match[1])) resulted.add(e.tool_use_id);
        }
        return; // launch imediato: so marca resulted se o fim ja veio; senao segue rodando
      }
      if (bgPendente.delete(e.tool_use_id)) {
        const bg = r.match(/Command running in background with ID:\s*([A-Za-z0-9_-]+)/);
        if (bg) {
          bgAgent.set(bg[1], e.tool_use_id);
          if (completedIds.delete(bg[1])) resulted.add(e.tool_use_id);
          return;   // foi pro fundo: quem fecha e a <task-notification>
        }
        // Pediu fundo e NAO foi (recusa do harness, erro de validacao): o resultado ja e o final
        // dele. Cair no `resulted` abaixo e o certo — senao ficaria "rodando" pra sempre.
      }
      resulted.add(e.tool_use_id);
      return;
    }
    if (e.kind === 'user_msg' && e.text && e.text.includes('<task-notification>')) {
      const match = e.text.match(/<task-id>([^<]+)<\/task-id>/);
      if (match) completeAgent(match[1].trim());
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
      // O plano do Codex. Mesma natureza do TodoWrite — a lista INTEIRA a cada chamada, a última
      // vence —, só com outros nomes de campo: `plan[].step` no lugar de `todos[].content`, e o
      // mesmo trio de status. Sem este caso ele chegava como ferramenta anônima e o painel de
      // tarefas ficava vazio numa sessão que tinha plano.
      case 'update_plan': {
        const plano = input.plan;
        if (Array.isArray(plano)) {
          todoWrite = plano
            .filter((t): t is Record<string, unknown> => !!t && typeof t === 'object' && typeof (t as Record<string, unknown>).step === 'string')
            .map((t, i) => ({
              id: String(i),
              title: String(t.step),
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
          title: String(input.subject ?? input.content ?? m.atividade_tarefa_fallback()),
          activeForm: typeof input.activeForm === 'string' ? input.activeForm : undefined,
          status: 'pending',
        });
        order.push(id);
        break;
      }
      case 'Bash': {
        // So o de FUNDO entra: o Bash comum bloqueia e ja acabou quando o resultado chega.
        if (e.tool_use_id && input.run_in_background === true) {
          bgPendente.add(e.tool_use_id);
          const cmd = String(input.command ?? '');
          shells.push({
            id: e.tool_use_id,
            command: cmd,
            rotulo: rotuloDeComando(cmd),
            description: typeof input.description === 'string' ? input.description : undefined,
            ts: typeof e.ts === 'number' ? e.ts : undefined,
          });
        }
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
      // AgentSwarm é a versão em LOTE do Agent, no Kimi: um subagente por item da lista (o terminal
      // do Kimi desenha uma coluna numerada por item). Cai no mesmo caso porque é UM tool_use, com
      // UM tool_result — inventar um item por `items[i]` daria ids que nenhum resultado fecha, e os
      // quatro ficariam "em execução" pra sempre. Quem mostra um a um é a lista de subagentes do
      // painel, que lê os wires de cada um.
      case 'Agent':
      case 'AgentSwarm': {
        if (e.tool_use_id) {
          const itens = Array.isArray(input.items) ? input.items.length : 0;
          const desc = String(input.description ?? input.subagent_type ?? m.atividade_agente());
          agents.push({
            id: e.tool_use_id,
            kind: 'agent',
            description: itens ? m.atividade_swarm_itens({ desc, n: itens }) : desc,
            subagentType: typeof input.subagent_type === 'string' ? input.subagent_type : undefined,
            model: typeof input.model === 'string' ? input.model : undefined,
            prompt: typeof input.prompt === 'string' ? input.prompt : undefined,
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
            description: workflowName(input.script) ?? m.atividade_workflow(),
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
    completedIds = new Set();
    shells = [];
    bgPendente = new Set();
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
    // Shell de fundo: enquanto o tool_use nao tiver a notificacao de fim, esta rodando. Mesmo
    // criterio do Agent em background, e pelo mesmo caminho (`resulted`).
    const shellRuns: ShellRun[] = shells.map((s) => ({ ...s, running: !resulted.has(s.id) }));
    shellRuns.sort((a, b) => Number(b.running) - Number(a.running));
    const done = tasks.filter((t) => t.status === 'completed').length;
    const inProgress = tasks.filter((t) => t.status === 'in_progress').length;
    const runningAgents = runs.filter((a) => a.running).length;
    const runningShells = shellRuns.filter((s) => s.running).length;
    return { tasks, agents: runs, shells: shellRuns, total: tasks.length, done, inProgress,
             runningAgents, runningShells };
  }

  return { push, reset, snapshot };
}

export function deriveActivity(events: ChatEvent[]): Activity {
  const f = createActivityFolder();
  f.reset(events);
  return f.snapshot();
}
