import { describe, it, expect } from 'vitest';
import { abbrevNum, countAwaiting, projectKey, projectLabel } from './format';

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
