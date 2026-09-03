// @vitest-environment happy-dom
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { NAV_MIN } from './navegadorPanel.svelte';

// Janela determinística (mesmo padrão do ctxPanel.test): o teto é função da largura da janela.
function fixarJanela(largura: number) {
  Object.defineProperty(window, 'innerWidth', { value: largura, configurable: true });
}

// O estado é singleton $state carregado no import — "restaura ao remontar" exige módulo fresco.
async function importarFresco() {
  vi.resetModules();
  return await import('./navegadorPanel.svelte');
}

beforeEach(() => {
  localStorage.clear();
  fixarJanela(1600);
});

describe('navegadorPanel — largura redimensionável', () => {
  it('nasce em ~42% da janela quando nada foi salvo', async () => {
    const mod = await importarFresco();
    expect(mod.navegadorPanel.largura).toBe(Math.round(1600 * 0.42));
  });

  it('arrastar a divisória até clientX=900 dá largura 700', async () => {
    const mod = await importarFresco();
    mod.arrastarNav(900);
    expect(mod.navegadorPanel.largura).toBe(700);
  });

  it('respeita o teto: janela menos chat e trilho da sidebar', async () => {
    const mod = await importarFresco();
    mod.arrastarNav(100);   // pediria 1500, mas 1600-520-52 = 1028
    expect(mod.navegadorPanel.largura).toBe(1028);
  });

  it('respeita o mínimo', async () => {
    const mod = await importarFresco();
    mod.arrastarNav(1599);
    expect(mod.navegadorPanel.largura).toBe(NAV_MIN);
  });

  it('guarda ao soltar e restaura ao remontar', async () => {
    const mod = await importarFresco();
    mod.arrastarNav(900);
    mod.salvarNav();
    const mod2 = await importarFresco();
    expect(mod2.navegadorPanel.largura).toBe(700);
  });
});
