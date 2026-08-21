// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { buildSessionTabs, focusedTabKey, tabKeyOf } from './sessionTabs';
import type { Server } from './auth';
import type { ServerBucket } from './sessions';
import type { AggSession, State } from '@hangar/core';

function srv(id: string, label: string): Server {
  return { id, label, baseUrl: `http://${id}`, token: 't' };
}

// O bucket real já vem ENRIQUECIDO do aggregateSessions (serverId/severLabel/serverColor por row),
// então a fixture também carrega serverId — a chave serverId::name depende dele.
function sess(name: string, state: State = 'idle', serverId = 'a'): AggSession {
  return { name, state, serverId } as AggSession;
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

describe('focusedTabKey', () => {
  const tabs = () => buildSessionTabs([
    bucket(srv('a', 'A'), [sess('x', 'idle', 'a'), sess('y', 'idle', 'a')]),
    bucket(srv('b', 'B'), [sess('z', 'idle', 'b')]),
  ]).tabs;

  it('sem currentKey (Board/Canvas) a primeira aba é a focável', () => {
    expect(focusedTabKey(tabs(), null, null)).toBe('a::x');
  });

  it('currentKey válido é a focável inicial', () => {
    expect(focusedTabKey(tabs(), 'b::z', null)).toBe('b::z');
  });

  it('focusedKey mantém o foco mesmo com currentKey diferente', () => {
    expect(focusedTabKey(tabs(), 'b::z', 'a::y')).toBe('a::y');
  });

  it('focusedKey que saiu da lista cai para currentKey; currentKey inválido cai para a primeira', () => {
    expect(focusedTabKey(tabs(), 'b::z', 'a::sumiu')).toBe('b::z');
    expect(focusedTabKey(tabs(), 'a::sumiu', 'a::sumiu')).toBe('a::x');
  });

  it('lista vazia devolve null (nada focável)', () => {
    expect(focusedTabKey([], 'a::x', null)).toBeNull();
  });

  it('chave serverId::name distingue homônimas', () => {
    const tabs = buildSessionTabs([
      bucket(srv('a', 'A'), [sess('api', 'idle', 'a')]),
      bucket(srv('b', 'B'), [sess('api', 'idle', 'b')]),
    ]).tabs;
    expect(tabKeyOf(tabs[0].session)).not.toBe(tabKeyOf(tabs[1].session));
    expect(focusedTabKey(tabs, null, null)).toBe('a::api');
    expect(focusedTabKey(tabs, 'b::api', null)).toBe('b::api');
  });
});
