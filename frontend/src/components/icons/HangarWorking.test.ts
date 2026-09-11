// @vitest-environment happy-dom
import { mount, tick, unmount } from 'svelte';
import { describe, expect, it, vi } from 'vitest';
import HangarWorking from './HangarWorking.svelte';

describe('HangarWorking', () => {
  it('pausa fora da tela e retoma quando volta', async () => {
    let emitir: IntersectionObserverCallback = () => {};
    const disconnect = vi.fn();
    class Observer {
      constructor(callback: IntersectionObserverCallback) { emitir = callback; }
      observe() {}
      disconnect() { disconnect(); }
    }
    vi.stubGlobal('IntersectionObserver', Observer);
    const host = document.createElement('div');
    const component = mount(HangarWorking, { target: host });
    const mark = host.querySelector('.mark');
    await tick();

    emitir([{ isIntersecting: false } as IntersectionObserverEntry], {} as IntersectionObserver);
    await tick();
    expect(mark?.classList.contains('paused')).toBe(true);

    emitir([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver);
    await tick();
    expect(mark?.classList.contains('paused')).toBe(false);

    await unmount(component);
    expect(disconnect).toHaveBeenCalledOnce();
    vi.unstubAllGlobals();
  });

  it('pausa enquanto a pagina esta escondida', async () => {
    const original = Object.getOwnPropertyDescriptor(document, 'visibilityState');
    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });
    class Observer {
      constructor(_callback: IntersectionObserverCallback) {}
      observe() {}
      disconnect() {}
    }
    vi.stubGlobal('IntersectionObserver', Observer);
    const host = document.createElement('div');
    const component = mount(HangarWorking, { target: host });
    await tick();
    expect(host.querySelector('.mark')?.classList.contains('paused')).toBe(true);

    Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' });
    document.dispatchEvent(new Event('visibilitychange'));
    await tick();
    expect(host.querySelector('.mark')?.classList.contains('paused')).toBe(false);

    await unmount(component);
    if (original) Object.defineProperty(document, 'visibilityState', original);
    vi.unstubAllGlobals();
  });
});
