import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from './apiEnv';
import { _resetDiagForTests, recentFailures, recordFailure } from './diag';
import { _limparEsfriamentoParaTestes } from './esfriamento';
import { getConfigForServer, recentFailuresFor, rotaGenerica, sendDiagReport, sendDiagReportAll, steerQueuedMessage } from './api';
import type { Server } from './servers';

const serverA: Server = { id: 'a', label: 'Servidor A', baseUrl: 'https://a.test', token: 'token-a' };
const serverB: Server = { id: 'b', label: 'Servidor B', baseUrl: 'https://b.test', token: 'token-b' };

const erro = (status: number, code: string) =>
  new Response(JSON.stringify({ detail: { code, message: 'falhou' } }), { status });

beforeEach(() => {
  vi.restoreAllMocks();
  _limparEsfriamentoParaTestes();
  _resetDiagForTests();
  configureApi({
    getBaseUrl: () => 'https://a.test',
    getToken: () => 'token-a',
    onUnauthorized: () => {},
    origin: 'https://app.test',
    createEventSource: () => ({ addEventListener() {}, removeEventListener() {}, close() {}, onerror: null, onopen: null, readyState: 0 }),
  });
});
afterEach(_resetDiagForTests);

describe('rotaGenerica — id da fila', () => {
  it('troca o id da entrada da fila por *', () => {
    expect(rotaGenerica('/api/sessions/minha-sessao/queue/q-123/steer')).toBe('/api/sessions/*/queue/*/steer');
    expect(rotaGenerica('/api/sessions/minha-sessao/queue/q-123?only_waiting=true')).toBe('/api/sessions/*/queue/*');
  });

  it('mantém a rota fixa de reordenar', () => {
    expect(rotaGenerica('/api/sessions/minha-sessao/queue/reorder')).toBe('/api/sessions/*/queue/reorder');
  });
});

describe('fila de falhas recentes', () => {
  it('guarda até 50 por servidor, mais recente primeiro', () => {
    for (let i = 1; i <= 55; i++) recordFailure('https://a.test', { ts: i, route: 'GET /api/config', status: 500, code: null });
    const lista = recentFailures('https://a.test');
    expect(lista).toHaveLength(50);
    expect(lista[0].ts).toBe(55);
    expect(lista[49].ts).toBe(6);
  });

  it('código fora do formato vira null', () => {
    recordFailure('https://a.test', { ts: 1, route: 'GET /api/config', status: 500, code: 'Texto Livre' });
    expect(recentFailures('https://a.test')[0].code).toBeNull();
  });

  it('registra só falha, com rota genérica, status e código', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, steered: true })))
      .mockResolvedValueOnce(erro(503, 'runtime_unhealthy'));
    await steerQueuedMessage('sessao-x', 'entrada-1', serverA);
    await expect(steerQueuedMessage('sessao-x', 'entrada-2', serverA)).rejects.toThrow();
    const lista = recentFailuresFor(serverA);
    expect(lista).toHaveLength(1);
    expect(lista[0]).toMatchObject({ route: 'POST /api/sessions/*/queue/*/steer', status: 503, code: 'runtime_unhealthy' });
    expect(lista[0].ts).toBeGreaterThan(1_700_000_000);
    expect(lista[0].ts).toBeLessThan(10_000_000_000);
  });

  it('falha de rede entra com status 0 e sem código', async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new TypeError('Failed to fetch'));
    await expect(steerQueuedMessage('s', 'e', serverA)).rejects.toThrow();
    expect(recentFailuresFor(serverA)[0]).toMatchObject({ route: 'POST /api/sessions/*/queue/*/steer', status: 0, code: null });
  });

  it('cada servidor tem a sua fila', async () => {
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(erro(500, 'falha_a'))
      .mockResolvedValueOnce(erro(502, 'falha_b'));
    await expect(getConfigForServer(serverA)).rejects.toThrow();
    await expect(getConfigForServer(serverB)).rejects.toThrow();
    expect(recentFailuresFor(serverA).map((f) => f.code)).toEqual(['falha_a']);
    expect(recentFailuresFor(serverB).map((f) => f.code)).toEqual(['falha_b']);
  });

  it('servidor de convite não entra', async () => {
    const convite: Server = { ...serverA, id: 'c', baseUrl: 'https://c.test', invite: true };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(erro(503, 'erro_sessao_indisponivel'));
    await expect(steerQueuedMessage('s', 'e', convite)).rejects.toThrow();
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(recentFailuresFor(convite)).toEqual([]);
  });

  it('junta os endereços do mesmo servidor (rede local e Tailscale)', () => {
    const comLan: Server = { ...serverA, lan: { url: 'http://192.168.0.9:8765', id: 'a' } };
    recordFailure('https://a.test', { ts: 1, route: 'GET /api/config', status: 500, code: null });
    recordFailure('http://192.168.0.9:8765', { ts: 2, route: 'GET /api/sessions', status: 0, code: null });
    expect(recentFailuresFor(comLan).map((f) => f.ts)).toEqual([2, 1]);
  });
});

describe('sendDiagReport', () => {
  it('envia só as falhas daquele servidor e devolve o código', async () => {
    recordFailure('https://a.test', { ts: 2, route: 'GET /api/config', status: 500, code: 'x' });
    recordFailure('https://b.test', { ts: 3, route: 'GET /api/config', status: 500, code: 'y' });
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValue(new Response(JSON.stringify({ code: 'abcd2345', diary: 'sent' }), { status: 201 }));
    const res = await sendDiagReport(serverA, { text: 'travou a fila', client: 'pwa 1.0' });
    expect(res).toEqual({ ok: true, code: 'abcd2345', diary: 'sent' });
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe('https://a.test/api/diag/report');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(String(init?.body))).toEqual({
      text: 'travou a fila', client: 'pwa 1.0',
      failures: [{ ts: 2, route: 'GET /api/config', status: 500, code: 'x' }],
    });
  });

  it('texto vazio não vai e diário com falha volta como failed', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValue(new Response(JSON.stringify({ code: 'abcd2345', diary: 'failed' }), { status: 201 }));
    expect(await sendDiagReport(serverA, { text: '  ', client: 'pwa 1.0' }))
      .toEqual({ ok: true, code: 'abcd2345', diary: 'failed' });
    expect(JSON.parse(String(fetchMock.mock.calls[0][1]?.body))).not.toHaveProperty('text');
  });

  it.each([
    [erro(429, 'diag_report_rate_limited'), { ok: false, code: 'diag_report_rate_limited', status: 429, kind: 'rate_limited' }],
    [erro(502, 'diag_report_unreachable'), { ok: false, code: 'diag_report_unreachable', status: 502, kind: 'unreachable' }],
    [erro(403, 'diag_report_forbidden'), { ok: false, code: 'diag_report_forbidden', status: 403, kind: 'failed' }],
    [new Response('{"detail":"Not Found"}', { status: 404 }), { ok: false, code: 'unsupported', status: 404, kind: 'unsupported' }],
    [new Response('bad gateway', { status: 502 }), { ok: false, code: 'unexpected', status: 502, kind: 'failed' }],
  ])('mapeia a resposta de erro sem lançar (%#)', async (resposta, esperado) => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(resposta);
    expect(await sendDiagReport(serverA, { client: 'pwa 1.0' })).toEqual(esperado);
  });

  it('sem resposta vira network', async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new TypeError('Failed to fetch'));
    expect(await sendDiagReport(serverA, { client: 'pwa 1.0' })).toEqual({ ok: false, code: 'network', status: 0, kind: 'unreachable' });
  });
});

describe('sendDiagReportAll', () => {
  const serverC: Server = { id: 'c', label: 'Servidor C', baseUrl: 'https://c.test', token: 'token-c' };
  const ok = (code: string) => new Response(JSON.stringify({ code, diary: 'sent' }), { status: 201 });
  const bodies = (fetchMock: { mock: { calls: unknown[][] } }) =>
    fetchMock.mock.calls.map(([url, init]) => [String(url), JSON.parse(String((init as RequestInit).body))] as const);

  it('uma máquina por vez: as seguintes levam o código da primeira como grupo', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(ok('aaaa2222')).mockResolvedValueOnce(ok('bbbb3333')).mockResolvedValueOnce(ok('cccc4444'));
    const res = await sendDiagReportAll([serverA, serverB, serverC], { text: 'na VM', client: 'pwa 1.0' });
    expect(res.code).toBe('aaaa2222');
    expect(res.results.map((r) => [r.server.id, r.result.ok])).toEqual([['a', true], ['b', true], ['c', true]]);
    const sent = bodies(fetchMock);
    expect(sent.map(([url]) => url)).toEqual(['https://a.test/api/diag/report', 'https://b.test/api/diag/report', 'https://c.test/api/diag/report']);
    expect(sent[0][1]).not.toHaveProperty('group');
    expect(sent[1][1].group).toBe('aaaa2222');
    expect(sent[2][1].group).toBe('aaaa2222');
    expect(sent[1][1].text).toBe('na VM');
  });

  it('a primeira que falha não vira grupo: o código é o da primeira que deu certo', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockRejectedValueOnce(new TypeError('Failed to fetch')).mockResolvedValueOnce(ok('bbbb3333')).mockResolvedValueOnce(ok('cccc4444'));
    const res = await sendDiagReportAll([serverA, serverB, serverC], { client: 'pwa 1.0' });
    expect(res.code).toBe('bbbb3333');
    expect(res.results[0].result).toEqual({ ok: false, code: 'network', status: 0, kind: 'unreachable' });
    const sent = bodies(fetchMock);
    expect(sent[1][1]).not.toHaveProperty('group');
    expect(sent[2][1].group).toBe('bbbb3333');
  });

  it('nenhuma deu certo: sem código, com o motivo de cada uma', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(erro(429, 'diag_report_rate_limited')).mockResolvedValueOnce(erro(502, 'diag_report_unreachable'));
    const res = await sendDiagReportAll([serverA, serverB], { client: 'pwa 1.0' });
    expect(res.code).toBeNull();
    expect(res.results.map((r) => r.result.ok ? null : r.result.kind)).toEqual(['rate_limited', 'unreachable']);
  });
});
