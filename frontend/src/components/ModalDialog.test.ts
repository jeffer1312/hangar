// @vitest-environment happy-dom
// Round 4: restauração de foco SEGURA — unmount estando aberto devolve o foco ao gatilho; gatilho
// oculto (aria-hidden) ou removido do DOM cai no fallbackFocus explícito.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ModalDialog from './ModalDialog.svelte';

function montar(props: Record<string, unknown> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ModalDialog, {
    target: el,
    props: {
      open: true,
      ariaLabel: 'Diálogo',
      onClose: vi.fn(),
      children: () => '<p>conteúdo</p>' as never,
      ...props,
    },
  });
  return { el, comp: comp as never };
}

describe('ModalDialog — restauração de foco', () => {
  it('unmount estando aberto devolve o foco ao gatilho', async () => {
    const gatilho = document.createElement('button');
    document.body.appendChild(gatilho);
    gatilho.focus();
    const t = montar();
    await tick(); await tick();
    expect(document.activeElement).not.toBe(gatilho);   // foco foi pra dentro do diálogo
    unmount(t.comp);
    expect(document.activeElement).toBe(gatilho);
    gatilho.remove();
  });

  it('gatilho dentro de aria-hidden cai no fallbackFocus', async () => {
    const wrapper = document.createElement('div');
    wrapper.setAttribute('aria-hidden', 'true');
    const gatilho = document.createElement('button');
    wrapper.appendChild(gatilho);
    document.body.appendChild(wrapper);
    gatilho.focus();
    const fallback = document.createElement('button');
    document.body.appendChild(fallback);
    const t = montar({ fallbackFocus: fallback });
    await tick(); await tick();
    unmount(t.comp);
    expect(document.activeElement).toBe(fallback);
    wrapper.remove();
    fallback.remove();
  });

  it('gatilho removido do DOM antes do fechar cai no fallbackFocus', async () => {
    const gatilho = document.createElement('button');
    document.body.appendChild(gatilho);
    gatilho.focus();
    const fallback = document.createElement('button');
    document.body.appendChild(fallback);
    const t = montar({ fallbackFocus: fallback });
    await tick(); await tick();
    gatilho.remove();
    unmount(t.comp);
    expect(document.activeElement).toBe(fallback);
    fallback.remove();
  });
});
