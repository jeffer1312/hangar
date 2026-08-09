// @vitest-environment happy-dom
// Round 4: restauração de foco SEGURA — sheet desmontada estando ABERTA devolve o foco ao gatilho;
// gatilho oculto (aria-hidden) cai no fallbackFocus explícito.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import BottomSheet from './BottomSheet.svelte';

function montar(props: Record<string, unknown> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(BottomSheet, {
    target: el,
    props: {
      open: true,
      onClose: vi.fn(),
      ariaLabel: 'Folha',
      children: () => '<p>conteúdo</p>' as never,
      ...props,
    },
  });
  return { el, comp: comp as never };
}

describe('BottomSheet — restauração de foco', () => {
  it('unmount estando aberta devolve o foco ao gatilho', async () => {
    const gatilho = document.createElement('button');
    document.body.appendChild(gatilho);
    gatilho.focus();
    const t = montar();
    await tick();
    unmount(t.comp);
    expect(document.activeElement).toBe(gatilho);
    gatilho.remove();
  });

  it('gatilho oculto (aria-hidden) cai no fallbackFocus', async () => {
    const wrapper = document.createElement('div');
    wrapper.setAttribute('aria-hidden', 'true');
    const gatilho = document.createElement('button');
    wrapper.appendChild(gatilho);
    document.body.appendChild(wrapper);
    gatilho.focus();
    const fallback = document.createElement('button');
    document.body.appendChild(fallback);
    const t = montar({ fallbackFocus: fallback });
    await tick();
    unmount(t.comp);
    expect(document.activeElement).toBe(fallback);
    wrapper.remove();
    fallback.remove();
  });
});
