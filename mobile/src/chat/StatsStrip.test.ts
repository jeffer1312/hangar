import { describe, it, expect, beforeAll } from 'vitest';
import { overwriteGetLocale } from '../paraglide/runtime';
import { linhaStats } from './StatsStrip';

// O baseLocale do projeto é `en`; os rótulos esperados aqui são os de pt-BR.
beforeAll(() => overwriteGetLocale(() => 'pt'));

describe('linhaStats', () => {
  it('monta as partes na ordem da PWA e omite o que não veio', () => {
    expect(linhaStats({ turns: 1, steps: 3, in_tok: 1200, out_tok: 300 })).toEqual(['1 turno', '3 chamadas', '↓ 1.2K · ↑ 300 tok']);
    expect(linhaStats({ turns: 2, steps: 1, in_tok: 0, out_tok: 0, tok_s: 41.6, cache_pct: 80 })).toContain('~42 tok/s');
    expect(linhaStats({ turns: 2, steps: 1, in_tok: 0, out_tok: 0, cache_pct: 80 })).toContain('cache 80%');
  });
});
