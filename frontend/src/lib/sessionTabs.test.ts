// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { buildSessionTabs } from './sessionTabs';
import type { Server } from './auth';
import type { ServerBucket } from './sessions';
import type { AggSession, State } from './types';

function srv(id: string, label: string): Server {
  return { id, label, baseUrl: `http://${id}`, token: 't' };
}

function sess(name: string, state: State = 'idle'): AggSession {
  return { name, state } as AggSession;
}

function bucket(server: Server, sessions: AggSession[], error: string | null = null, loaded = true): ServerBucket {
  return { server, sessions, error, loaded };
}

describe('buildSessionTabs', () => {
  it('preserva a ordem dos buckets e usa a ordenação compartilhada', () => {
    const model = buildSessionTabs([
      bucket(srv('a', 'A'), [sess('zeta'), sess('alpha', 'awaiting_input'), sess('beta')]),
      bucket(srv('b', 'B'), [sess('gamma')]),
    ]);
    // sortSessions: aguardando resposta primeiro, depois alfabético.
    expect(model.tabs.map((t) => t.session.name)).toEqual(['alpha', 'beta', 'zeta', 'gamma']);
  });

  it('marca boundary só no primeiro item do próximo servidor', () => {
    const model = buildSessionTabs([
      bucket(srv('a', 'A'), [sess('x'), sess('y')]),
      bucket(srv('b', 'B'), [sess('w')]),
    ]);
    expect(model.tabs.map((t) => t.boundary)).toEqual([false, false, true]);
  });

  it('bucket offline não gera aba e entra no indicador offline', () => {
    const model = buildSessionTabs([
      bucket(srv('a', 'A'), [sess('x')]),
      bucket(srv('off', 'Servidor fora'), [], 'offline', false),
      bucket(srv('c', 'C'), [sess('y')]),
    ]);
    expect(model.tabs.map((t) => t.session.name)).toEqual(['x', 'y']);
    // O primeiro item do próximo servidor PRESENTE ainda é boundary — o offline não conta.
    expect(model.tabs[1].boundary).toBe(true);
    expect(model.offlineLabels).toEqual(['Servidor fora']);
  });

  it('buckets vazios não geram boundary fantasma', () => {
    const model = buildSessionTabs([
      bucket(srv('a', 'A'), [sess('x')]),
      bucket(srv('b', 'B'), []),
      bucket(srv('c', 'C'), [sess('y')]),
    ]);
    expect(model.tabs.map((t) => t.session.name)).toEqual(['x', 'y']);
    expect(model.tabs[1].boundary).toBe(true);
    expect(model.offlineLabels).toEqual([]);
  });
});
