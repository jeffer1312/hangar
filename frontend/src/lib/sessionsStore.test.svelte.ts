// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { sessionsStore } from './sessionsStore.svelte';

const streams = vi.hoisted(() => new Map<string, Map<string, (event: { data: string }) => void>>());
vi.mock('./auth', () => ({
  listServers: () => ['lan', 'vpn'].map(id => ({ id, label: id, baseUrl: `http://${id}`, token: 'test' })),
  onServersChanged: () => () => {},
}));
vi.mock('./navPelaLista', () => ({ navPelaLista: vi.fn() }));
vi.mock('./navegadorPanel.svelte', () => ({ podarNavMortos: vi.fn() }));
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  openSessionsStream: (server: { id: string }) => {
    const handlers = new Map();
    streams.set(server.id, handlers);
    return { close: vi.fn(), addEventListener: (name: string, fn: unknown) => handlers.set(name, fn) };
  },
}));
afterEach(() => { sessionsStore.release(); streams.clear(); });

it('preserva a sessão da VPN quando a LAN assume a duplicata na lista visual', () => {
  sessionsStore.retain();
  const session = { name: 'hangar-6', jsonl: '/same.jsonl', provider: 'codex', tracked: true, state: 'working' };
  const publish = (id: string) => streams.get(id)!.get('sessions')!({ data: JSON.stringify([session]) });
  publish('vpn');
  expect(sessionsStore.sessionsForServer('vpn')).toEqual([session]);
  publish('lan');
  expect(sessionsStore.rows.map(s => s.serverId)).toEqual(['lan']);
  expect(sessionsStore.sessionsForServer('vpn')).toEqual([session]);
});
