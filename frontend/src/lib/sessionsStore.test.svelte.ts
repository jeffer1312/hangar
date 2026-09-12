// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { sessionsStore } from './sessionsStore.svelte';
import { configureDiag } from '@hangar/core';

const streams = vi.hoisted(() => new Map<string, Map<string, (event: { data: string }) => void>>());
const connectionIds = vi.hoisted(() => new Map<string, string | undefined>());
vi.mock('./auth', () => ({
  listServers: () => ['lan', 'vpn'].map(id => ({ id, label: id, baseUrl: `http://${id}`, token: 'test' })),
  onServersChanged: () => () => {},
}));
vi.mock('./navPelaLista', () => ({ navPelaLista: vi.fn() }));
vi.mock('./navegadorPanel.svelte', () => ({ podarNavMortos: vi.fn() }));
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  openSessionsStream: (server: { id: string }, req?: string) => {
    connectionIds.set(server.id, req);
    const handlers = new Map();
    streams.set(server.id, handlers);
    return { close: vi.fn(), addEventListener: (name: string, fn: unknown) => handlers.set(name, fn) };
  },
}));
afterEach(() => { sessionsStore.release(); streams.clear(); connectionIds.clear(); vi.clearAllTimers(); vi.useRealTimers();
  configureDiag({ registrar: () => {}, novoReq: () => '' }); });

it('passa à abertura da lista o mesmo ID registrado em cada servidor', () => {
  vi.useFakeTimers();
  const registrar = vi.fn();
  let sequence = 0;
  configureDiag({ registrar, novoReq: () => `lista-${++sequence}` });
  sessionsStore.retain();
  expect(new Set(connectionIds.values()).size).toBe(2);
  for (const [server, req] of connectionIds) {
    expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'lista.abrir', req }), `http://${server}`);
  }
});

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

it('registra primeiro quadro, parse inválido, silêncio e volta sem copiar quadros ou URLs', async () => {
  vi.useFakeTimers();
  const registrar = vi.fn();
  configureDiag({ registrar, novoReq: () => 'conexao' });
  sessionsStore.retain();
  const publicar = (data: string) => streams.get('lan')!.get('sessions')!({ data });
  publicar('[]');
  publicar('[]');
  expect(registrar.mock.calls.filter(([e, destino]) => e.evento === 'lista.conectou' && destino === 'http://lan')).toHaveLength(1);
  publicar('segredo conversa token');
  publicar('[]');
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ codigo: 'json_invalido' }), 'http://lan');
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'lista.voltou' }), 'http://lan');
  await vi.advanceTimersByTimeAsync(25000);
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ codigo: 'silencio', espera_ms: 25000 }), 'http://lan');
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'lista.retentativa', espera_ms: 5000 }), 'http://lan');
  expect(JSON.stringify(registrar.mock.calls.map(([e]) => e))).not.toMatch(/segredo|conversa|token|http:/);
});
