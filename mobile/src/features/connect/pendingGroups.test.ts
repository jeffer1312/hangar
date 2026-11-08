import { describe, expect, it } from 'vitest';
import { pendingGroups } from './pendingGroups';

const row = (device_id: string, status: 'pending' | 'approved', install_id?: string, name = 'iPhone') =>
  ({ device_id, name, status, ...(install_id ? { install_id } : {}) });

describe('pendingGroups', () => {
  it('joins one phone pending on several machines into one card and keeps the rest apart', () => {
    const groups = pendingGroups([
      { serverId: 'casa', label: 'casa', rows: [row('d1', 'pending', 'ph1'), row('d2', 'approved', 'ph1'), row('d3', 'pending')] },
      { serverId: 'notebook', label: 'notebook', rows: [row('d4', 'pending', 'ph1'), row('d5', 'pending', 'ph2', 'Pixel')] },
      { serverId: 'vm', label: 'vm', rows: null },
    ]);
    expect(groups).toEqual([
      { key: 'ph1', name: 'iPhone', targets: [{ serverId: 'casa', label: 'casa', deviceId: 'd1' }, { serverId: 'notebook', label: 'notebook', deviceId: 'd4' }] },
      { key: 'casa:d3', name: 'iPhone', targets: [{ serverId: 'casa', label: 'casa', deviceId: 'd3' }] },
      { key: 'ph2', name: 'Pixel', targets: [{ serverId: 'notebook', label: 'notebook', deviceId: 'd5' }] },
    ]);
  });
});
