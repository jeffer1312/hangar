import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getManual, getNewsHistory } from './api';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import type { Server } from './servers';

const server: Server = { id: 'casa', label: 'casa', baseUrl: 'https://casa.test', token: 'token-casa' };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), {
  status, headers: { 'Content-Type': 'application/json' },
});
const news = { news: [{ id: '2026-11-12-scheduled-tasks', date: '2026-11-12', title: 'Tarefas', kind: 'new',
  platforms: 'all', summary: 's', text: 's\n\nmais', more: true, try_it: null, animation: null, manual: 'agendar' }] };
const manual = { categories: [{ id: 'comecar', articles: [{ id: 'agendar', title: 'Agendar', summary: 's', icon: 'clock',
  platforms: 'all', intro: 'i', sections: [{ id: 'a', title: 'A', markdown: 'm' }],
  news: [{ id: '2026-11-12-scheduled-tasks', date: '2026-11-12', title: 'Tarefas', platforms: 'all' }] }] }] };

describe.each([
  ['getNewsHistory', getNewsHistory, '/api/news', news],
  ['getManual', getManual, '/api/manual', manual],
] as const)('%s', (_name, get, path, body) => {
  const fetchMock = vi.fn<typeof fetch>();
  beforeEach(() => {
    fetchMock.mockReset();
    configureApi({ getBaseUrl: () => 'https://active.test', getToken: () => 'active-token',
      origin: null, onUnauthorized: () => { throw new Error('unexpected-logout'); }, createEventSource: vi.fn() });
  });
  afterEach(() => { vi.restoreAllMocks(); _resetApiEnvForTests(); });

  it('GET no servidor pedido devolve os dados', async () => {
    fetchMock.mockResolvedValue(json(body));
    expect(await get(server, fetchMock)).toEqual(body);
    const [url, init] = fetchMock.mock.calls[0];
    expect(String(url)).toBe(`https://casa.test${path}`);
    expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer token-casa');
  });

  it.each([404, 405])('servidor sem a rota (%i) devolve null', async (status) => {
    fetchMock.mockResolvedValue(json({ detail: 'Not Found' }, status));
    expect(await get(server, fetchMock)).toBeNull();
  });

  it('503 chega com status e código', async () => {
    const code = path === '/api/news' ? 'news_unavailable' : 'manual_unavailable';
    fetchMock.mockResolvedValue(json({ detail: { code } }, 503));
    await expect(get(server, fetchMock)).rejects.toMatchObject({ status: 503, code });
  });
});
