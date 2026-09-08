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

  it('abertos NASCE com o que está no localStorage (a carga não pode depender de const declarada depois)', async () => {
    localStorage.setItem('cp_nav_abertos', JSON.stringify({ 'srv-x::hangar': 'http://localhost:3000/' }));
    const mod = await importarFresco();
    expect(mod.navegadorPanel.abertos['srv-x::hangar']).toBe('http://localhost:3000/');
  });

  it('marcar/fechar persiste o conjunto de abertos', async () => {
    const mod = await importarFresco();
    mod.marcarNavAberto('srv-x::hangar');
    expect(JSON.parse(localStorage.getItem('cp_nav_abertos')!)).toHaveProperty('srv-x::hangar');
    mod.fecharNav('srv-x::hangar');
    expect(JSON.parse(localStorage.getItem('cp_nav_abertos')!)).not.toHaveProperty('srv-x::hangar');
  });

  it('sessão que sumiu da lista leva o navegador junto; servidor sem lista lida não decide', async () => {
    const close = vi.fn();
    (window as unknown as { hangar?: unknown }).hangar = { nav: { close } };
    localStorage.setItem('cp_nav_abertos', JSON.stringify({
      'srv-x::morta': 'http://localhost:8888/test-form.html',
      'srv-x::viva': 'http://localhost:3000/',
      'srv-off::qualquer': 'http://localhost:4000/',
    }));
    const mod = await importarFresco();
    mod.podarNavMortos(new Map([['srv-x', new Map([['viva', '/t/a.jsonl']])]]));
    expect(Object.keys(mod.navegadorPanel.abertos).sort()).toEqual(['srv-off::qualquer', 'srv-x::viva']);
    expect(JSON.parse(localStorage.getItem('cp_nav_abertos')!)).not.toHaveProperty('srv-x::morta');
    expect(close).toHaveBeenCalledWith('srv-x::morta');
    expect(close).toHaveBeenCalledTimes(1);
    delete (window as unknown as { hangar?: unknown }).hangar;
  });

  it('mesmo nome com outro transcript é sessão nova: a marca da morta cai, mesmo com o app fechado no meio', async () => {
    const close = vi.fn();
    (window as unknown as { hangar?: unknown }).hangar = { nav: { close } };
    localStorage.setItem('cp_nav_abertos', JSON.stringify({ 'srv-x::hangar-4': 'http://localhost:8888/test-form.html' }));
    let mod = await importarFresco();
    // 1ª lista com a dona viva: adota o transcript dela e persiste (é o que sobrevive ao app fechar).
    mod.podarNavMortos(new Map([['srv-x', new Map([['hangar-4', '/t/velha.jsonl']])]]));
    expect(JSON.parse(localStorage.getItem('cp_nav_donos')!)).toEqual({ 'srv-x::hangar-4': '/t/velha.jsonl' });
    // app fechou, sessão morreu, outra nasceu com o mesmo nome; app reabre (módulo fresco).
    mod = await importarFresco();
    mod.podarNavMortos(new Map([['srv-x', new Map([['hangar-4', '/t/nova.jsonl']])]]));
    expect(mod.navegadorPanel.abertos).toEqual({});
    expect(JSON.parse(localStorage.getItem('cp_nav_donos')!)).toEqual({});
    expect(close).toHaveBeenCalledWith('srv-x::hangar-4');
    // `/clear` com o app ABERTO: a sessão nunca saiu da lista, só trocou de transcript — o
    // navegador fica e a dona acompanha.
    mod.marcarNavAberto('srv-x::viva');
    mod.podarNavMortos(new Map([['srv-x', new Map([['viva', '/t/antes.jsonl']])]]));
    mod.podarNavMortos(new Map([['srv-x', new Map([['viva', '/t/pos-clear.jsonl']])]]));
    expect(mod.navegadorPanel.abertos).toHaveProperty('srv-x::viva');
    expect(JSON.parse(localStorage.getItem('cp_nav_donos')!)).toEqual({ 'srv-x::viva': '/t/pos-clear.jsonl' });
    expect(close).toHaveBeenCalledTimes(1);
    mod.fecharNav('srv-x::viva');
    // sessão ainda sem transcript (Codex nascendo) não condena nem adota.
    mod.marcarNavAberto('srv-x::cx');
    mod.podarNavMortos(new Map([['srv-x', new Map([['cx', null]])]]));
    expect(mod.navegadorPanel.abertos).toHaveProperty('srv-x::cx');
    expect(JSON.parse(localStorage.getItem('cp_nav_donos')!)).toEqual({});
    delete (window as unknown as { hangar?: unknown }).hangar;
  });
});
