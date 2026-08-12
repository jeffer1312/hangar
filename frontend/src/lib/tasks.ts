// Reconstrução da lista de tarefas do agente a partir do transcript.
//
// Diferente do TodoWrite (que mandava a lista INTEIRA a cada chamada), o TaskCreate/TaskUpdate do
// Claude Code é INCREMENTAL — medido no transcript desta máquina em 2026-08-11:
//
//   tool_use  TaskCreate {"subject": "...", "description": "...", "activeForm": "..."}
//   result    "Task #2 created successfully: <subject>"          <- o id só existe AQUI
//   tool_use  TaskUpdate {"taskId": "2", "status": "in_progress"}
//
// Ou seja: nenhum evento sozinho carrega a lista. Ela é o resultado de dobrar o fluxo na ordem,
// pegando o id no texto do RESULTADO do create e aplicando os updates por cima. Por isso isto é
// uma função pura testável, e não um `$derived` escondido dentro do componente.
import type { ChatEvent } from './types';

export type TaskStatus = 'pending' | 'in_progress' | 'completed';

export interface Task {
  id: string;
  subject: string;
  description: string;
  /** Forma no gerúndio ("Portando o Task Rows") — o que o agente mostra enquanto trabalha nela. */
  activeForm: string;
  status: TaskStatus;
}

// "Task #2 created successfully: ..." — o id nasce no texto do resultado, não no input.
const ID_NO_RESULTADO = /Task\s+#(\d+)/i;

function comoTexto(v: unknown): string {
  return typeof v === 'string' ? v : '';
}

/**
 * Dobra os eventos numa lista de tarefas, na ordem em que foram criadas.
 *
 * `resultadoDe` devolve o tool_result de um tool_use (o mesmo mapa que o MessageList já mantém).
 * Um create SEM resultado ainda não tem id — ele entra na lista assim mesmo, com id vazio, porque
 * sumir da tela até o resultado chegar faria a tarefa piscar; ele só não pode receber updates.
 */
export function foldTasks(
  events: ChatEvent[],
  resultadoDe: (toolUseId: string) => ChatEvent | undefined | null,
): Task[] {
  const porId = new Map<string, Task>();
  const semId: Task[] = [];
  const ordem: Task[] = [];

  for (const ev of events) {
    if (ev.kind !== 'tool_use') continue;
    const input = (ev.tool_input ?? {}) as Record<string, unknown>;

    if (ev.tool_name === 'TaskCreate') {
      const res = resultadoDe(ev.tool_use_id ?? '');
      const m = comoTexto(res?.result).match(ID_NO_RESULTADO);
      const t: Task = {
        id: m ? m[1] : '',
        subject: comoTexto(input['subject']) || '(sem título)',
        description: comoTexto(input['description']),
        activeForm: comoTexto(input['activeForm']),
        status: 'pending',
      };
      ordem.push(t);
      if (t.id) porId.set(t.id, t);
      else semId.push(t);
      continue;
    }

    if (ev.tool_name === 'TaskUpdate') {
      const id = comoTexto(input['taskId']);
      const alvo = porId.get(id);
      if (!alvo) continue;   // update de tarefa de OUTRA sessão/janela: ignora, não inventa linha
      const st = comoTexto(input['status']);
      if (st === 'deleted') {
        porId.delete(id);
        const i = ordem.indexOf(alvo);
        if (i >= 0) ordem.splice(i, 1);
        continue;
      }
      if (st === 'pending' || st === 'in_progress' || st === 'completed') alvo.status = st;
      const novoSubject = comoTexto(input['subject']);
      if (novoSubject) alvo.subject = novoSubject;
      const novaDesc = comoTexto(input['description']);
      if (novaDesc) alvo.description = novaDesc;
      const novoActive = comoTexto(input['activeForm']);
      if (novoActive) alvo.activeForm = novoActive;
    }
  }

  void semId;   // mantidos em `ordem`; a lista separada existe só pra deixar a intenção explícita
  return ordem;
}

/** Contagem pro cabeçalho ("2/5"). */
export function contarTasks(tasks: Task[]): { feitas: number; total: number } {
  return { feitas: tasks.filter((t) => t.status === 'completed').length, total: tasks.length };
}
