// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import FileSearchBar, { type Props } from './FileSearchBar.svelte';
import { overwriteGetLocale } from '../../paraglide/runtime';
// Harness: segura q em estado para re-render com prop nova ($set e bloqueado em DEV).
import Harness from './FileSearchBar.harness.svelte';

function montar(props: Props) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return { el, comp: mount(FileSearchBar, { target: el, props }) };
}

function montarHarness(props: Props) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return { el, comp: mount(Harness, { target: el, props }) };
}

describe('FileSearchBar', () => {
  beforeEach(() => overwriteGetLocale(() => 'pt'));
  afterEach(() => vi.useRealTimers());

  it('espera 250ms antes de avisar', async () => {
    vi.useFakeTimers();
    const onBusca = vi.fn();
    const { el, comp } = montar({ q: '', mode: 'names', onBusca });
    const campo = el.querySelector('input') as HTMLInputElement;
    campo.value = 'abc';
    campo.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    expect(onBusca).not.toHaveBeenCalled();
    vi.advanceTimersByTime(260);
    expect(onBusca).toHaveBeenCalledWith('abc', 'names');
    unmount(comp);
  });

  it('trocar de aba refaz a busca sem limpar o campo', async () => {
    const onBusca = vi.fn();
    const { el, comp } = montar({ q: 'abc', mode: 'names', onBusca });
    const abas = el.querySelectorAll('.seg button');
    abas[1].dispatchEvent(new MouseEvent('click', { bubbles: true }));   // "Conteúdo"
    await tick();
    expect(onBusca).toHaveBeenCalledWith('abc', 'contents');
    expect((el.querySelector('input') as HTMLInputElement).value).toBe('abc');
    unmount(comp);
  });

  it('trocar de aba usa o termo digitado ainda pendente, nao o q antigo', async () => {
    vi.useFakeTimers();
    const onBusca = vi.fn();
    const { el, comp } = montarHarness({ q: 'velho', mode: 'names', onBusca });
    const campo = el.querySelector('input') as HTMLInputElement;
    campo.value = 'novo';
    campo.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    const abas = el.querySelectorAll('.seg button');
    abas[1].dispatchEvent(new MouseEvent('click', { bubbles: true }));   // "Conteúdo"
    await tick();
    expect(onBusca).toHaveBeenCalledWith('novo', 'contents');
    // Host confirma o termo na prop: o campo continua com o que o usuario ve.
    (el.querySelector('.h-ecoar') as HTMLButtonElement).click();
    await tick();
    expect((el.querySelector('input') as HTMLInputElement).value).toBe('novo');
    unmount(comp);
  });

  it('mudanca externa de q cancela o debounce pendente', async () => {
    vi.useFakeTimers();
    const onBusca = vi.fn();
    const { el, comp } = montarHarness({ q: '', mode: 'names', onBusca });
    const campo = el.querySelector('input') as HTMLInputElement;
    campo.value = 'ab';
    campo.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    (el.querySelector('.h-ecoar') as HTMLButtonElement).click();   // troca de sessao, limpeza
    await tick();
    vi.advanceTimersByTime(260);
    expect(onBusca).not.toHaveBeenCalled();
    unmount(comp);
  });

  it('os tres controles ficam na ordem natural do tab', async () => {
    const { el, comp } = montar({ q: '', mode: 'names', onBusca: vi.fn() });
    const controles = el.querySelectorAll('input, .seg button');
    expect(controles.length).toBe(3);
    for (const c of controles) {
      (c as HTMLElement).focus();
      expect(document.activeElement).toBe(c);
    }
    unmount(comp);
  });
});
