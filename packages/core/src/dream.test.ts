import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import * as m from './paraglide/messages';
import { dismissDreamSuggestion, dreamErrorText, dreamFactsText, dreamFailure, dreamFileChanges, dreamFirstMatch, dreamLineDiff, dreamMonthText,
  dreamUndoFailures, getDreamConfig, getDreamRun, getDreamStatus, parseDreamCap, putDreamConfig, putDreamProject, startDream, undoDreamRun,
  validDreamHour } from './dream';

const server = { id: 'test', label: 'Servidor', baseUrl: 'https://dream.test', token: 'owner' };
beforeEach(() => {
  _resetApiEnvForTests();
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized() {}, origin: null, createEventSource: vi.fn() });
});
afterEach(() => { vi.restoreAllMocks(); _resetApiEnvForTests(); });

it('chama cada rota no servidor escolhido com o token dele', async () => {
  const fetch = vi.spyOn(globalThis, 'fetch');
  const calls = [
    [() => getDreamConfig(server), 'GET', '/api/dream/config', undefined],
    [() => putDreamConfig(server, { month_cap_usd: null, machine: 'vps' }), 'PUT', '/api/dream/config', { month_cap_usd: null, machine: 'vps' }],
    [() => getDreamStatus(server), 'GET', '/api/dream/status', undefined],
    [() => startDream(server), 'POST', '/api/dream/run', {}],
    [() => startDream(server, 'git:x/y'), 'POST', '/api/dream/run', { project: 'git:x/y' }],
    [() => getDreamRun(server, 'r/1'), 'GET', '/api/dream/runs/r%2F1', undefined],
    [() => undoDreamRun(server, 'r/1'), 'POST', '/api/dream/runs/r%2F1/undo', undefined],
    [() => undoDreamRun(server, 'r/1', 'git:x/y'), 'POST', '/api/dream/runs/r%2F1/undo?project=git%3Ax%2Fy', undefined],
    [() => putDreamProject(server, 'dir:vps/a b', { alias_of: 'git:x' }), 'PUT', '/api/dream/project?id=dir%3Avps%2Fa%20b', { alias_of: 'git:x' }],
    [() => dismissDreamSuggestion(server, 2), 'POST', '/api/dream/suggestions/dismiss', { index: 2 }],
  ] as const;
  for (const [call, method, path, body] of calls) {
    fetch.mockResolvedValueOnce(new Response('{}'));
    await call();
    const [url, init] = fetch.mock.calls.at(-1)!;
    expect(url).toBe(`https://dream.test${path}`);
    expect(init?.method ?? 'GET').toBe(method);
    expect(init?.body ? JSON.parse(String(init.body)) : undefined).toEqual(body);
    expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer owner');
  }
});

it('traduz o código antes do primeiro dois-pontos e passa o resto como detalhe', () => {
  expect(dreamErrorText('dream_memory_changed: vps, notebook')).toBe(m.dream_error_dream_memory_changed({ detail: 'vps, notebook' }));
  expect(dreamErrorText('dream_running')).toBe(m.dream_error_dream_running());
  expect(dreamErrorText('dream_io: disco cheio')).toBe(m.dream_error_generic({ detail: 'dream_io: disco cheio' }));
  expect(dreamErrorText('sem código')).toBe(m.dream_error_generic({ detail: 'sem código' }));
});

it('traduz o erro lançado pela API pelo código e pelo campo inválido', async () => {
  vi.spyOn(globalThis, 'fetch')
    .mockResolvedValueOnce(new Response(JSON.stringify({ detail: { code: 'dream_not_dreamer', msg: 'dream_not_dreamer: esta máquina não é a que sonha', params: {} } }), { status: 409 }))
    .mockResolvedValueOnce(new Response(JSON.stringify({ detail: { code: 'dream_invalid_config', msg: 'valor inválido', params: { field: 'hour' } } }), { status: 422 }))
    .mockResolvedValueOnce(new Response(JSON.stringify({ detail: { code: 'dream_invalid_config', msg: 'valor inválido', params: { field: 'project_cap_usd' } } }), { status: 422 }));
  expect(dreamFailure(await startDream(server).catch((e) => e))).toBe(m.dream_error_dream_not_dreamer());
  expect(dreamFailure(await putDreamConfig(server, { hour: '25:00' }).catch((e) => e))).toBe(m.dream_invalid_hour());
  expect(dreamFailure(await putDreamConfig(server, { project_cap_usd: 0 }).catch((e) => e))).toBe(m.dream_invalid_cap());
  expect(dreamFailure(new Error('rede caiu'))).toBe(m.dream_error_generic({ detail: 'rede caiu' }));
});

it('classifica só os arquivos que mudaram, em ordem', () => {
  expect(dreamFileChanges({ 'a.md': '1', 'b.md': 'x', 'c.md': 'igual' }, { 'b.md': 'y', 'c.md': 'igual', 'd.md': 'novo' })).toEqual([
    { file: 'a.md', kind: 'removed' }, { file: 'b.md', kind: 'changed' }, { file: 'd.md', kind: 'added' },
  ]);
  expect(dreamFileChanges({}, {})).toEqual([]);
});

it('mostra o gasto da chave contra o teto quando há teto', () => {
  const month = { month: '2026-11', key_usd: 1.5, account_usd: 0.25 };
  expect(dreamMonthText(month, 20)).toBe(m.dream_month_value({ key: '1.50/20.00', account: '0.25' }));
  expect(dreamMonthText(month, null)).toBe(m.dream_month_value({ key: '1.50', account: '0.25' }));
});

it('valida horário e tetos antes de gravar', () => {
  expect(validDreamHour('03:30')).toBe(true);
  expect(validDreamHour('3:30')).toBe(false);
  expect(validDreamHour('24:00')).toBe(false);
  expect(parseDreamCap('')).toBeNull();
  expect(parseDreamCap('1,5')).toBe(1.5);
  expect(parseDreamCap('0')).toBeUndefined();
  expect(parseDreamCap('abc')).toBeUndefined();
});

it('mostra antes e depois linha a linha, mantendo as iguais', () => {
  expect(dreamLineDiff('a\nb\nc', 'a\nB\nc\nd')).toEqual([
    { kind: ' ', text: 'a' }, { kind: '-', text: 'b' }, { kind: '+', text: 'B' }, { kind: ' ', text: 'c' }, { kind: '+', text: 'd' },
  ]);
  expect(dreamLineDiff('', 'novo')).toEqual([{ kind: '+', text: 'novo' }]);
  expect(dreamLineDiff('velho', '')).toEqual([{ kind: '-', text: 'velho' }]);
});

it('deixa de fora a contagem de conversas desconhecida', () => {
  expect(dreamFactsText('vps', 3)).toBe(`vps · ${m.dream_new_sessions({ n: 3 })}`);
  expect(dreamFactsText('vps', null)).toBe('vps');
  expect(dreamFactsText('', 0)).toBe(m.dream_new_sessions({ n: 0 }));
});

it('diz o motivo de cada lugar que o desfazer não restaurou', () => {
  const label = (machine: string) => machine.toUpperCase();
  expect(dreamUndoFailures([
    { project: 'git:x', machine: 'vps', root: '/a', ok: true, error: null },
    { project: 'git:x', machine: 'nb', root: '/b', ok: false, error: 'dream_memory_changed: nb' },
    { project: 'user-memory', machine: 'vps', root: '/c', ok: false, error: null },
  ], label)).toEqual([`NB: ${m.dream_error_dream_memory_changed({ detail: 'nb' })}`, m.dream_undo_failed({ machine: 'VPS' })]);
});

it('acha a primeira leitura que casa sem esperar as lentas', async () => {
  const never = new Promise<number>(() => {});
  expect(await dreamFirstMatch([never, Promise.reject(new Error('fora')), Promise.resolve(2)], (n) => n === 2)).toBe(2);
  expect(await dreamFirstMatch([Promise.resolve(1), Promise.reject(new Error('fora'))], (n) => n === 2)).toBeNull();
  expect(await dreamFirstMatch<number>([], () => true)).toBeNull();
});
