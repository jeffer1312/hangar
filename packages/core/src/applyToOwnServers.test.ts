import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import { applyCodexContext, applySessionDefault, applyToOwnServers } from './applyToOwnServers';
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

  const value = { model: 'opus', effort: 'high', permission: '' };
  const calls = (method: string) => fetchMock.mock.calls.filter(([, init]) => (init?.method ?? 'GET') === method)
    .map(([url, init]) => [String(url), init?.body ? JSON.parse(init.body as string) : null]);

  it('padrão do Claude sem motor grava no servidor e espelha no settings.json', async () => {
    fetchMock.mockImplementation(async (input) => {
      const url = new URL(String(input instanceof Request ? input.url : input));
      return new Response(JSON.stringify(url.pathname === '/api/session-defaults' ? { configured: true, defaults: value } : { ok: true, written: [] }), { status: 200 });
    });
    await applySessionDefault(server('ok'), { provider: 'claude', value, mirror: true });
    expect(calls('PUT')).toEqual([['https://ok.test/api/session-defaults?provider=claude', { defaults: value, if_absent: false }]]);
    expect(calls('POST')).toEqual([['https://ok.test/api/claude/defaults', value]]);
  });

  it('desmarcar apaga sem espelhar; motor ausente na máquina não grava', async () => {
    fetchMock.mockImplementation(async (input) => {
      const url = new URL(String(input instanceof Request ? input.url : input));
      if (url.pathname === '/api/engines') return new Response(JSON.stringify({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '' }), { status: 200 });
      return new Response(JSON.stringify({ configured: false, defaults: null }), { status: 200 });
    });
    await applySessionDefault(server('ok'), { provider: 'claude', value: null, mirror: true });
    expect(calls('PUT')).toEqual([['https://ok.test/api/session-defaults?provider=claude', { defaults: null, if_absent: false }]]);
    expect(calls('POST')).toEqual([]);
    fetchMock.mockClear();
    expect(await applySessionDefault(server('ok'), { provider: 'claude', engine: 'proxy', value, mirror: false })).toBe('absent');
    expect(calls('PUT')).toEqual([]);
  });

  it('motor com conta grava no escopo da conta só se ela existir lá', async () => {
    const account = { account: 'pro', credential_id: 'codex:/pro', email: 'pro@example.test', label: 'pro' };
    fetchMock.mockImplementation(async (input) => {
      const url = new URL(String(input instanceof Request ? input.url : input));
      if (url.pathname === '/api/engines') return new Response(JSON.stringify({ motores: { proxy: { label: 'Proxy', model: 'sol',
        cliproxy_accounts: [account], cliproxy_binding: 'b1', cliproxy_bound_account: account } } }), { status: 200 });
      return new Response(JSON.stringify({ configured: true, defaults: value }), { status: 200 });
    });
    expect(await applySessionDefault(server('ok'), { provider: 'claude', engine: 'proxy', engineAccount: 'plus', value, mirror: false })).toBe('absent');
    expect(calls('PUT')).toEqual([]);
    await applySessionDefault(server('ok'), { provider: 'claude', engine: 'proxy', engineAccount: 'pro', value, mirror: false });
    expect(calls('PUT')).toEqual([['https://ok.test/api/session-defaults?provider=claude&engine=proxy&engine_account=pro', { defaults: value, if_absent: false }]]);
    expect(calls('POST')).toEqual([]);
  });
});
