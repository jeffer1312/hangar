// @vitest-environment happy-dom
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import LinhaConfig from './LinhaConfig.svelte';
import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';

function store(campos: Record<string, unknown>, valor: unknown = '') {
  return {
    get campos() { return campos; },
    valorAtual: () => valor,
    rascunhoDe: () => '',
    setRascunho: vi.fn(),
  } as unknown as ConfigServidorStore;
}

describe('LinhaConfig', () => {
  it('segredo já definido mostra a máscara e o campo vazio', () => {
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(LinhaConfig, { target: alvo, props: {
      campo: { chave: 'k', rotulo: 'Chave', ajuda: 'ajuda', tipo: 'segredo' },
      store: store({ k: { definido: true, valor: 'sk-•••1234' } }),
    } });
    expect(alvo.textContent).toContain('sk-•••1234');
    expect(alvo.querySelector<HTMLInputElement>('input[type="text"]')!.value).toBe('');
    unmount(app);
  });

  it('interruptor reflete o valor atual', () => {
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(LinhaConfig, { target: alvo, props: {
      campo: { chave: 'a', rotulo: 'Automações', ajuda: 'ajuda', tipo: 'liga' },
      store: store({ a: {} }, true),
    } });
    expect(alvo.querySelector<HTMLInputElement>('input[type="checkbox"]')!.checked).toBe(true);
    unmount(app);
  });
});
