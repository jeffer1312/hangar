// @vitest-environment happy-dom
// Focal do PlanRing: pct=NaN não pode vazar "NaN" pra texto/ARIA/stroke; clamp preserva negativos/
// >100 e ±Infinity é determinístico (Infinity -> 100, -Infinity -> 0).
import { describe, it, expect } from 'vitest';
import { mount, unmount } from 'svelte';
import PlanRing from './PlanRing.svelte';

const C = 2 * Math.PI * 9;

function renderizar(pct: number) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(PlanRing, { target: el, props: { pct } });
  const ring = el.querySelector('.ring') as HTMLElement;
  const arc = el.querySelector('.arc') as SVGElement;
  const texto = el.querySelector('text')?.textContent ?? '';
  const offset = arc?.getAttribute('stroke-dashoffset') ?? '';
  return {
    el, comp, ring, arc, texto,
    now: ring.getAttribute('aria-valuenow'),
    offsetNum: Number.parseFloat(offset),
    html: el.innerHTML,
  };
}

describe('PlanRing', () => {
  it('NaN não gera NaN em texto/ARIA/stroke; cai em 0 (anel vazio)', () => {
    const t = renderizar(Number.NaN);
    expect(t.texto).toBe('0');
    expect(t.now).toBe('0');
    expect(t.offsetNum).toBeCloseTo(C, 5);   // vazio = arco todo desenhado por cima
    expect(t.html).not.toContain('NaN');
    unmount(t.comp as never);
  });

  it('clamp preserva negativos e >100', () => {
    const neg = renderizar(-5);
    expect(neg.texto).toBe('0');
    expect(neg.now).toBe('0');
    const alto = renderizar(150);
    expect(alto.texto).toBe('100');
    expect(alto.now).toBe('100');
    expect(alto.offsetNum).toBeCloseTo(0, 5);   // cheio = arco zerado
    unmount(neg.comp as never);
    unmount(alto.comp as never);
  });

  it('±Infinity é determinístico: +Inf -> 100, -Inf -> 0, sem NaN', () => {
    const pos = renderizar(Number.POSITIVE_INFINITY);
    expect(pos.texto).toBe('100');
    expect(pos.html).not.toContain('NaN');
    const neg = renderizar(Number.NEGATIVE_INFINITY);
    expect(neg.texto).toBe('0');
    expect(neg.html).not.toContain('NaN');
    unmount(pos.comp as never);
    unmount(neg.comp as never);
  });

  it('valores normais e bordas exatas passam retos', () => {
    const meio = renderizar(50);
    expect(meio.texto).toBe('50');
    expect(meio.now).toBe('50');
    expect(meio.offsetNum).toBeCloseTo(C / 2, 5);
    const zero = renderizar(0);
    expect(zero.texto).toBe('0');
    const cem = renderizar(100);
    expect(cem.texto).toBe('100');
    unmount(meio.comp as never);
    unmount(zero.comp as never);
    unmount(cem.comp as never);
  });
});
