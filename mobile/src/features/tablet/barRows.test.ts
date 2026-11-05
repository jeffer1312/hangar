import { describe, expect, it } from 'vitest';
import type { AggSession } from '@hangar/core';
import { barRows } from './barRows';

const session = (name: string, serverId = 'nb', extra: Partial<AggSession> = {}): AggSession => ({
  name, serverId, serverLabel: serverId, serverColor: '#999', state: 'idle', last_activity: 1, ...extra,
});
const identities = new Map([['nb', 'notebook'], ['vm', 'casa']]);

describe('barRows', () => {
  describe.each([true, false])('ordem do desktop com groupsFirst=%s', groupsFirst => {
    it.each([
      { order: { tabsWorkingFirst: true }, tail: ['b-ativa', 'c-ativa', 'a-pergunta', 'd-antiga', 'e-recente'] },
      { order: { tabsRecentFirst: true }, tail: ['b-ativa', 'c-ativa', 'e-recente', 'a-pergunta', 'd-antiga'] },
      { order: { tabsWorkingFirst: true, tabsRecentFirst: true }, tail: ['b-ativa', 'c-ativa', 'e-recente', 'a-pergunta', 'd-antiga'] },
    ])('mantém todos os grupos no topo com $order', ({ order, tail }) => {
      const rows = [
        session('a-pergunta', 'nb', { state: 'awaiting_input', last_reply_at: 3 }),
        session('b-ativa', 'nb', { state: 'working', last_reply_at: 1 }),
        session('c-ativa', 'nb', { state: 'working', last_reply_at: 100 }),
        session('d-antiga', 'nb', { last_reply_at: 1 }),
        session('e-recente', 'nb', { last_reply_at: 50 }),
        session('f-par', 'nb', { pair_gid: 'par', last_reply_at: 1000 }),
        session('g-par', 'nb', { pair_gid: 'par' }),
        session('h-mae', 'nb', { pair_gid: 'ligadas' }),
        session('i-filha', 'nb', { pair_gid: 'ligadas', linked_by: 'h-mae', last_reply_at: 2000 }),
        session('j-orq', 'nb', { pair_gid: 'orq', provider: 'orq' }),
        session('k-executor', 'nb', { pair_gid: 'orq' }),
      ];
      expect(barRows(rows, identities, groupsFirst, order).map(item => item.kind === 'group'
        ? item.members.map(s => s.name) : item.session.name)).toEqual([
        ['f-par', 'g-par'], ['h-mae', 'i-filha'], ['j-orq', 'k-executor'], ...tail,
      ]);
    });
  });

  it.each([NaN, Infinity, -Infinity])('trata last_reply_at=%s como zero na ordem por data', last_reply_at => {
    const rows = [session('a-invalida', 'nb', { last_reply_at }), session('b-sem-data'),
      session('c-recente', 'nb', { last_reply_at: 30 })];
    expect(barRows(rows, identities, true, { tabsRecentFirst: true }).map(item => item.kind === 'single' && item.session.name))
      .toEqual(['c-recente', 'a-invalida', 'b-sem-data']);
  });

  it.each([true, false])('separa grupos, trabalhando e paradas com gruposFirst=%s e ignora limite de uso', groupsFirst => {
    const waiting = session('pergunta', 'nb', { state: 'awaiting_input' });
    const limited = session('limite', 'nb', { state: 'working', limited: true });
    const working = session('ocupada', 'nb', { state: 'working' });
    const parent = session('mae');
    const child = session('filha', 'nb', { linked_by: 'mae' });
    const items = barRows([waiting, limited, working, parent, child], identities, groupsFirst, { tabsWorkingFirst: true });
    expect(items.map(item => item.kind === 'group' ? item.members.map(s => s.name) : item.session.name))
      .toEqual([['mae', 'filha'], 'ocupada', 'pergunta', 'limite']);
  });

  it('recentes separa trabalhando mesmo sem a outra opção e ordena só paradas por last_reply_at', () => {
    const rows = [
      session('a', 'nb', { state: 'working', last_reply_at: 1 }),
      session('b', 'nb', { state: 'working', last_reply_at: 90 }),
      session('limite', 'nb', { state: 'working', limited: true, last_reply_at: 50 }),
      session('pergunta', 'nb', { state: 'awaiting_input', last_reply_at: 2 }),
      session('recente', 'nb', { last_reply_at: 30 }),
      session('empate', 'nb', { last_reply_at: 30 }),
      session('sem-data'),
    ];
    const names = (items: ReturnType<typeof barRows>) => items.map(item => item.kind === 'single' && item.session.name);
    const options = { tabsRecentFirst: true };
    expect(names(barRows(rows, identities, true, options))).toEqual(['a', 'b', 'limite', 'empate', 'recente', 'pergunta', 'sem-data']);
    const updated = rows.map(s => s.name === 'a' ? { ...s, last_reply_at: 100 } : s);
    expect(names(barRows(updated, identities, true, options))).toEqual(names(barRows(rows, identities, true, options)));
    const finished = updated.map(s => s.name === 'a' ? { ...s, state: 'idle' as const } : s);
    expect(names(barRows(finished, identities, true, options))).toEqual(['b', 'a', 'limite', 'empate', 'recente', 'pergunta', 'sem-data']);
  });

  it('a data de um grupo não o troca de lugar nem muda a mãe', () => {
    const rows = [session('mae', 'nb', { pair_gid: 'g' }),
      session('filha', 'nb', { pair_gid: 'g', linked_by: 'mae', last_reply_at: 500 }),
      session('par-a', 'nb', { pair_gid: 'h', last_reply_at: 1000 }), session('par-b', 'nb', { pair_gid: 'h' })];
    expect(barRows(rows, identities, true, { tabsRecentFirst: true }).map(item => item.kind === 'group' && item.members.map(s => s.name)))
      .toEqual([['mae', 'filha'], ['par-a', 'par-b']]);
  });

  it('coloca grupos antes das sessões soltas, preservando a ordem da barra à esquerda', () => {
    const single = session('pergunta', 'vm', { state: 'awaiting_input' });
    const parent = session('mae');
    const child = session('filha', 'nb', { linked_by: 'mae' });
    const rows = [single, parent, child];
    expect(barRows(rows, identities).map(item => item.kind)).toEqual(['group', 'single']);
    expect(barRows(rows, identities, false).map(item => item.kind)).toEqual(['single', 'group']);
  });
  it('mistura os servidores sem cabeçalhos, na ordem de estado da lista do celular', () => {
    const idle = session('pronta');
    const working = session('trabalhando', 'vm', { state: 'working' });
    const waiting = session('pergunta', 'nb', { state: 'awaiting_input' });
    expect(barRows([idle, working, waiting], identities)).toEqual([
      { kind: 'single', session: waiting }, { kind: 'single', session: working }, { kind: 'single', session: idle },
    ]);
  });

  it('junta mãe e filha ligada, mesmo sem metadados de pareamento', () => {
    const parent = session('mae');
    const child = session('filha', 'nb', { linked_by: 'mae', state: 'working' });
    expect(barRows([parent, child], identities)).toEqual([{ kind: 'group', key: 'mother:nb::mae', label: null, members: [parent, child] }]);
  });

  it('numa cadeia mantém a mãe do topo antes das filhas sem reordenar o resto', () => {
    const parent = session('mae');
    const child = session('filha', 'nb', { linked_by: 'mae', state: 'working' });
    const grandchild = session('neta', 'nb', { linked_by: 'filha', state: 'awaiting_input' });
    const [group] = barRows([child, grandchild, parent], identities);
    expect(group).toEqual({ kind: 'group', key: 'mother:nb::mae', label: null, members: [parent, grandchild, child] });
  });

  it('põe o líder da orquestração primeiro mesmo quando outro membro pede resposta', () => {
    const parent = session('lider', 'nb', { pair_gid: 'g', provider: 'orq' });
    const child = session('executor', 'nb', { pair_gid: 'g', state: 'awaiting_input' });
    expect(barRows([child, parent], identities)).toEqual([
      { kind: 'group', key: 'local:nb/g', label: null, members: [parent, child] },
    ]);
  });

  it('mantém a chave pelo gid quando o estado troca o primeiro membro da lista', () => {
    const a = session('api', 'nb', { pair_gid: 'g', state: 'working' });
    const b = session('web', 'nb', { pair_gid: 'g' });
    const before = barRows([a, b], identities)[0];
    const after = barRows([{ ...a, state: 'idle' }, { ...b, state: 'awaiting_input' }], identities)[0];
    expect(before).toMatchObject({ kind: 'group', key: 'local:nb/g', label: null });
    expect(after).toMatchObject({ kind: 'group', key: 'local:nb/g', label: null });
  });

  it('não confunde mães com o mesmo nome em máquinas diferentes', () => {
    const parent = session('mae');
    const child = session('filha', 'vm', { linked_by: 'mae', state: 'working' });
    expect(barRows([parent, child], identities)).toEqual([
      { kind: 'single', session: child }, { kind: 'single', session: parent },
    ]);
  });

  it('junta pares entre servidores na posição do primeiro membro e conserva o rótulo', () => {
    const pair = { pair_gid: 'g', pair_owner: 'notebook', pair_task: 'PM-123 tela' };
    const a = session('api', 'nb', pair);
    const b = session('web', 'vm', { ...pair, state: 'awaiting_input' });
    const single = session('solta', 'nb', { state: 'working' });
    expect(barRows([single, a, b], identities)).toEqual([
      { kind: 'group', key: 'fed:notebook/g', label: 'PM-123 tela', members: [b, a] }, { kind: 'single', session: single },
    ]);
  });

  it('retira mortos sem criar grupo de um membro', () => {
    const parent = session('mae');
    expect(barRows([parent, session('filha', 'nb', { linked_by: 'mae', state: 'dead' })], identities))
      .toEqual([{ kind: 'single', session: parent }]);
  });

  it('tira as ocultas, menos a aberta, que sai quando a pessoa troca de sessão', () => {
    const rows = [session('a'), session('b-oculta', 'nb', { hidden_from_list: true }), session('c-oculta', 'nb', { hidden_from_list: true })];
    const names = (current?: { serverId: string; name: string }) =>
      barRows(rows, identities, true, {}, current).map(item => item.kind === 'single' ? item.session.name : item.key);
    expect(names()).toEqual(['a']);
    expect(names({ serverId: 'nb', name: 'b-oculta' })).toEqual(['a', 'b-oculta']);
    expect(names({ serverId: 'vm', name: 'b-oculta' })).toEqual(['a']);
    expect(names({ serverId: 'nb', name: 'a' })).toEqual(['a']);
  });
});
