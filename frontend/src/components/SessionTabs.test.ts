// @vitest-environment happy-dom
// Correção 3 (round 7): no Board/Canvas o `forced` da sidebarPin segura a sidebar recolhida; o
// botão expandir das abas não pode virar clique morto que grava a preferência por baixo — com
// override ativo ele fica desabilitado (tooltip explica) e a preferência só muda quando o
// usuário decide de verdade (sem override).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import SessionTabs from './SessionTabs.svelte';
import { sidebarPin } from '../lib/sidebarPin.svelte';

vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: {
    byServer: [{
      server: { id: 'srv-a', label: 'A' },
      sessions: [{ name: 'sess-1', serverId: 'srv-a', state: 'idle' }],
      error: null, loaded: true,
    }],
  },
}));
vi.mock('../lib/format', () => ({
  stateColors: {}, stateLabels: {}, sortSessions: (s: unknown[]) => s,
}));
vi.mock('../lib/plan', () => ({ planBadge: () => null }));

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(SessionTabs, {
    target: el,
    props: { currentKey: 'srv-a::sess-1', onSelect: vi.fn(), onOpenConfig: vi.fn() },
  });
  return { el, comp: comp as never };
}

beforeEach(() => {
  sidebarPin.setForced(null);
  sidebarPin.setUser(false);
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
