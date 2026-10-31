import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Slot } from './sessions';
import type { Server } from './servers';
import type { SessionInfo } from './types';
import { aggregateSessions, epocasDeRecriacao, machineNoticeVisible, machinesAllDown, nextNoticeEnd, OFFLINE_NOTICE_MS, patchSlot, splitHidden, sweepHidden, groupHideTargets } from './sessions';

const srv = (id: string): Server => ({ id, label: `srv-${id}`, baseUrl: `http://${id}`, token: 't' });
const sess = (name: string, extra: Partial<SessionInfo> = {}): SessionInfo =>
  ({ name, state: 'idle', jsonl: `/j/${name}.jsonl`, ...extra });
const slots = (m: Record<string, Slot>) => new Map(Object.entries(m));

describe('aggregateSessions', () => {
  it('chaveiro trancado marca o bucket do servidor, menos quando ele está fora do ar', () => {
    const a = aggregateSessions([srv('a'), srv('b'), srv('c')], slots({
      a: { sessions: [], error: null, keyring: 'locked' },
      b: { sessions: [], error: null, keyring: 'unlocked' },
      c: { sessions: [], error: 'offline', keyring: 'locked' },
    }));
    expect(a.byServer.map((b) => b.keyringLocked)).toEqual([true, false, false]);
  });

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

  it('servidor offline com cache não exibe sessões antigas como disponíveis', () => {
    const a = aggregateSessions([srv('a')], slots({ a: { sessions: [sess('x')], error: 'offline' } }));
    expect(a.byServer[0]).toMatchObject({ error: 'offline', loaded: true });
    expect(a.rows).toEqual([]);
  });

  it('rota offline não esconde a mesma sessão da rota local saudável', () => {
    const duplicate = sess('x');
    const a = aggregateSessions([srv('vpn'), srv('local')], slots({
      vpn: { sessions: [duplicate], error: 'offline' },
      local: { sessions: [duplicate], error: null },
    }));
    expect(a.rows.map(r => r.serverId)).toEqual(['local']);
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

  it('marca as linhas de servidor de convite', () => {
    const convite: Server = { ...srv('c'), invite: true };
    const a = aggregateSessions([srv('a'), convite], slots({
      a: { sessions: [sess('x')], error: null },
      c: { sessions: [sess('y')], error: null },
    }));
    expect(a.rows.map((r) => [r.name, r.serverInvite])).toEqual([['x', false], ['y', true]]);
  });

  it('a marca de convite vale mesmo com a lista do servidor reaproveitada', () => {
    const lista = [sess('y')];
    const s = slots({ c: { sessions: lista, error: null } });
    expect(aggregateSessions([srv('c')], s).rows[0].serverInvite).toBe(false);
    expect(aggregateSessions([{ ...srv('c'), invite: true }], s).rows[0].serverInvite).toBe(true);
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

describe('aviso de máquina fora do ar', () => {
  afterEach(() => vi.useRealTimers());

  it('a queda conta da primeira falha: novas tentativas não reabrem a janela, e a resposta a encerra', () => {
    vi.useFakeTimers({ now: 1_000 });
    const m = new Map<string, Slot>();
    m.set('a', patchSlot(m, 'a', { error: 'offline' }));
    expect(m.get('a')!.offlineSince).toBe(1_000);
    vi.setSystemTime(20_000);
    m.set('a', patchSlot(m, 'a', { error: 'offline', keyring: undefined }));
    expect(m.get('a')!.offlineSince).toBe(1_000);
    m.set('a', patchSlot(m, 'a', { sessions: [], error: null }));
    expect(m.get('a')!.offlineSince).toBeUndefined();
    vi.setSystemTime(90_000);
    m.set('a', patchSlot(m, 'a', { error: 'offline' }));
    expect(m.get('a')!.offlineSince).toBe(90_000);
    expect(aggregateSessions([srv('a')], m).byServer[0].offlineSince).toBe(90_000);
  });

  it('a queda vem do início gravado e não reabre quando o erro alterna antes de uma lista boa', () => {
    vi.useFakeTimers({ now: 500_000 });
    const m = new Map<string, Slot>();
    m.set('a', patchSlot(m, 'a', { error: 'offline', offlineSince: 7_000 }));
    expect(m.get('a')!.offlineSince).toBe(7_000);
    m.set('a', patchSlot(m, 'a', { error: 'erro do servidor' }));
    expect(m.get('a')!.offlineSince).toBe(7_000);
    m.set('a', patchSlot(m, 'a', { error: 'offline', offlineSince: undefined }));
    expect(m.get('a')!.offlineSince).toBe(7_000);
    // O início gravado vence o da memória: a recarga pode ter lido um mais antigo.
    m.set('a', patchSlot(m, 'a', { error: 'offline', offlineSince: 3_000 }));
    expect(m.get('a')!.offlineSince).toBe(3_000);
    m.set('b', patchSlot(m, 'b', { error: 'erro do servidor' }));
    expect(m.get('b')!.offlineSince).toBeUndefined();
  });

  it('todas as máquinas próprias fora do ar, convite não conta', () => {
    const down = { server: srv('a'), sessions: [], loaded: false, error: 'offline' };
    const up = { server: srv('b'), sessions: [], loaded: true, error: null };
    const guest = { server: { ...srv('g'), invite: true }, sessions: [], loaded: true, error: null };
    expect(machinesAllDown([down, guest])).toBe(true);
    expect(machinesAllDown([down, up])).toBe(false);
    expect(machinesAllDown([down, { ...up, error: 'erro do servidor' }])).toBe(false);
    expect(machinesAllDown([guest])).toBe(false);
    const disabled = { ...up, server: { ...srv('d'), disabled: true } };
    const ephemeral = { ...up, server: { ...srv('e'), ephemeral: true } };
    const removed = { ...up, server: { ...srv('r'), removed: true } };
    expect(machinesAllDown([down, disabled, ephemeral, removed])).toBe(true);
  });

  it('offline só aparece dentro da janela; outro erro fica; sem erro não há aviso', () => {
    const since = 5_000;
    const off = { error: 'offline', offlineSince: since };
    expect(machineNoticeVisible(off, since)).toBe(true);
    expect(machineNoticeVisible(off, since + OFFLINE_NOTICE_MS - 1)).toBe(true);
    expect(machineNoticeVisible(off, since + OFFLINE_NOTICE_MS)).toBe(false);
    expect(machineNoticeVisible({ error: 'offline' }, since)).toBe(false);
    expect(machineNoticeVisible({ error: 'list-unavailable' }, Number.MAX_SAFE_INTEGER)).toBe(true);
    expect(machineNoticeVisible({ error: null }, since)).toBe(false);
  });

  it('próximo fim de aviso é o mais cedo ainda por vir', () => {
    const b = [
      { error: 'offline', offlineSince: 10_000 },
      { error: 'offline', offlineSince: 1_000 },
      { error: 'list-unavailable' },
      { error: null },
    ];
    expect(nextNoticeEnd(b, 0)).toBe(1_000 + OFFLINE_NOTICE_MS);
    expect(nextNoticeEnd(b, 1_000 + OFFLINE_NOTICE_MS)).toBe(10_000 + OFFLINE_NOTICE_MS);
    expect(nextNoticeEnd(b, 10_000 + OFFLINE_NOTICE_MS)).toBeNull();
  });
});

describe('splitHidden', () => {
  it('separa pelas linhas marcadas, preservando a ordem de cada grupo', () => {
    const rows = [sess('a'), sess('b', { hidden_from_list: true }), sess('c', { hidden_from_list: false }), sess('d', { hidden_from_list: true })];
    const { shown, hidden } = splitHidden(rows);
    expect(shown.map((s) => s.name)).toEqual(['a', 'c']);
    expect(hidden.map((s) => s.name)).toEqual(['b', 'd']);
  });
});

describe('groupHideTargets', () => {
  type Row = Parameters<typeof groupHideTargets>[0];
  const row = (serverId: string, name: string, extra: Partial<Row> = {}): Row =>
    ({ serverId, name, pair_gid: 'g1', state: 'idle', ...extra }) as Row;

  it('fora de grupo devolve só a própria', () => {
    const solo = row('pc', 'solo', { pair_gid: null });
    expect(groupHideTargets(solo, [solo, row('pc', 'x')])).toEqual([solo]);
  });

  it('junta o grupo de todas as máquinas, a própria primeiro, sem as de fora, ocultas ou esperando', () => {
    const me = row('pc', 'a');
    const rows = [
      row('pc', 'b'),
      me,
      row('nb', 'c'),
      row('pc', 'outra-pessoa', { outside: { alias: 'x', owner: 'y', localServerId: 'pc', localSession: 'a' } }),
      row('pc', 'ja-oculta', { hidden_from_list: true }),
      row('pc', 'esperando', { state: 'awaiting_input' }),
      row('pc', 'pergunta', { pending_questions: 1 }),
      row('pc', 'outro-grupo', { pair_gid: 'g2' }),
      row('convite', 'de-convite', { serverInvite: true }),
      row('pc', 'par-so-ver', { guest_kind: 'pair' }),
    ];
    expect(groupHideTargets(me, rows).map((r) => `${r.serverId}::${r.name}`)).toEqual(['pc::a', 'pc::b', 'nb::c']);
  });
});
