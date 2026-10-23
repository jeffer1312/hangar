import { describe, expect, it } from 'vitest';
import { groupGlobal, type AggSession } from '@hangar/core';
import { buildSessionSections } from './sessionSections';

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
      const sections = buildSessionSections(rows, mode, '', new Set(), undefined, groupGlobal(rows, ids));
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
    const one = buildSessionSections(withDead, 'none', '', new Set(), undefined, groupGlobal(withDead, ids));
    expect(one.every((s) => s.heads.size === 0)).toBe(true);
    const three = [...withDead, fed('ABC-1-proxy', nb, 'idle', 30)];
    const sections = buildSessionSections(three, 'none', '', new Set(), undefined, groupGlobal(three, ids));
    expect([...sections[0].heads.values()]).toMatchObject([{ label: 'ABC-1 bulas', machines: 1, size: 2 }]);
  });

  it('recolhido, o bloco fica só com o cabeçalho e conta quem espera resposta', () => {
    const asking = rows.map((r) => r.name === 'ABC-1-app' ? { ...r, state: 'awaiting_input' } as AggSession : r);
    const groups = groupGlobal(asking, ids);
    const open = buildSessionSections(asking, 'server', '', new Set(), undefined, groups);
    const head = [...open[0].heads.values()][0];
    const closed = buildSessionSections(asking, 'server', '', new Set([head.id]), undefined, groups);
    const flat = closed.flatMap((s) => s.data.map(key));
    expect(flat.filter((k) => /ABC-1|abc2/.test(k))).toHaveLength(1);
    expect(flat).toEqual(expect.arrayContaining(['nb::solta', 'vm::outra']));
    expect([...closed[0].heads.values()]).toMatchObject([{ id: head.id, collapsed: true, size: 4, awaiting: 1 }]);
  });

  it('a busca que acha um membro traz o bloco inteiro', () => {
    const sections = buildSessionSections(rows, 'server', '1-app', new Set(), undefined, groupGlobal(rows, ids));
    expect(sections.flatMap((s) => s.data.map(key)).filter((k) => /ABC-1|abc2/.test(k))).toHaveLength(4);
  });
});
