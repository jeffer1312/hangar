import { describe, expect, it } from 'vitest';
import type { Slot } from './sessions';
import type { Server } from './auth';
import type { SessionInfo } from './types';

// sessions.ts importa auth.ts, que toca localStorage no load via migrate(); vitest env=node nao tem
// localStorage. Stub minimo ANTES do import dinamico — mesmo padrao do auth.test.ts (import estatico
// de aggregateSessions rodaria migrate() antes do stub).
const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
const { aggregateSessions, sweepHidden } = await import('./sessions');

const srv = (id: string): Server => ({ id, label: `srv-${id}`, baseUrl: `http://${id}`, token: 't' });
const sess = (name: string, extra: Partial<SessionInfo> = {}): SessionInfo =>
  ({ name, state: 'idle', jsonl: `/j/${name}.jsonl`, ...extra });
const slots = (m: Record<string, Slot>) => new Map(Object.entries(m));

describe('aggregateSessions', () => {
  it('achata na ordem de servers e enriquece com serverId/label/color', () => {
    const a = aggregateSessions([srv('a'), srv('b')], slots({
      a: { sessions: [sess('x')], error: null },
      b: { sessions: [sess('y')], error: null },
    }));
    expect(a.rows.map((r) => [r.name, r.serverId])).toEqual([['x', 'a'], ['y', 'b']]);
    expect(a.rows[0].serverLabel).toBe('srv-a');
    expect(a.rows[0].serverColor).toMatch(/^#/);
  });

  it('dedup global por jsonl::name — o primeiro servidor na ordem vence', () => {
    const dup = sess('x'); // mesmo jsonl + nome nos dois servidores (backend atrás de 2 URLs)
    const a = aggregateSessions([srv('a'), srv('b')], slots({
      a: { sessions: [dup], error: null },
      b: { sessions: [dup, sess('y')], error: null },
    }));
    expect(a.rows.map((r) => `${r.serverId}:${r.name}`)).toEqual(['a:x', 'b:y']);
    expect(a.byServer[1].sessions.map((s) => s.name)).toEqual(['y']);
  });

  it('sem jsonl cai pro cwd e por fim pro nome (chave `${jsonl ?? cwd ?? ""}::${name}`)', () => {
    const a = aggregateSessions([srv('a'), srv('b')], slots({
      a: { sessions: [sess('x', { jsonl: null, cwd: '/p' })], error: null },
      b: { sessions: [sess('x', { jsonl: null, cwd: '/p' })], error: null },
    }));
    expect(a.rows).toHaveLength(1);
  });

  it('servidor offline sem lista: bucket presente, error, loaded=false, sem linhas', () => {
    const a = aggregateSessions([srv('a')], slots({ a: { sessions: null, error: 'offline' } }));
    expect(a.byServer[0]).toMatchObject({ error: 'offline', loaded: false, sessions: [] });
    expect(a.rows).toEqual([]);
  });

  it('servidor offline com lista STALE: mantém as linhas E o error (consumidor decide)', () => {
    const a = aggregateSessions([srv('a')], slots({ a: { sessions: [sess('x')], error: 'offline' } }));
    expect(a.byServer[0]).toMatchObject({ error: 'offline', loaded: true });
    expect(a.rows.map((r) => r.name)).toEqual(['x']);
  });

  it('loading: true só enquanto NENHUM servidor emitiu evento nem erro', () => {
    expect(aggregateSessions([srv('a')], new Map()).loading).toBe(true);
    expect(aggregateSessions([srv('a')], slots({ a: { sessions: null, error: 'offline' } })).loading).toBe(false);
    expect(aggregateSessions([srv('a')], slots({ a: { sessions: [], error: null } })).loading).toBe(false);
    expect(aggregateSessions([], new Map()).loading).toBe(false);
  });

  it('servidor sem slot ainda: bucket vazio presente (ordem preservada)', () => {
    const a = aggregateSessions([srv('a'), srv('b')], slots({ b: { sessions: [sess('y')], error: null } }));
    expect(a.byServer.map((b) => b.server.id)).toEqual(['a', 'b']);
    expect(a.byServer[0]).toMatchObject({ error: null, loaded: false, sessions: [] });
  });

  it('hidden esconde a sessão marcada (exclusão otimista) sem afetar as vizinhas', () => {
    const a = aggregateSessions(
      [srv('a')],
      slots({ a: { sessions: [sess('x'), sess('y')], error: null } }),
      new Set(['a::x']),
    );
    expect(a.rows.map((r) => r.name)).toEqual(['y']);
    expect(a.byServer[0].sessions.map((s) => s.name)).toEqual(['y']);
  });

  it('hidden é por servidor: mesma sessão em outro servidor não some', () => {
    const a = aggregateSessions(
      [srv('a'), srv('b')],
      slots({
        a: { sessions: [sess('x', { jsonl: '/j/ax.jsonl' })], error: null },
        b: { sessions: [sess('x', { jsonl: '/j/bx.jsonl' })], error: null },
      }),
      new Set(['a::x']),
    );
    expect(a.rows.map((r) => `${r.serverId}:${r.name}`)).toEqual(['b:x']);
  });
});

describe('sweepHidden', () => {
  it('mantém a marca enquanto a sessão ainda aparece na lista do servidor', () => {
    const kept = sweepHidden(new Set(['a::x']), slots({ a: { sessions: [sess('x')], error: null } }));
    expect([...kept]).toEqual(['a::x']);
  });

  it('remove a marca quando o SSE re-emite a lista sem a sessão (delete confirmado)', () => {
    const kept = sweepHidden(new Set(['a::x']), slots({ a: { sessions: [sess('y')], error: null } }));
    expect(kept.size).toBe(0);
  });

  it('servidor sem lista (offline/ainda sem slot): não dá pra saber, marca fica', () => {
    expect(sweepHidden(new Set(['a::x']), slots({ a: { sessions: null, error: 'offline' } })).size).toBe(1);
    expect(sweepHidden(new Set(['a::x']), new Map()).size).toBe(1);
  });
});
