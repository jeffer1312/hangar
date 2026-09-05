import { describe, it, expect } from 'vitest';
import { agruparSessoes } from './agruparSessoes';
import type { AggSession } from './types';

const s = (name: string, serverId: string, cwd: string, state: AggSession['state'] = 'idle'): AggSession =>
  ({ name, serverId, serverLabel: serverId, serverColor: '#123', cwd, state } as AggSession);

describe('agruparSessoes', () => {
  const rows = [s('b', 'A', '/x/repo1'), s('a', 'A', '/x/repo1', 'awaiting_input'), s('c', 'B', '/y/repo2')];
  it('none = um grupo só, ordenado por sortSessions', () => {
    const g = agruparSessoes(rows, 'none');
    expect(g).toHaveLength(1);
    expect(g[0].sessions.map((x) => x.name)).toEqual(['a', 'b', 'c']);
  });
  it('server = um grupo por servidor, cor do servidor', () => {
    const g = agruparSessoes(rows, 'server');
    expect(g.map((x) => [x.id, x.sessions.length])).toEqual([['A', 2], ['B', 1]]);
    expect(g[0].color).toBe('#123');
  });
  it('project = um grupo por cwd, rótulo é o basename', () => {
    const g = agruparSessoes(rows, 'project');
    expect(g.map((x) => x.label)).toEqual(['repo1', 'repo2']);
  });
});
