import { describe, expect, it } from 'vitest';
import cases from '../../../../packages/core/src/installCode.fixtures.json';
import { chosenTargets, pendingGroups } from './pendingGroups';

const row = (device_id: string, status: 'pending' | 'approved', install_id?: string, name = 'iPhone', created_at = 100) =>
  ({ device_id, name, status, created_at, ...(install_id ? { install_id } : {}) });
const [phone, pixel] = cases;

describe('pendingGroups', () => {
  it('joins one phone pending on several machines into one card and keeps the rest apart', () => {
    const groups = pendingGroups([
      { serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', phone.install_id, 'iPhone', 300), row('d2', 'approved', phone.install_id), row('d3', 'pending')] },
      { serverId: 'notebook', label: 'notebook', rows: [row('d4', 'pending', phone.install_id, 'iPhone', 200), row('d5', 'pending', pixel.install_id, 'Pixel')] },
      { serverId: 'vm', label: 'vm', rows: null },
    ]);
    expect(groups.map(({ name, code, createdAt, targets }) => ({ name, code, createdAt, targets }))).toEqual([
      { name: 'iPhone', code: phone.code, createdAt: 200, targets: [
        { serverId: 'casa', label: 'casa', deviceId: 'd1', stale: [] }, { serverId: 'notebook', label: 'notebook', deviceId: 'd4', stale: [] }] },
      { name: 'iPhone', code: '', createdAt: 100, targets: [{ serverId: 'casa', label: 'casa', deviceId: 'd3', stale: [] }] },
      { name: 'Pixel', code: pixel.code, createdAt: 100, targets: [{ serverId: 'notebook', label: 'notebook', deviceId: 'd5', stale: [] }] },
    ]);
  });

  it('never joins requests whose device names differ, even with the same install id', () => {
    const groups = pendingGroups([
      { serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', phone.install_id, 'iPhone da Ana')] },
      { serverId: 'notebook', label: 'notebook', rows: [row('d2', 'pending', phone.install_id, 'iPhone do Bruno')] },
    ]);
    expect(groups.map((group) => [group.name, group.targets.map((target) => target.deviceId)])).toEqual([
      ['iPhone da Ana', ['d1']], ['iPhone do Bruno', ['d2']],
    ]);
  });

  it('keeps one box per machine: the newest retry, with the older requests answered along with it', () => {
    const groups = pendingGroups([{ serverId: 'casa', label: 'casa', rows: [
      row('old', 'pending', phone.install_id, 'iPhone', 100), row('new', 'pending', phone.install_id, 'iPhone', 200),
      row('legacy1', 'pending'), row('legacy2', 'pending')] }]);
    expect(groups.map((group) => group.targets)).toEqual([
      [{ serverId: 'casa', label: 'casa', deviceId: 'new', stale: ['old'] }],
      [{ serverId: 'casa', label: 'casa', deviceId: 'legacy1', stale: [] }],
      [{ serverId: 'casa', label: 'casa', deviceId: 'legacy2', stale: [] }],
    ]);
  });

  it('keeps a machine unchecked when the phone retries there', () => {
    const lists = (casa: string) => [
      { serverId: 'casa', label: 'casa', rows: [row(casa, 'pending', phone.install_id, 'iPhone', casa === 'd1' ? 100 : 200)] },
      { serverId: 'notebook', label: 'notebook', rows: [row('d4', 'pending', phone.install_id)] },
    ];
    const off = new Set(['casa']);
    expect(chosenTargets(pendingGroups(lists('d1'))[0], off).map((target) => target.serverId)).toEqual(['notebook']);
    expect(chosenTargets(pendingGroups(lists('retry'))[0], off).map((target) => target.serverId)).toEqual(['notebook']);
  });

  it('keeps the same object for a card that did not change', () => {
    const lists = [{ serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', phone.install_id), row('d3', 'pending')] }];
    const before = pendingGroups(lists);
    const after = pendingGroups([{ ...lists[0], rows: [row('d1', 'pending', phone.install_id), row('d3', 'pending'), row('d9', 'pending')] }], before);
    expect(after[0]).toBe(before[0]);
    expect(after[1]).toBe(before[1]);
    expect(after).toHaveLength(3);
  });
});
