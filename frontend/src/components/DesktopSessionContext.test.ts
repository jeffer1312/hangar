// @vitest-environment happy-dom
// Follow-up visual: com toggle externo (barra/rail), o DesktopSessionContext NÃO
// pode ter botão duplicado (.ctx-fold) nem aba vertical central quando recolhido — o painel
// simplesmente some. Sem toggle externo (sidebar expandida), a porta acessível do painel continua.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import DesktopSessionContext from './DesktopSessionContext.svelte';
import { ctxPanel } from '../lib/ctxPanel.svelte';

// Stubs dos componentes internos pesados (PlanRing/PlanPanel renderizam SVG/estado de plano).
vi.mock('./PlanRing.svelte', () => ({ default: class { $destroy() {} } }));
vi.mock('./PlanPanel.svelte', () => ({ default: class { $destroy() {} } }));

function montar(toggleExterno: boolean) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(DesktopSessionContext, {
    target: el,
    props: {
      state: 'idle',
      sessionName: 'sess-1',
      toggleExterno,
    },
  });
  return { el, comp: comp as never };
}

beforeEach(() => {
  ctxPanel.recolhido = false;
  document.body.innerHTML = '';
});

describe('DesktopSessionContext — toggle na barra (follow-up visual)', () => {
  it('toggleExterno: NENHUM .ctx-fold (nem no aberto) — sem botão duplicado', async () => {
    const t = montar(true);
    await tick();
    expect(document.querySelector('.ctx-fold')).toBeNull();
    unmount(t.comp);
  });

  it('toggleExterno + recolhido: painel some (sem aba vertical central)', async () => {
    const t = montar(true);
    await tick();
    ctxPanel.recolhido = true;
    await tick();
    const aside = document.querySelector<HTMLElement>('.session-context');
    expect(aside?.classList.contains('recolhido')).toBe(true);
    expect(aside?.classList.contains('toggle-externo')).toBe(true);
    // nenhuma aba vertical: não existe .ctx-fold (o display:none da regra recolhido+toggle-externo
    // é validado no browser — happy-dom não injeta o CSS escopado)
    expect(document.querySelector('.ctx-fold')).toBeNull();
    unmount(t.comp);
  });

  it('sem toggleExterno (sidebar expandida): porta acessível PRESERVADA — .ctx-fold existe e recolhido mantém a aba', async () => {
    const t = montar(false);
    await tick();
    const fold = document.querySelector<HTMLButtonElement>('.ctx-fold');
    expect(fold).not.toBeNull();
    expect(fold?.getAttribute('aria-label')).toBe('Recolher contexto');
    ctxPanel.recolhido = true;
    await tick();
    expect(document.querySelector('.ctx-fold')).not.toBeNull();   // aba continua clicável
    expect(getComputedStyle(document.querySelector<HTMLElement>('.session-context')!).display).not.toBe('none');
    unmount(t.comp);
  });
});
