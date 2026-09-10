import { describe, expect, it } from 'vitest';
import type { Slot } from './sessions';
import type { Server } from './servers';
import type { SessionInfo } from './types';
import { aggregateSessions, epocasDeRecriacao, sweepHidden } from './sessions';

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

describe('epocasDeRecriacao', () => {
  const zero = { vistas: new Map<string, string | null>(), sumidas: new Map<string, string | null>(), epochs: new Map<string, number>() };

  it('sessão que some e volta com o mesmo nome e OUTRO transcript ganha época nova; as demais não mudam', () => {
    const e1 = epocasDeRecriacao(zero, slots({ a: { sessions: [sess('x'), sess('y')], error: null } }));
    expect(e1.epochs.size).toBe(0);                       // primeira lista: nada foi recriado
    const e2 = epocasDeRecriacao(e1, slots({ a: { sessions: [sess('y')], error: null } }));
    expect(e2.epochs.get('a::x')).toBeUndefined();        // sumir não é recriar
    // Codex recém-criado ainda não tem thread: jsonl null. Diferente do da morta = recriada.
    const e3 = epocasDeRecriacao(e2, slots({ a: { sessions: [sess('y'), sess('x', { jsonl: null })], error: null } }));
    expect(e3.epochs.get('a::x')).toBe(1);
    expect(e3.epochs.get('a::y')).toBeUndefined();
    expect(e3.epochs).not.toBe(e2.epochs);                // reatribuído: quem lê num $derived acorda
  });

  it('sumiço passageiro (mesmo transcript ao voltar) não é recriação: o Chat não remonta', () => {
    const e1 = epocasDeRecriacao(zero, slots({ a: { sessions: [sess('x')], error: null } }));
    const e2 = epocasDeRecriacao(e1, slots({ a: { sessions: [], error: null } }));
    const e3 = epocasDeRecriacao(e2, slots({ a: { sessions: [sess('x')], error: null } }));
    expect(e3.epochs.size).toBe(0);
    expect(e3.epochs).toBe(e1.epochs);
    // A volta com o mesmo transcript esquece o sumiço: uma recriação de verdade depois ainda conta.
    const e4 = epocasDeRecriacao(e3, slots({ a: { sessions: [], error: null } }));
    const e5 = epocasDeRecriacao(e4, slots({ a: { sessions: [sess('x', { jsonl: '/j/x2.jsonl' })], error: null } }));
    expect(e5.epochs.get('a::x')).toBe(1);
  });

  it('servidor offline guarda a última lista: não conta como sumiço nem como volta', () => {
    const e1 = epocasDeRecriacao(zero, slots({ a: { sessions: [sess('x')], error: null } }));
    const e2 = epocasDeRecriacao(e1, slots({ a: { sessions: [sess('x')], error: 'offline' } }));
    const e3 = epocasDeRecriacao(e2, slots({ a: { sessions: [sess('x')], error: null } }));
    expect(e3.epochs.size).toBe(0);
    expect(e3.epochs).toBe(e1.epochs);
  });
});

describe('sweepHidden', () => {
  const marca = new Map([['a::x', '/j/x.jsonl']]);

  it('mantém a marca enquanto a sessão ainda aparece na lista do servidor', () => {
    const kept = sweepHidden(marca, slots({ a: { sessions: [sess('x')], error: null } }));
    expect([...kept.keys()]).toEqual(['a::x']);
  });

  it('remove a marca quando o SSE re-emite a lista sem a sessão (delete confirmado)', () => {
    const kept = sweepHidden(marca, slots({ a: { sessions: [sess('y')], error: null } }));
    expect(kept.size).toBe(0);
  });

  it('sessão NOVA com o mesmo nome (outro jsonl, ou nenhum) não herda a marca da que morreu', () => {
    expect(sweepHidden(marca, slots({ a: { sessions: [sess('x', { jsonl: '/j/x2.jsonl' })], error: null } })).size).toBe(0);
    expect(sweepHidden(marca, slots({ a: { sessions: [sess('x', { jsonl: null })], error: null } })).size).toBe(0);
    // Sem jsonl nos dois lados (sessão sem transcript excluída e recriada) não há como separar: fica.
    expect(sweepHidden(new Map([['a::x', null]]), slots({ a: { sessions: [sess('x', { jsonl: null })], error: null } })).size).toBe(1);
  });

  it('servidor sem lista (offline/ainda sem slot): não dá pra saber, marca fica', () => {
    expect(sweepHidden(marca, slots({ a: { sessions: null, error: 'offline' } })).size).toBe(1);
    expect(sweepHidden(marca, new Map()).size).toBe(1);
  });
});
