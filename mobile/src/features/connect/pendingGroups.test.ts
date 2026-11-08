import { describe, expect, it } from 'vitest';
import { installCode, pendingGroups } from './pendingGroups';

const row = (device_id: string, status: 'pending' | 'approved', install_id?: string, name = 'iPhone', created_at = 100) =>
  ({ device_id, name, status, created_at, ...(install_id ? { install_id } : {}) });

describe('pendingGroups', () => {
  it('joins one phone pending on several machines into one card and keeps the rest apart', () => {
    const groups = pendingGroups([
      { serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', 'ab12cd', 'iPhone', 300), row('d2', 'approved', 'ab12cd'), row('d3', 'pending')] },
      { serverId: 'notebook', label: 'notebook', rows: [row('d4', 'pending', 'ab12cd', 'iPhone', 200), row('d5', 'pending', 'ff99', 'Pixel')] },
      { serverId: 'vm', label: 'vm', rows: null },
    ]);
    expect(groups.map(({ name, code, createdAt, targets }) => ({ name, code, createdAt, targets }))).toEqual([
      { name: 'iPhone', code: 'AB12', createdAt: 200,
        targets: [{ serverId: 'casa', label: 'casa', deviceId: 'd1' }, { serverId: 'notebook', label: 'notebook', deviceId: 'd4' }] },
      { name: 'iPhone', code: '', createdAt: 100, targets: [{ serverId: 'casa', label: 'casa', deviceId: 'd3' }] },
      { name: 'Pixel', code: 'FF99', createdAt: 100, targets: [{ serverId: 'notebook', label: 'notebook', deviceId: 'd5' }] },
    ]);
  });

  it('never joins requests whose device names differ, even with the same install id', () => {
    const groups = pendingGroups([
      { serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', 'ab12cd', 'iPhone da Ana')] },
      { serverId: 'notebook', label: 'notebook', rows: [row('d2', 'pending', 'ab12cd', 'iPhone do Bruno')] },
    ]);
    expect(groups.map((group) => [group.name, group.targets.map((target) => target.deviceId)])).toEqual([
      ['iPhone da Ana', ['d1']], ['iPhone do Bruno', ['d2']],
    ]);
  });

  it('shows the same short code the phone shows', () => {
    expect(installCode('0a1b2c3d4e5f')).toBe('0A1B');
  });
});
