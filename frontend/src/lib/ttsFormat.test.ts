import { describe, it, expect } from 'vitest';
import { formatClock } from './ttsFormat';

describe('formatClock', () => {
  it('formata segundos como m:ss', () => {
    expect(formatClock(0)).toBe('0:00');
    expect(formatClock(9)).toBe('0:09');
    expect(formatClock(75)).toBe('1:15');
    expect(formatClock(600)).toBe('10:00');
  });

  it('nao quebra com valor invalido do elemento de audio', () => {
    // duration de um <audio> ainda sem metadados vem NaN, e Infinity em stream sem tamanho.
    expect(formatClock(NaN)).toBe('0:00');
    expect(formatClock(Infinity)).toBe('0:00');
    expect(formatClock(-3)).toBe('0:00');
  });
});
