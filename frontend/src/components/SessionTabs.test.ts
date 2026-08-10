// @vitest-environment happy-dom
// Correção 3 (round 7): no Board/Canvas o `forced` da sidebarPin segura a sidebar recolhida; o
// botão expandir das abas não pode virar clique morto que grava a preferência por baixo — com
// override ativo ele fica desabilitado (tooltip explica) e a preferência só muda quando o
// usuário decide de verdade (sem override).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import SessionTabs from './SessionTabs.svelte';
import { sidebarPin } from '../lib/sidebarPin.svelte';
import { sidebarBridge } from '../lib/sidebarBridge';

// Store REATIVO (fixture .svelte.ts): o $derived do SessionTabs re-computa quando o modelo muda —
// necessário pro teste do foco pós-rename esperar o reflexo do SSE. Mutar via fixtureByServer.
vi.mock('../lib/sessionsStore.svelte', async () => ({
  sessionsStore: (await import('./sessionTabs.test-store.svelte')).fixtureStore,
}));
import { fixtureByServer } from './sessionTabs.test-store.svelte';
vi.mock('../lib/format', () => ({
  stateColors: {}, stateLabels: {}, sortSessions: (s: unknown[]) => s,
}));
vi.mock('../lib/plan', () => ({ planBadge: vi.fn() }));
import { planBadge } from '../lib/plan';

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(SessionTabs, {
    target: el,
    props: { currentKey: 'srv-a::sess-1', onSelect: vi.fn(), onOpenConfig: vi.fn() },
  });
  return { el, comp: comp as never };
}

function comSessao(nome: string) {
  fixtureByServer.length = 0;
  fixtureByServer.push({
    server: { id: 'srv-a', label: 'A' },
    sessions: [{ name: nome, serverId: 'srv-a', state: 'idle' }],
    error: null, loaded: true,
  });
}

beforeEach(() => {
  sidebarPin.setForced(null);
  sidebarPin.setUser(false);
  vi.mocked(planBadge).mockReturnValue(null);   // sem plano por padrão (filete ausente)
  comSessao('sess-1');
});

describe('SessionTabs — expandir sob override do Board/Canvas (round 7)', () => {
  it('forced=true: botão desabilitado com tooltip e clique NÃO altera a preferência', async () => {
    sidebarPin.setUser(true);   // preferência do usuário: recolhida
    sidebarPin.setForced(true); // board/canvas segurando o recolhido
    const t = montar();
    await tick();
    const btn = document.querySelector<HTMLButtonElement>('.tab-expand')!;
    expect(btn.disabled).toBe(true);
    expect(btn.title).not.toBe('');   // explica o porquê do bloqueio
    btn.click();
    await tick();
    expect(sidebarPin.preferred).toBe(true);   // nada gravado por baixo
    unmount(t.comp);
  });

  it('forced=null com preferência recolhida: botão habilitado e clique expande de verdade', async () => {
    sidebarPin.setUser(true);   // recolhida por PREFERÊNCIA (não por override)
    const t = montar();
    await tick();
    const btn = document.querySelector<HTMLButtonElement>('.tab-expand')!;
    expect(btn.disabled).toBe(false);
    btn.click();
    await tick();
    expect(sidebarPin.preferred).toBe(false);   // expandiu: preferência mudou de propósito
    unmount(t.comp);
  });
});

describe('SessionTabs — filete de progresso do plano (round 7)', () => {
  function comPlano(pct: number, complete: boolean) {
    vi.mocked(planBadge).mockReturnValue({
      pct, complete, label: '', title: `Plano ${pct}%`,
    });
  }

  it('0%: filete presente com largura zero (nada a mostrar, sem quebrar)', async () => {
    comPlano(0, false);
    const t = montar();
    await tick();
    const filete = document.querySelector<HTMLElement>('.tab-plan');
    expect(filete).not.toBeNull();
    expect(filete?.style.width).toBe('0%');
    expect(filete?.classList.contains('done')).toBe(false);
    unmount(t.comp);
  });

  it('parcial: largura proporcional ao progresso, sem marcação de concluído', async () => {
    comPlano(37, false);
    const t = montar();
    await tick();
    const filete = document.querySelector<HTMLElement>('.tab-plan');
    expect(filete?.style.width).toBe('37%');
    expect(filete?.classList.contains('done')).toBe(false);
    unmount(t.comp);
  });

  it('concluído: largura cheia e classe done (cor de sucesso no CSS)', async () => {
    comPlano(100, true);
    const t = montar();
    await tick();
    const filete = document.querySelector<HTMLElement>('.tab-plan');
    expect(filete?.style.width).toBe('100%');
    expect(filete?.classList.contains('done')).toBe(true);
    unmount(t.comp);
  });
});

describe('SessionTabs — foco pós-rename via bridge (round 2)', () => {
  const aba = () => document.querySelector<HTMLButtonElement>('.tab');

  it('focusTab com chave presente foca a aba imediatamente', async () => {
    const t = montar();
    await tick();
    sidebarBridge.focusTab('srv-a::sess-1');
    await tick();
    expect(document.activeElement).toBe(aba());
    unmount(t.comp);
  });

  it('focusTab com chave ainda ausente espera o modelo refletir e foca a aba recriada (conectada)', async () => {
    const t = montar();
    await tick();
    // O SSE ainda não refletiu o rename: a chave nova não existe no modelo
    sidebarBridge.focusTab('srv-a::sess-novo');
    await tick();
    expect(document.activeElement).not.toBe(aba());
    // Modelo reflete o novo nome: a aba antiga (keyed por nome) é substituída pela recriada
    comSessao('sess-novo');
    await tick(); await tick();
    const novaAba = aba();
    expect(novaAba?.isConnected).toBe(true);
    expect(document.activeElement).toBe(novaAba);
    unmount(t.comp);
  });
});
