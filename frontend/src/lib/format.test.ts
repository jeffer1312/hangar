import { describe, it, expect } from 'vitest';
import { abbrevNum } from './format';

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
