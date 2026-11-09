import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  addMarketplaceForServer, checkPluginsForServer, getAvailablePluginsForServer, getPluginDetailsForServer,
  getPluginsForServer, liveClaudeSessions, pluginActionForServer, removeMarketplaceForServer,
  setMarketplaceAutoUpdateForServer, updateMarketplaceForServer,
} from './plugins';
import type { SessionInfo } from './types';

const server = { id: 'a', label: 'Servidor A', baseUrl: 'https://a.test', token: 'token-a' };
const STATE = { plugins: [], marketplaces: [], checked_at: null };

afterEach(() => vi.restoreAllMocks());

describe('chamadas de plugins', () => {
  const calls: Array<[string, () => Promise<unknown>, string, string, unknown, number]> = [
    ['lista', () => getPluginsForServer(server), 'GET', '/api/plugins', undefined, 30_000],
    ['check', () => checkPluginsForServer(server), 'POST', '/api/plugins/check', undefined, 210_000],
    ['disponíveis', () => getAvailablePluginsForServer(server), 'GET', '/api/plugins/available', undefined, 70_000],
    ['detalhe', () => getPluginDetailsForServer(server, 'a b@c/d'), 'GET', '/api/plugins/a%20b%40c%2Fd/details', undefined, 30_000],
    ['instalar', () => pluginActionForServer(server, 'caveman@caveman', 'install'), 'POST', '/api/plugins/caveman%40caveman/install', undefined, 210_000],
    ['atualizar', () => pluginActionForServer(server, 'caveman@caveman', 'update'), 'POST', '/api/plugins/caveman%40caveman/update', undefined, 210_000],
    ['ligar', () => pluginActionForServer(server, 'caveman@caveman', 'enable'), 'POST', '/api/plugins/caveman%40caveman/enable', undefined, 50_000],
    ['desligar', () => pluginActionForServer(server, 'caveman@caveman', 'disable'), 'POST', '/api/plugins/caveman%40caveman/disable', undefined, 50_000],
    ['remover', () => pluginActionForServer(server, 'caveman@caveman', 'uninstall'), 'DELETE', '/api/plugins/caveman%40caveman', undefined, 50_000],
    ['marketplace novo', () => addMarketplaceForServer(server, 'JuliusBrussee/caveman'), 'POST', '/api/plugins/marketplaces', { source: 'JuliusBrussee/caveman' }, 210_000],
    ['marketplace atualizar', () => updateMarketplaceForServer(server, 'caveman'), 'POST', '/api/plugins/marketplaces/caveman/update', undefined, 210_000],
    ['marketplace remover', () => removeMarketplaceForServer(server, 'caveman'), 'DELETE', '/api/plugins/marketplaces/caveman', undefined, 70_000],
    ['atualizar sozinho', () => setMarketplaceAutoUpdateForServer(server, 'caveman', false), 'PUT', '/api/plugins/marketplaces/caveman/auto-update', { enabled: false }, 50_000],
  ];

  it.each(calls)('%s: método, rota, corpo e prazo do pior caso do servidor + 10 s', async (_, call, method, path, body, prazo) => {
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify(STATE)));
    await expect(call()).resolves.toEqual(STATE);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe(`https://a.test${path}`);
    expect(init?.method ?? 'GET').toBe(method);
    expect(init?.body === undefined ? undefined : JSON.parse(String(init.body))).toEqual(body);
    expect(timeout).toHaveBeenCalledWith(prazo);
  });

  it('erro do servidor chega com o código do contrato', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(
      JSON.stringify({ detail: { code: 'plugins_busy', params: {}, msg: 'plugins_busy' } }), { status: 409 }));
    await expect(pluginActionForServer(server, 'caveman@caveman', 'enable')).rejects.toMatchObject({ status: 409, code: 'plugins_busy' });
  });
});

describe('sessões que ainda estão com o estado anterior', () => {
  const s = (name: string, extra: Partial<SessionInfo>): SessionInfo => ({ name, state: 'idle', ...extra });

  it('conta só Claude vivo e acordado; sem provider vale Claude', () => {
    const sessions = [
      s('sem-provider', {}),
      s('claude', { provider: 'claude', state: 'working' }),
      s('morta', { provider: 'claude', state: 'dead' }),
      s('dormindo', { provider: 'claude', asleep: true }),
      s('codex', { provider: 'codex' }),
      s('pi', { provider: 'pi' }),
    ];
    expect(liveClaudeSessions(sessions).map((x) => x.name)).toEqual(['sem-provider', 'claude']);
  });
});
