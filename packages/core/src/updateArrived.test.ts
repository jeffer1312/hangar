import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getArrivedNews, markArrivedSeen } from './api';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import type { Server } from './servers';

const server: Server = { id: 'casa', label: 'casa', baseUrl: 'https://casa.test', token: 'token-casa' };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), {
  status, headers: { 'Content-Type': 'application/json' },
});
const item = { id: '2026-11-09-plugins-do-claude', title: 'Plugins', kind: 'new', text: 't',
  platforms: 'all', try_it: null, animation: 'plugins-do-claude' };

describe('novidades que chegaram pela atualização', () => {
  const fetchMock = vi.fn<typeof fetch>();
  beforeEach(() => {
    fetchMock.mockReset();
    configureApi({ getBaseUrl: () => 'https://active.test', getToken: () => 'active-token',
      origin: null, onUnauthorized: () => { throw new Error('unexpected-logout'); }, createEventSource: vi.fn() });
  });
  afterEach(() => { vi.restoreAllMocks(); _resetApiEnvForTests(); });

  it('GET leva o aparelho codificado na query e o token do servidor', async () => {
    fetchMock.mockResolvedValue(json({ news: [item], through: 7 }));
    expect(await getArrivedNews(server, 'a b/c', fetchMock)).toEqual({ news: [item], through: 7 });
    const [url, init] = fetchMock.mock.calls[0];
    expect(String(url)).toBe('https://casa.test/api/update/arrived?device=a%20b%2Fc');
    expect((init?.method ?? 'GET').toUpperCase()).toBe('GET');
    expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer token-casa');
  });

  it('POST seen manda aparelho e through em JSON', async () => {
    fetchMock.mockResolvedValue(json({ through: 7 }));
    expect(await markArrivedSeen(server, 'dev-1', 9, fetchMock)).toEqual({ through: 7 });
    const [url, init] = fetchMock.mock.calls[0];
    expect(String(url)).toBe('https://casa.test/api/update/arrived/seen');
    expect(init?.method).toBe('POST');
    expect(new Headers(init?.headers).get('Content-Type')).toBe('application/json');
    expect(JSON.parse(String(init?.body))).toEqual({ device: 'dev-1', through: 9 });
  });

  it.each([404, 405])('servidor sem a rota (%i) devolve null', async (status) => {
    fetchMock.mockResolvedValue(json({ detail: 'Not Found' }, status));
    expect(await getArrivedNews(server, 'dev-1', fetchMock)).toBeNull();
  });

  it('erro do servidor chega com status e código do envelope', async () => {
    fetchMock.mockResolvedValue(json({ detail: { code: 'update_arrived_owner_only' } }, 403));
    await expect(getArrivedNews(server, 'dev-1', fetchMock)).rejects.toMatchObject({ status: 403, code: 'update_arrived_owner_only' });
  });
});
