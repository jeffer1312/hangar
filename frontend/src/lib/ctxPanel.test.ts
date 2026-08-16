// @vitest-environment happy-dom
// Task 17: largura do painel de contexto redimensionável — guarda ao arrastar, restaura ao
// remontar, respeita mínimo/máximo e o recolhido continua indo e voltando sem mexer na largura.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { LARGURA_MIN, LARGURA_MAX, RESERVA_VISOR, RESERVA_NAV } from './ctxPanel.svelte';

// Janela determinística: happy-dom nasce em 1024 e o teto pela janela depende dela. Com 1600,
// teto = min(LARGURA_MAX, 1600 - RESERVA_VISOR) — e o default crescido (>=1600 → 300) também fica
// testável.
function fixarJanela(largura: number) {
  Object.defineProperty(window, 'innerWidth', { value: largura, configurable: true });
}

// O estado é singleton $state carregado no import do módulo — "restaura ao remontar" exige um
// módulo fresco, como o App faz ao recarregar a página.
async function importarFresco() {
  vi.resetModules();
  return await import('./ctxPanel.svelte');
}

beforeEach(() => {
  localStorage.clear();
  fixarJanela(1600);
});

describe('ctxPanel — largura redimensionável (task 17)', () => {
  it('default cresce com a tela quando nada foi salvo (>=1600 → 300)', async () => {
    const mod = await importarFresco();
    expect(mod.ctxPanel.largura).toBe(300);
  });

  it('arrastar atualiza a largura', async () => {
    const mod = await importarFresco();
    // janela 1600, arrastou a divisória até clientX=1100 -> painel = 1600-1100 = 500
    mod.arrastarLargura(1100);
    expect(mod.ctxPanel.largura).toBe(500);
  });

  it('respeita o máximo', async () => {
    const mod = await importarFresco();
    mod.arrastarLargura(0); // painel = janela inteira -> clampa no teto
    expect(mod.ctxPanel.largura).toBe(Math.min(LARGURA_MAX, 1600 - RESERVA_VISOR - RESERVA_NAV));
    expect(mod.ctxPanel.largura).toBe(LARGURA_MAX);
  });

  it('respeita o mínimo', async () => {
    const mod = await importarFresco();
    mod.arrastarLargura(9999); // painel negativo -> clampa no piso
    expect(mod.ctxPanel.largura).toBe(LARGURA_MIN);
  });

  it('guardar persiste e o módulo fresco restaura o valor salvo', async () => {
    const mod = await importarFresco();
    mod.arrastarLargura(1100); // 500
    mod.salvarLargura();
    expect(localStorage.getItem('cp_ctx_w')).toBe('500');

    const fresco = await importarFresco();
    expect(fresco.ctxPanel.largura).toBe(500);
  });

  it('largura salva quebrada na tela menor é limitada pela janela na aplicação', async () => {
    const mod = await importarFresco();
    mod.arrastarLargura(0); // teto (LARGURA_MAX) numa janela de 1600
    mod.salvarLargura();

    fixarJanela(1280); // abriu numa tela menor — o painel só existe >=1280, este é o caso real
    const fresco = await importarFresco();
    expect(fresco.ctxPanel.largura).toBe(1280 - RESERVA_VISOR - RESERVA_NAV);
  });

  it('reclamparLargura reaplica o teto quando a janela muda, sem tocar no salvo (bloqueador 2)', async () => {
    const mod = await importarFresco();
    fixarJanela(1600);
    mod.arrastarLargura(0); // teto de 1600
    mod.salvarLargura();
    const salvo = localStorage.getItem('cp_ctx_w');

    fixarJanela(1280); // encolheu a janela SEM recarregar
    mod.reclamparLargura();
    expect(mod.ctxPanel.largura).toBe(1280 - RESERVA_VISOR - RESERVA_NAV);
    expect(localStorage.getItem('cp_ctx_w')).toBe(salvo); // escolha grande preservada

    fixarJanela(1600); // monitor grande voltou: a escolha restaura do salvo
    mod.reclamparLargura();
    expect(mod.ctxPanel.largura).toBe(Number(salvo));
  });

  it('recolher/expandir continua indo e voltando sem tocar na largura', async () => {
    const mod = await importarFresco();
    mod.arrastarLargura(1100); // 500
    mod.alternarCtxPanel();
    expect(mod.ctxPanel.recolhido).toBe(true);
    expect(mod.ctxPanel.largura).toBe(500); // recolher não zera nem vira trilho
    mod.alternarCtxPanel();
    expect(mod.ctxPanel.recolhido).toBe(false);
    expect(mod.ctxPanel.largura).toBe(500);
  });
});
