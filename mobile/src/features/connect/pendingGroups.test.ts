import { describe, expect, it } from 'vitest';
import { chosenTargets, pendingGroups } from './pendingGroups';

// Pedido de celular novo: o servidor manda o código pronto e a chave de grupo, nunca o id do celular.
const row = (device_id: string, status: 'pending' | 'approved', install_group?: string, name = 'iPhone', created_at = 100) =>
  ({ device_id, name, status, created_at, ...(install_group ? { install_group, install_code: `${install_group.toUpperCase()}-CODE` } : {}) });

describe('pendingGroups', () => {
  it('joins one phone pending on several machines into one card and keeps the rest apart', () => {
    const groups = pendingGroups([
      { serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', 'g1', 'iPhone', 300), row('d2', 'approved', 'g1'), row('d3', 'pending')] },
      { serverId: 'notebook', label: 'notebook', rows: [row('d4', 'pending', 'g1', 'iPhone', 200), row('d5', 'pending', 'g2', 'Pixel')] },
      { serverId: 'vm', label: 'vm', rows: null },
    ]);
    expect(groups.map(({ name, code, createdAt, targets }) => ({ name, code, createdAt, targets }))).toEqual([
      { name: 'iPhone', code: 'G1-CODE', createdAt: 200, targets: [
        { serverId: 'casa', label: 'casa', deviceId: 'd1' }, { serverId: 'notebook', label: 'notebook', deviceId: 'd4' }] },
      { name: 'iPhone', code: '', createdAt: 100, targets: [{ serverId: 'casa', label: 'casa', deviceId: 'd3' }] },
      { name: 'Pixel', code: 'G2-CODE', createdAt: 100, targets: [{ serverId: 'notebook', label: 'notebook', deviceId: 'd5' }] },
    ]);
  });

  it('never joins requests whose device names differ, even from the same phone', () => {
    const groups = pendingGroups([
      { serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', 'g1', 'iPhone da Ana')] },
      { serverId: 'notebook', label: 'notebook', rows: [row('d2', 'pending', 'g1', 'iPhone do Bruno')] },
    ]);
    expect(groups.map((group) => [group.name, group.targets.map((target) => target.deviceId)])).toEqual([
      ['iPhone da Ana', ['d1']], ['iPhone do Bruno', ['d2']],
    ]);
  });

  it('keeps old phones apart even on the same machine', () => {
    const groups = pendingGroups([{ serverId: 'casa', label: 'casa', rows: [row('l1', 'pending'), row('l2', 'pending')] }]);
    expect(groups.map((group) => group.targets.map((target) => target.deviceId))).toEqual([['l1'], ['l2']]);
  });

  it('keeps a machine unchecked when the phone retries there', () => {
    const lists = (casa: string) => [
      { serverId: 'casa', label: 'casa', rows: [row(casa, 'pending', 'g1')] },
      { serverId: 'notebook', label: 'notebook', rows: [row('d4', 'pending', 'g1')] },
    ];
    const off = new Set(['casa']);
    expect(chosenTargets(pendingGroups(lists('d1'))[0], off).map((target) => target.serverId)).toEqual(['notebook']);
    expect(chosenTargets(pendingGroups(lists('retry'))[0], off).map((target) => target.serverId)).toEqual(['notebook']);
  });

  it('keeps the lone box left approvable, with no box to re-check', () => {
    const [group] = pendingGroups([{ serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', 'g1')] }]);
    expect(chosenTargets(group, new Set(['casa'])).map((target) => target.serverId)).toEqual(['casa']);
  });

  it('marks both cards when two different phones show the same code', () => {
    const same = (device_id: string, install_group: string, name = 'iPhone') =>
      ({ device_id, name, status: 'pending' as const, install_group, install_code: 'ABC-DEF' });
    const groups = pendingGroups([
      { serverId: 'casa', label: 'casa', rows: [same('a', 'g1'), same('b', 'g2'), row('c', 'pending', 'g3', 'Pixel')] },
      { serverId: 'notebook', label: 'notebook', rows: [same('d', 'g1'), same('e', 'g1', 'iPhone da Ana')] },
    ]);
    expect(groups.map((group) => [group.name, group.clash])).toEqual([
      ['iPhone', true], ['iPhone', true], ['Pixel', false], ['iPhone da Ana', true],
    ]);
  });

  it('keeps the same object for a card that did not change', () => {
    const lists = [{ serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', 'g1'), row('d3', 'pending')] }];
    const before = pendingGroups(lists);
    const after = pendingGroups([{ ...lists[0], rows: [row('d1', 'pending', 'g1'), row('d3', 'pending'), row('d9', 'pending')] }], before);
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    expect(after).toHaveLength(3);
  });
});
