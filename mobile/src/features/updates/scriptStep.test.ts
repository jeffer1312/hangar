import { describe, expect, it } from 'vitest';
import { nextStep } from './scriptStep';

describe('nextStep', () => {
  const steps = [0.5, 1.4, 2.8];

  it('vai ao próximo degrau à frente, nunca ao atual', () => {
    expect(nextStep(steps, 6, 0)).toBe(0.5);
    expect(nextStep(steps, 6, 0.5)).toBe(1.4);
    expect(nextStep(steps, 6, 2)).toBe(2.8);
  });

  it('depois do último degrau espera o fim do ciclo', () => {
    expect(nextStep(steps, 6, 2.8)).toBe(6);
    expect(nextStep([], 6, 1)).toBe(6);
  });
});
