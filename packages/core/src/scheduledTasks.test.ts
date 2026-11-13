import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import { createScheduledTask, deleteScheduledTask, getScheduledTasks, runScheduledTask, setScheduledTaskEnabled, updateScheduledTask, validateScheduledTask, taskDateToInstant, taskDateParts, reuseUnchangedTasks, type ScheduledTask, type ScheduledTaskInput } from './scheduledTasks';

const server = { id: 'test', label: 'Servidor', baseUrl: 'https://tasks.test', token: 'owner' };
const input: ScheduledTaskInput = { name: 'revisar', prompt: 'revisar projeto', enabled: true,
  schedule: { kind: 'weekly', time: '09:00', weekdays: [1, 5] }, target: { kind: 'session', name: 'projeto', session_key: 'vida-original' } };
beforeEach(() => {
  _resetApiEnvForTests();
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized() {}, origin: null, createEventSource: vi.fn() });
});
afterEach(() => { vi.restoreAllMocks(); _resetApiEnvForTests(); });

it('envia revisão e identidade da conversa sem perder o servidor escolhido', async () => {
  const fetch = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{}'));
  const calls = [
    [() => getScheduledTasks(server), 'GET', '/api/scheduled-tasks', undefined],
    [() => createScheduledTask(server, input), 'POST', '/api/scheduled-tasks', input],
    [() => createScheduledTask(server, { ...input, request_id: 'pedido-1' }), 'POST', '/api/scheduled-tasks', { ...input, request_id: 'pedido-1' }],
    [() => updateScheduledTask(server, 'a/b', 8, input), 'PUT', '/api/scheduled-tasks/a%2Fb', { ...input, revision: 8 }],
    [() => setScheduledTaskEnabled(server, 'a/b', 9, false), 'PATCH', '/api/scheduled-tasks/a%2Fb/enabled', { enabled: false, revision: 9 }],
    [() => runScheduledTask(server, 'a/b'), 'POST', '/api/scheduled-tasks/a%2Fb/run', undefined],
    [() => deleteScheduledTask(server, 'a/b', 10), 'DELETE', '/api/scheduled-tasks/a%2Fb?revision=10', undefined],
  ] as const;
  for (const [call, method, path, body] of calls) {
    fetch.mockResolvedValueOnce(new Response('{}'));
    await call();
    expect(fetch).toHaveBeenCalled();
    const [url, init] = fetch.mock.calls.at(-1)!;
    expect(url).toBe(`https://tasks.test${path}`);
    expect(init?.method ?? 'GET').toBe(method);
    expect(init?.body ? JSON.parse(String(init.body)) : undefined).toEqual(body);
    expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer owner');
  }
});

it('preserva o código de edição concorrente para manter o rascunho no cliente', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ detail: { code: 'scheduled_revision_conflict', msg: 'changed' } }), { status: 409 }));
  await expect(updateScheduledTask(server, 'a', 1, input)).rejects.toMatchObject({ status: 409, code: 'scheduled_revision_conflict' });
});

it('recusa horários e destinos inválidos antes de salvar', () => {
  expect(validateScheduledTask(input, Date.parse('2026-11-12T00:00:00Z'))).toBeNull();
  expect(validateScheduledTask({ ...input, schedule: { kind: 'weekly', weekdays: [], time: '09:00' } })).toBe('days');
  expect(validateScheduledTask({ ...input, schedule: { kind: 'weekly', weekdays: [1], time: '25:00' } })).toBe('time');
  expect(validateScheduledTask({ ...input, schedule: { kind: 'interval', minutes: 1.2 } })).toBe('interval');
  expect(validateScheduledTask({ ...input, schedule: { kind: 'once', at: '2026-11-20T09:00:00' } })).toBe('date');
  expect(validateScheduledTask({ ...input, schedule: { kind: 'once', at: '2026-02-30T09:00:00-03:00' } }, 0)).toBe('date');
  expect(validateScheduledTask({ ...input, target: { kind: 'new', launch: {}, worktree: false } })).toBe('folder');
});

it('converte a data da máquina sem usar o fuso do celular e recusa o salto de horário de verão', () => {
  expect(taskDateToInstant('18/11/2026', '09:00', 'America/Sao_Paulo')).toBe('2026-11-18T12:00:00.000Z');
  expect(taskDateParts('2026-11-18T12:00:00Z', 'America/Sao_Paulo')).toEqual({ date: '18/11/2026', time: '09:00' });
  expect(taskDateToInstant('08/03/2026', '02:30', 'America/New_York')).toBeNull();
  expect(taskDateToInstant('30/02/2026', '09:00', 'America/Sao_Paulo')).toBeNull();
});

it('recusa a hora repetida no fim do horário de verão sem escolher uma ocorrência', () => {
  expect(taskDateToInstant('07/11/2027', '01:30', 'America/New_York')).toBeNull();
  expect(taskDateToInstant('14/03/2027', '02:30', 'America/New_York')).toBeNull();
  expect(taskDateToInstant('07/11/2027', '03:30', 'America/New_York')).toBe('2027-11-07T08:30:00.000Z');
});

it('permite conservar Once encerrado na edição e recusa criar ou mudar para outro horário passado', () => {
  const now = Date.parse('2027-11-12T12:00:00Z');
  const original: ScheduledTaskInput = { ...input, schedule: { kind: 'once', at: '2027-11-01T09:00:37-03:00' } };
  const renamed = { ...original, name: 'nome-editado' };
  expect(validateScheduledTask(renamed, now, original)).toBeNull();
  expect(validateScheduledTask(renamed, now)).toBe('date');
  expect(validateScheduledTask({ ...renamed, schedule: { kind: 'once', at: '2027-11-02T09:00:37-03:00' } }, now, original)).toBe('date');
  expect(validateScheduledTask({ ...renamed, schedule: { kind: 'once', at: '2027-11-13T09:00:00-03:00' } }, now, original)).toBeNull();
});

it('mantém o mesmo objeto para a tarefa que não mudou e troca só a que mudou', () => {
  const base: ScheduledTask = { ...input, id: 'a', revision: 1, next_run_at: null, last_run_at: null, last_status: 'never', last_error: null, last_session: null };
  const prev = [base, { ...base, id: 'b' }];
  const next = reuseUnchangedTasks(prev, [{ ...base }, { ...base, id: 'b', last_status: 'sent' }, { ...base, id: 'c' }]);
  expect(next[0]).toBe(prev[0]);
  expect(next[1]).not.toBe(prev[1]);
  expect(next[1].last_status).toBe('sent');
  expect(next[2].id).toBe('c');
  expect(reuseUnchangedTasks(undefined, prev)).toEqual(prev);
});
