import { describe, it, expect } from 'vitest';
import {
  abbrevNum, attentionFeed, countAwaiting, effectiveGroupBy, groupSelectedByServer, nextAwaiting,
  projectKey, projectLabel, encodeCompareIds, parseCompareIds, latestAssistantEvent,
} from './format';
import type { ChatEvent } from './types';

describe('abbrevNum', () => {
  it('abbreviates millions', () => {
    expect(abbrevNum(3_668_662)).toBe('3.7M');
  });
  it('abbreviates billions', () => {
    expect(abbrevNum(1_539_946_914)).toBe('1.5B');
  });
  it('abbreviates thousands', () => {
    expect(abbrevNum(12_500)).toBe('12.5K');
  });
  it('leaves small numbers as-is', () => {
    expect(abbrevNum(999)).toBe('999');
  });
  it('drops trailing .0', () => {
    expect(abbrevNum(2_000_000)).toBe('2M');
  });
});

describe('countAwaiting', () => {
  it('counts only awaiting_input sessions', () => {
    const sessions = [
      { state: 'awaiting_input' as const },
      { state: 'working' as const },
      { state: 'awaiting_input' as const },
      { state: 'idle' as const },
      { state: 'dead' as const },
    ];
    expect(countAwaiting(sessions)).toBe(2);
  });

  it('returns 0 for an empty list', () => {
    expect(countAwaiting([])).toBe(0);
  });

  it('returns 0 when none are awaiting', () => {
    expect(countAwaiting([{ state: 'working' as const }, { state: 'idle' as const }])).toBe(0);
  });
});

describe('nextAwaiting', () => {
  it('returns null when nothing is awaiting', () => {
    const sessions = [{ name: 'a', state: 'idle' as const }, { name: 'b', state: 'working' as const }];
    expect(nextAwaiting(sessions, 'a')).toBeNull();
  });

  it('jumps to the single awaiting session when current is not it', () => {
    const sessions = [
      { name: 'a', state: 'idle' as const },
      { name: 'b', state: 'awaiting_input' as const },
    ];
    expect(nextAwaiting(sessions, 'a')).toBe('b');
  });

  it('wraps around from the last awaiting session back to the first', () => {
    const sessions = [
      { name: 'a', state: 'awaiting_input' as const },
      { name: 'b', state: 'awaiting_input' as const },
      { name: 'c', state: 'awaiting_input' as const },
    ];
    expect(nextAwaiting(sessions, 'c')).toBe('a');
  });

  it('skips past the current session when it is itself awaiting', () => {
    const sessions = [
      { name: 'a', state: 'awaiting_input' as const },
      { name: 'b', state: 'awaiting_input' as const },
      { name: 'c', state: 'awaiting_input' as const },
    ];
    expect(nextAwaiting(sessions, 'a')).toBe('b');
  });

  it('returns itself when it is the only awaiting session', () => {
    const sessions = [{ name: 'a', state: 'awaiting_input' as const }];
    expect(nextAwaiting(sessions, 'a')).toBe('a');
  });
});

describe('attentionFeed', () => {
  it('keeps only awaiting_input sessions', () => {
    const sessions = [
      { name: 'a', state: 'working' as const, last_activity: 1 },
      { name: 'b', state: 'awaiting_input' as const, last_activity: 2 },
      { name: 'c', state: 'idle' as const, last_activity: 3 },
      { name: 'd', state: 'awaiting_input' as const, last_activity: 4 },
    ];
    expect(attentionFeed(sessions).map((s) => s.name)).toEqual(['b', 'd']);
  });

  it('sorts oldest-waiting (smallest last_activity) first', () => {
    const sessions = [
      { name: 'newer', state: 'awaiting_input' as const, last_activity: 200 },
      { name: 'older', state: 'awaiting_input' as const, last_activity: 100 },
      { name: 'mid', state: 'awaiting_input' as const, last_activity: 150 },
    ];
    expect(attentionFeed(sessions).map((s) => s.name)).toEqual(['older', 'mid', 'newer']);
  });

  it('merges across servers (any shape with the fields) and puts missing last_activity last', () => {
    const sessions = [
      { name: 'z', state: 'awaiting_input' as const, last_activity: null, serverId: 's2' },
      { name: 'a', state: 'awaiting_input' as const, last_activity: 50, serverId: 's1' },
    ];
    expect(attentionFeed(sessions).map((s) => s.name)).toEqual(['a', 'z']);
  });

  it('breaks ties by name for a stable order', () => {
    const sessions = [
      { name: 'beta', state: 'awaiting_input' as const, last_activity: 10 },
      { name: 'alpha', state: 'awaiting_input' as const, last_activity: 10 },
    ];
    expect(attentionFeed(sessions).map((s) => s.name)).toEqual(['alpha', 'beta']);
  });

  it('returns an empty list when nothing is awaiting', () => {
    expect(attentionFeed([{ name: 'a', state: 'idle' as const }])).toEqual([]);
  });
});

describe('projectKey', () => {
  it('strips a trailing slash', () => {
    expect(projectKey('/home/user/repo/')).toBe('/home/user/repo');
  });
  it('keeps the root path as-is', () => {
    expect(projectKey('/')).toBe('/');
  });
  it('keeps a nested path as-is (no trailing slash)', () => {
    expect(projectKey('/home/user/repo/backend')).toBe('/home/user/repo/backend');
  });
  it('same cwd with/without trailing slash -> same key', () => {
    expect(projectKey('/a/b/c')).toBe(projectKey('/a/b/c/'));
  });
  it('falls back to a fixed sentinel when there is no cwd', () => {
    const noCwd = projectKey(undefined);
    expect(projectKey(null)).toBe(noCwd);
    expect(projectKey('')).toBe(noCwd);
    expect(noCwd).not.toBe(projectKey('/'));
  });
});

describe('groupSelectedByServer', () => {
  const sessions = [
    { name: 'a', serverId: 's1' },
    { name: 'b', serverId: 's1' },
    { name: 'c', serverId: 's2' },
    { name: 'd', serverId: 's2' },
  ];

  it('groups selected names by their owning server', () => {
    const selected = new Set(['s1:a', 's2:c', 's2:d']);
    const grouped = groupSelectedByServer(sessions, selected);
    expect(grouped.get('s1')).toEqual(['a']);
    expect(grouped.get('s2')).toEqual(['c', 'd']);
    expect(grouped.size).toBe(2);
  });

  it('omits servers with nothing selected', () => {
    const grouped = groupSelectedByServer(sessions, new Set(['s1:a']));
    expect(grouped.has('s2')).toBe(false);
  });

  it('returns an empty map when nothing is selected', () => {
    expect(groupSelectedByServer(sessions, new Set()).size).toBe(0);
  });

  it('ignores selection keys that do not match any session', () => {
    const grouped = groupSelectedByServer(sessions, new Set(['s1:a', 's3:ghost']));
    expect(grouped.size).toBe(1);
    expect(grouped.get('s1')).toEqual(['a']);
  });
});

describe('effectiveGroupBy', () => {
  it('keeps the preference when there are 2+ servers', () => {
    expect(effectiveGroupBy('server', 2)).toBe('server');
    expect(effectiveGroupBy('project', 3)).toBe('project');
  });
  it('forces project with a single server (server grouping would be one giant group)', () => {
    expect(effectiveGroupBy('server', 1)).toBe('project');
    expect(effectiveGroupBy('project', 1)).toBe('project');
  });
  it('forces project with zero servers too', () => {
    expect(effectiveGroupBy('server', 0)).toBe('project');
  });
});

describe('projectLabel', () => {
  it('is the basename for a trailing-slash path', () => {
    expect(projectLabel('/home/user/repo/')).toBe('repo');
  });
  it('is the root path itself when cwd is root', () => {
    expect(projectLabel('/')).toBe('/');
  });
  it('is the basename for a nested path', () => {
    expect(projectLabel('/home/user/repo/backend')).toBe('backend');
  });
  it('has a fixed label when there is no cwd', () => {
    expect(projectLabel(undefined)).toBe('sem projeto');
    expect(projectLabel(null)).toBe('sem projeto');
  });
});

describe('encodeCompareIds / parseCompareIds', () => {
  it('round-trips a normal list', () => {
    const ids = [{ serverId: 's1', name: 'work' }, { serverId: 's2', name: 'home' }];
    expect(parseCompareIds(encodeCompareIds(ids))).toEqual(ids);
  });

  it('escapes literal separators inside ids/names so they never collide with , or :', () => {
    const ids = [{ serverId: 'a:b', name: 'x,y' }, { serverId: 'c,d', name: 'e:f' }];
    const encoded = encodeCompareIds(ids);
    expect(encoded).not.toMatch(/a:b|x,y|c,d|e:f/); // valores crus não sobrevivem ao encode
    expect(parseCompareIds(encoded)).toEqual(ids);
  });

  it('parses an empty param as an empty list', () => {
    expect(parseCompareIds('')).toEqual([]);
  });

  it('drops malformed pairs (no colon, or missing side)', () => {
    expect(parseCompareIds('noColonHere')).toEqual([]);
    expect(parseCompareIds(':nome')).toEqual([]); // serverId vazio
    expect(parseCompareIds('srv:')).toEqual([]); // nome vazio
  });

  it('keeps well-formed pairs alongside malformed ones', () => {
    const encoded = `${encodeURIComponent('s1')}:${encodeURIComponent('a')},garbage,${encodeURIComponent('s2')}:${encodeURIComponent('b')}`;
    expect(parseCompareIds(encoded)).toEqual([{ serverId: 's1', name: 'a' }, { serverId: 's2', name: 'b' }]);
  });
});

describe('latestAssistantEvent', () => {
  const asst = (id: string, text: string): ChatEvent => ({ kind: 'assistant_msg', id, text });
  const userMsg = (id: string, text: string): ChatEvent => ({ kind: 'user_msg', id, text });

  it('returns the last assistant_msg with text', () => {
    const events = [asst('1', 'oi'), userMsg('2', 'e ai'), asst('3', 'tudo bem')];
    expect(latestAssistantEvent(events)?.id).toBe('3');
  });

  it('skips assistant_msg entries without text', () => {
    const events = [asst('1', 'primeira'), { kind: 'assistant_msg', id: '2' } as ChatEvent];
    expect(latestAssistantEvent(events)?.id).toBe('1');
  });

  it('returns null when there is no assistant_msg', () => {
    expect(latestAssistantEvent([userMsg('1', 'oi')])).toBeNull();
  });

  it('returns null for an empty list', () => {
    expect(latestAssistantEvent([])).toBeNull();
  });
});
