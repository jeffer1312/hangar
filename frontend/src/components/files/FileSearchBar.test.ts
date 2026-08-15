// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import FileSearchBar, { type Props } from './FileSearchBar.svelte';
import { overwriteGetLocale } from '../../paraglide/runtime';

function montar(props: Props) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return { el, comp: mount(FileSearchBar, { target: el, props }) };
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
});
