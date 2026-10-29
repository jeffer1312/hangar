import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import { applyCodexContext, applyToOwnServers } from './applyToOwnServers';
import { _limparEsfriamentoParaTestes } from './esfriamento';
import type { Server } from './servers';

const server = (id: string, extra: Partial<Server> = {}): Server =>
  ({ id, label: id, baseUrl: `https://${id}.test`, token: `token-${id}`, ...extra });

describe('applyToOwnServers', () => {
  const fetchMock = vi.fn<typeof fetch>();
  beforeEach(() => {
    _limparEsfriamentoParaTestes();
    fetchMock.mockReset();
    vi.stubGlobal('fetch', fetchMock);
    configureApi({ getBaseUrl: () => 'https://active.test', getToken: () => 'active-token',
      origin: null, onUnauthorized: () => { throw new Error('unexpected-logout'); }, createEventSource: vi.fn() });
  });
  afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); _resetApiEnvForTests(); });

  it('uma fora do ar, uma antiga, uma aplicada, uma sem o Codex; convite e desligada ficam de fora', async () => {
    fetchMock.mockImplementation(async (input) => {
      const url = new URL(String(input instanceof Request ? input.url : input));
      if (url.hostname === 'down.test') throw new TypeError('fetch failed');
      if (url.hostname === 'old.test') return new Response('{"detail":"Not Found"}', { status: 404 });
      if (url.hostname === 'bad.test') return new Response(JSON.stringify({ detail: { code: 'harness_invalid' } }), { status: 400 });
      if (url.pathname === '/api/harness') {
        return new Response(JSON.stringify([{ id: 'codex', instalado: url.hostname !== 'none.test' }]), { status: 200 });
      }
      return new Response(JSON.stringify({ contexto_estendido: true }), { status: 200 });
    });
    const servers = [server('down'), server('old'), server('ok'), server('none'), server('bad'),
      server('guest', { invite: true }), server('off', { disabled: true })];
    const results = await applyToOwnServers(servers, (s) => applyCodexContext(s, true));
    expect(results.map(({ serverId, status }) => [serverId, status])).toEqual([
      ['down', 'offline'], ['old', 'old'], ['ok', 'ok'], ['none', 'absent'], ['bad', 'failed'],
    ]);
    expect(results[0].error).toBeTruthy();
    expect(results[2].error).toBeUndefined();
    const post = fetchMock.mock.calls.find(([url, init]) => String(url).startsWith('https://ok.test') && init?.method === 'POST')!;
    expect(String(post[0])).toBe('https://ok.test/api/harness/codex/opcoes');
    expect(JSON.parse(post[1]?.body as string)).toEqual({ contexto_estendido: true });
    expect(fetchMock.mock.calls.some(([url, init]) => String(url).startsWith('https://none.test') && init?.method === 'POST')).toBe(false);
  });
});
