import { describe, it, expect } from 'vitest';
import { abbrevNum, countAwaiting } from './format';

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
