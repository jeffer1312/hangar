import { describe, it, expect } from 'vitest';
import { comAcento, hexParaRgb } from './corTema';
import { dark } from './theme';

describe('hexParaRgb', () => {
  it('lê #rrggbb', () => { expect(hexParaRgb('#ff8000')).toEqual([255, 128, 0]); });
  it('recusa o que não é hex de 6 dígitos', () => {
    expect(hexParaRgb('azul')).toBeNull();
    expect(hexParaRgb('#fff')).toBeNull();
  });
});

describe('comAcento', () => {
  it('null devolve os tokens originais', () => { expect(comAcento(dark, null)).toBe(dark); });
  it('hex troca base, dim (18% alpha) e press (mais escuro)', () => {
    const t = comAcento(dark, '#ff0000');
    expect(t.accent.base).toBe('#ff0000');
    expect(t.accent.dim).toBe('rgba(255,0,0,0.18)');
    expect(t.accent.press).toMatch(/^#[0-9a-f]{6}$/);
    expect(t.accent.press).not.toBe('#ff0000');
    expect(t.pill.working.fg).not.toBe(dark.pill.working.fg);
  });
  it('hex inválido é ignorado', () => { expect(comAcento(dark, 'azul')).toBe(dark); });
});
