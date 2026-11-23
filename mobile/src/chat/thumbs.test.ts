import { describe, expect, it } from 'vitest';
import { thumbWidth, thumbWidthFor } from './thumbs';

describe('largura da miniatura', () => {
  it('é a caixa vezes a densidade da tela, dentro do que o servidor aceita', () => {
    expect(thumbWidth(96, 3)).toBe(288);
    expect(thumbWidth(96, 2.625)).toBe(252);
    expect(thumbWidth(600, 3)).toBe(1024);
    expect(thumbWidth(4, 1)).toBe(16);
  });

  it('só pede miniatura do que o servidor reduz', () => {
    expect(thumbWidthFor('a.PNG')).toBe(288);
    expect(thumbWidthFor('foto.jpeg', 140)).toBe(420);
    expect(thumbWidthFor('anim.gif')).toBeUndefined();
    expect(thumbWidthFor('logo.svg')).toBeUndefined();
    expect(thumbWidthFor('clip.mp4')).toBeUndefined();
  });
});
