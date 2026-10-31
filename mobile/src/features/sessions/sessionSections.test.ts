import { describe, expect, it } from 'vitest';
import { groupGlobal, type AggSession } from '@hangar/core';
import { buildSessionSections, HIDDEN_SECTION_ID } from './sessionSections';
import * as m from '../../paraglide/messages';

const nb = { serverId: 'nb', serverLabel: 'notebook', serverColor: '#000' };
const vm = { serverId: 'vm', serverLabel: 'delphi', serverColor: '#111' };
const ids = new Map([['nb', 'lab'], ['vm', 'vm']]);
const fed = (name: string, at: typeof nb, state: AggSession['state'], last_activity: number) => ({
  ...at, name, state, last_activity, pair_gid: 'g1', pair_owner: 'lab', pair_task: 'ABC-1 bulas',
  pair_peers: ['lab::ABC-1-coord', 'lab::ABC-1-proxy', 'vm::ABC-1-app', 'vm::abc2-app'].filter((p) => !p.endsWith(`::${name}`)),
}) as AggSession;

const rows = [
  { ...nb, name: 'solta', state: 'idle', last_activity: 50 } as AggSession,
  fed('ABC-1-coord', nb, 'idle', 40),
  fed('ABC-1-proxy', nb, 'working', 30),
  { ...vm, name: 'outra', state: 'idle', last_activity: 20 } as AggSession,
  fed('ABC-1-app', vm, 'idle', 10),
  fed('abc2-app', vm, 'idle', 5),
];
const key = (r: AggSession) => `${r.serverId}::${r.name}`;

describe('grupo entre máquinas na lista', () => {
  for (const mode of ['server', 'none', 'project'] as const) {
    it(`modo ${mode}: um bloco só com os 4, na posição do primeiro membro`, () => {
      const sections = buildSessionSections(rows, 'collapsed', mode, '', new Set(), undefined, groupGlobal(rows, ids));
      const flat = sections.flatMap((s) => s.data.map(key));
      const members = flat.filter((k) => k.includes('ABC-1') || k.includes('abc2'));
      expect(members).toHaveLength(4);
      const start = flat.indexOf(members[0]);
      // contíguos e todos na mesma seção
      expect(flat.slice(start, start + 4)).toEqual(members);
      expect(sections.filter((s) => s.data.some((r) => members.includes(key(r))))).toHaveLength(1);
      // o primeiro membro na ordem da lista é o que estava trabalhando (seção do topo)
      expect(members[0]).toBe('nb::ABC-1-proxy');
      const head = sections.find((s) => s.heads.has(members[0]))!.heads.get(members[0]);
      expect(head).toMatchObject({ label: 'ABC-1 bulas', machines: 2, size: 4, collapsed: false });
    });
  }

  it('membro morto não conta: nem bloco de 1, nem máquina a mais', () => {
    const withDead = [fed('ABC-1-coord', nb, 'idle', 40), fed('ABC-1-app', vm, 'dead', 10)];
    const one = buildSessionSections(withDead, 'collapsed', 'none', '', new Set(), undefined, groupGlobal(withDead, ids));
    expect(one.every((s) => s.heads.size === 0)).toBe(true);
    const three = [...withDead, fed('ABC-1-proxy', nb, 'idle', 30)];
    const sections = buildSessionSections(three, 'collapsed', 'none', '', new Set(), undefined, groupGlobal(three, ids));
    expect([...sections[0].heads.values()]).toMatchObject([{ label: 'ABC-1 bulas', machines: 1, size: 2 }]);
  });

  it('recolhido, o bloco fica só com o cabeçalho e conta quem espera resposta', () => {
    const asking = rows.map((r) => r.name === 'ABC-1-app' ? { ...r, state: 'awaiting_input' } as AggSession : r);
    const groups = groupGlobal(asking, ids);
    const open = buildSessionSections(asking, 'collapsed', 'server', '', new Set(), undefined, groups);
    const head = [...open[0].heads.values()][0];
    const closed = buildSessionSections(asking, 'collapsed', 'server', '', new Set([head.id]), undefined, groups);
    const flat = closed.flatMap((s) => s.data.map(key));
    expect(flat.filter((k) => /ABC-1|abc2/.test(k))).toHaveLength(1);
    expect(closed.reduce((n, s) => n + s.total, 0)).toBe(rows.length);
    expect(flat).toEqual(expect.arrayContaining(['nb::solta', 'vm::outra']));
    const searching = buildSessionSections(asking, 'collapsed', 'server', 'abc2', new Set([head.id]), undefined, groups);
    expect(searching.flatMap((s) => s.data.map(key))).toContain('vm::abc2-app');
    expect([...closed[0].heads.values()]).toMatchObject([{ id: head.id, collapsed: true, size: 4, awaiting: 1 }]);
  });

  it('a busca que acha um membro traz o bloco inteiro', () => {
    const sections = buildSessionSections(rows, 'collapsed', 'server', '1-app', new Set(), undefined, groupGlobal(rows, ids));
    expect(sections.flatMap((s) => s.data.map(key)).filter((k) => /ABC-1|abc2/.test(k))).toHaveLength(4);
  });
});

describe('sessões ocultas', () => {
  const hiddenRows = [
    { ...nb, name: 'visivel', state: 'idle', last_activity: 50 } as AggSession,
    { ...nb, name: 'oculta-nb', state: 'idle', last_activity: 40, hidden_from_list: true } as AggSession,
    { ...vm, name: 'oculta-vm', state: 'working', last_activity: 30, hidden_from_list: true } as AggSession,
    { ...vm, name: 'morta', state: 'dead', last_activity: 20, hidden_from_list: true } as AggSession,
    // de outra pessoa, ancorada na oculta: vai junto com ela
    { ...nb, serverId: 'fora', name: 'convidada', state: 'idle', last_activity: 10,
      outside: { localServerId: 'nb', localSession: 'oculta-nb', owner: 'ana', alias: 'ana' } } as unknown as AggSession,
  ];
  const keys = (sections: ReturnType<typeof buildSessionSections>) => sections.flatMap((s) => s.data.map(key));

  it('lista: seção "Ocultas (N)" no fim, recolhida, juntando as máquinas e sem a morta', () => {
    const closed = buildSessionSections(hiddenRows, 'collapsed', 'server', '', new Set());
    const last = closed[closed.length - 1];
    expect(last).toMatchObject({ id: HIDDEN_SECTION_ID, label: m.session_hidden_group({ n: 3 }), total: 3, collapsed: true, data: [] });
    expect(keys(closed)).toEqual(['nb::visivel']);
    const open = buildSessionSections(hiddenRows, 'expanded', 'server', '', new Set());
    // trabalhando não sobe ao topo: a oculta fica na seção dela
    expect(open.map((s) => s.id)).toEqual(['nb', HIDDEN_SECTION_ID]);
    expect(open[1].data.map(key)).toEqual(['nb::oculta-nb', 'fora::convidada', 'vm::oculta-vm']);
  });

  it('lista: buscando, a seção abre só com as que casam; sem oculta, não aparece', () => {
    const found = buildSessionSections(hiddenRows, 'collapsed', 'server', 'oculta-vm', new Set());
    expect(found.map((s) => s.id)).toEqual([HIDDEN_SECTION_ID]);
    expect(found[0]).toMatchObject({ collapsed: false, total: 1 });
    expect(buildSessionSections(rows, 'collapsed', 'server', '', new Set()).some((s) => s.id === HIDDEN_SECTION_ID)).toBe(false);
  });

  it('barra do tablet: as ocultas e a de fora ancorada nelas somem', () => {
    const sections = buildSessionSections(hiddenRows, 'exclude', 'none', '', new Set());
    expect(keys(sections)).toEqual(['nb::visivel']);
  });

  it('seletor: as ocultas ficam no lugar de sempre, sem seção própria', () => {
    const sections = buildSessionSections(hiddenRows, 'include', 'server', '', new Set());
    expect(sections.some((s) => s.id === HIDDEN_SECTION_ID)).toBe(false);
    expect(keys(sections)).toEqual(expect.arrayContaining(['nb::visivel', 'nb::oculta-nb', 'vm::oculta-vm', 'fora::convidada']));
    expect(sections.find((s) => s.id === 'state:working')?.data.map(key)).toEqual(['vm::oculta-vm']);
  });
});
