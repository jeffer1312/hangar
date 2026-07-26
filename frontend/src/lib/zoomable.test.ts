import { describe, it, expect } from 'vitest';
import { clampScale, clampPan, panParaAncorar, MAX_SCALE } from './zoomable';

describe('clampScale', () => {
  it('nunca fica menor que a imagem inteira', () => {
    expect(clampScale(0.2)).toBe(1);
    expect(clampScale(-3)).toBe(1);
  });

  it('respeita o teto', () => {
    expect(clampScale(999)).toBe(MAX_SCALE);
    expect(clampScale(2.5)).toBe(2.5);
  });
});

describe('clampPan', () => {
  it('trava no centro quando a imagem cabe inteira', () => {
    expect(clampPan(120, -80, 1, 400, 300)).toEqual({ x: 0, y: 0 });
  });

  it('deixa arrastar só a metade que sobra', () => {
    // 2x numa imagem de 400px: sobra 400px, 200 pra cada lado.
    expect(clampPan(1000, 0, 2, 400, 300)).toEqual({ x: 200, y: 0 });
    expect(clampPan(-1000, 0, 2, 400, 300)).toEqual({ x: -200, y: 0 });
    expect(clampPan(50, 20, 2, 400, 300)).toEqual({ x: 50, y: 20 });
  });
});

describe('panParaAncorar', () => {
  it('mantém o ponto sob o dedo parado ao ampliar', () => {
    // Ponto 100px à direita do centro, sem deslocamento, indo de 1x pra 2x.
    const t = panParaAncorar({ x: 100, y: 0 }, { x: 0, y: 0 }, 1, 2);
    // Depois de dobrar, aquele ponto estaria em 200 — o deslocamento tem que puxar 100 de volta.
    expect(t).toEqual({ x: -100, y: 0 });
  });

  it('o centro exato não se move', () => {
    expect(panParaAncorar({ x: 0, y: 0 }, { x: 0, y: 0 }, 1, 3)).toEqual({ x: 0, y: 0 });
  });

  it('reduzir desfaz o deslocamento na mesma proporção', () => {
    const ampliado = panParaAncorar({ x: 100, y: 40 }, { x: 0, y: 0 }, 1, 2);
    const devolta = panParaAncorar({ x: 100, y: 40 }, ampliado, 2, 1);
    expect(devolta.x).toBeCloseTo(0);
    expect(devolta.y).toBeCloseTo(0);
  });
});
