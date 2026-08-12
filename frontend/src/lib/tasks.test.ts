import { describe, it, expect } from 'vitest';
import { foldTasks, contarTasks, type Task } from './tasks';
import type { ChatEvent } from './types';

// Helpers no shape REAL medido no transcript (ver o cabeçalho de tasks.ts).
let seq = 0;
function create(subject: string, extras: Record<string, unknown> = {}): ChatEvent {
  seq += 1;
  return {
    id: `e${seq}`, kind: 'tool_use', tool_use_id: `tu${seq}`, tool_name: 'TaskCreate',
    tool_input: { subject, description: 'desc de ' + subject, activeForm: 'Fazendo ' + subject, ...extras },
  } as ChatEvent;
}
function update(taskId: string, input: Record<string, unknown>): ChatEvent {
  seq += 1;
  return {
    id: `e${seq}`, kind: 'tool_use', tool_use_id: `tu${seq}`, tool_name: 'TaskUpdate',
    tool_input: { taskId, ...input },
  } as ChatEvent;
}
/** O id da tarefa só existe no TEXTO do resultado do create. */
function resultados(pares: [string, string][]) {
  const m = new Map<string, ChatEvent>();
  for (const [toolUseId, texto] of pares) {
    m.set(toolUseId, { id: 'r', kind: 'tool_result', result: texto } as ChatEvent);
  }
  return (id: string) => m.get(id);
}

describe('foldTasks', () => {
  it('pega o id no texto do resultado, não no input do create', () => {
    const c = create('Portar Task Rows');
    const out = foldTasks([c], resultados([[c.tool_use_id!, 'Task #7 created successfully: Portar Task Rows']]));
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe('7');
    expect(out[0].status).toBe('pending');
    expect(out[0].activeForm).toBe('Fazendo Portar Task Rows');
  });

  it('aplica o update de status por id', () => {
    const c = create('A');
    const evs = [c, update('3', { status: 'in_progress' }), update('3', { status: 'completed' })];
    const out = foldTasks(evs, resultados([[c.tool_use_id!, 'Task #3 created successfully: A']]));
    expect(out[0].status).toBe('completed');
  });

  it('remove a tarefa apagada', () => {
    const c1 = create('fica'); const c2 = create('some');
    const out = foldTasks([c1, c2, update('2', { status: 'deleted' })], resultados([
      [c1.tool_use_id!, 'Task #1 created successfully: fica'],
      [c2.tool_use_id!, 'Task #2 created successfully: some'],
    ]));
    expect(out.map((t: Task) => t.subject)).toEqual(['fica']);
  });

  it('ignora update de id que não existe em vez de inventar linha', () => {
    // Acontece de verdade: outra janela/sessão mexendo na mesma lista.
    const out = foldTasks([update('99', { status: 'completed' })], resultados([]));
    expect(out).toEqual([]);
  });

  it('mantém a tarefa na tela mesmo antes de o resultado chegar (id ainda vazio)', () => {
    // Sumir até o id chegar faria a tarefa piscar na tela a cada criação.
    const c = create('recem criada');
    const out = foldTasks([c], resultados([]));
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe('');
    expect(out[0].status).toBe('pending');
  });

  it('respeita a ordem de criação, não a de atualização', () => {
    const c1 = create('primeira'); const c2 = create('segunda');
    const out = foldTasks([c1, c2, update('2', { status: 'completed' })], resultados([
      [c1.tool_use_id!, 'Task #1 created successfully: primeira'],
      [c2.tool_use_id!, 'Task #2 created successfully: segunda'],
    ]));
    expect(out.map((t: Task) => t.subject)).toEqual(['primeira', 'segunda']);
  });

  it('update pode reescrever o texto da tarefa', () => {
    const c = create('nome velho');
    const out = foldTasks([c, update('1', { subject: 'nome novo' })],
      resultados([[c.tool_use_id!, 'Task #1 created successfully: nome velho']]));
    expect(out[0].subject).toBe('nome novo');
  });

  it('ignora status desconhecido em vez de estourar', () => {
    const c = create('A');
    const out = foldTasks([c, update('1', { status: 'coisa-nova-do-futuro' })],
      resultados([[c.tool_use_id!, 'Task #1 created successfully: A']]));
    expect(out[0].status).toBe('pending');
  });
});

describe('contarTasks', () => {
  it('conta só as concluídas', () => {
    const tasks = [
      { id: '1', subject: 'a', description: '', activeForm: '', status: 'completed' },
      { id: '2', subject: 'b', description: '', activeForm: '', status: 'in_progress' },
      { id: '3', subject: 'c', description: '', activeForm: '', status: 'pending' },
    ] as Task[];
    expect(contarTasks(tasks)).toEqual({ feitas: 1, total: 3 });
  });
});
