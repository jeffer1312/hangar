// @vitest-environment happy-dom
// formatarIntervalo importa lib/auth no module scope (getBaseUrl) — precisa de localStorage.
import { describe, expect, it } from 'vitest';
import { formatarIntervalo } from './contaEstado';

describe('formatarIntervalo', () => {
  it('formata minutos, horas e dias como dado curto (mock: "última leitura há 2 h")', () => {
    expect(formatarIntervalo(30)).toBe('1 min');
    expect(formatarIntervalo(150)).toBe('2 min');
    expect(formatarIntervalo(3600 * 2 + 60)).toBe('2 h');
    expect(formatarIntervalo(86400 * 3)).toBe('3 d');
  });

  it('não estoura em leitura inexistente', () => {
    expect(formatarIntervalo(null)).toBe('—');
    expect(formatarIntervalo(Number.NaN)).toBe('—');
  });
});