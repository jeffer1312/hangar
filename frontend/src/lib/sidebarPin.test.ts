import { beforeEach, describe, expect, it, vi } from 'vitest';

// Stub funcional de localStorage: cada teste isola o storage. vi.resetModules() força o módulo
// (que lê o storage no load) a re-executar por teste — sem isto o estado vazava entre `it`s.
function fakeStorage(initial: Record<string, string> = {}) {
  const store = new Map<string, string>(Object.entries(initial));
  return {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => { store.set(k, v); },
    removeItem: (k: string) => { store.delete(k); },
  };
}

describe('sidebarPin', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.unstubAllGlobals();
  });

  it('lê o pin persistido do localStorage', async () => {
    vi.stubGlobal('localStorage', fakeStorage({ cp_sidebar_collapsed: '1' }));
    const { sidebarPin } = await import('./sidebarPin.svelte');
    expect(sidebarPin.collapsed).toBe(true);
    expect(sidebarPin.preferred).toBe(true);
  });

  it('setUser(true) persiste 1 e muda o estado efetivo', async () => {
    const storage = fakeStorage();
    vi.stubGlobal('localStorage', storage);
    const { sidebarPin } = await import('./sidebarPin.svelte');
    sidebarPin.setUser(true);
    expect(storage.getItem('cp_sidebar_collapsed')).toBe('1');
    expect(sidebarPin.collapsed).toBe(true);
  });

  it('setForced muda o efetivo sem tocar storage nem preferred', async () => {
    const storage = fakeStorage();
    vi.stubGlobal('localStorage', storage);
    const { sidebarPin } = await import('./sidebarPin.svelte');
    sidebarPin.setUser(false);
    sidebarPin.setForced(true);
    expect(sidebarPin.collapsed).toBe(true);
    expect(sidebarPin.preferred).toBe(false);
    expect(storage.getItem('cp_sidebar_collapsed')).toBe('0');
  });

  it('setForced(null) restaura a preferência', async () => {
    vi.stubGlobal('localStorage', fakeStorage({ cp_sidebar_collapsed: '1' }));
    const { sidebarPin } = await import('./sidebarPin.svelte');
    sidebarPin.setForced(false);
    expect(sidebarPin.collapsed).toBe(false);
    sidebarPin.setForced(null);
    expect(sidebarPin.collapsed).toBe(true);
  });

  it('toggleUser inverte e persiste', async () => {
    const storage = fakeStorage();
    vi.stubGlobal('localStorage', storage);
    const { sidebarPin } = await import('./sidebarPin.svelte');
    sidebarPin.toggleUser();
    expect(sidebarPin.collapsed).toBe(true);
    sidebarPin.toggleUser();
    expect(sidebarPin.collapsed).toBe(false);
    expect(storage.getItem('cp_sidebar_collapsed')).toBe('0');
  });

  it('forcedOverride expõe o override sem confundir com a preferência (round 7)', async () => {
    const storage = fakeStorage();
    vi.stubGlobal('localStorage', storage);
    const { sidebarPin } = await import('./sidebarPin.svelte');
    expect(sidebarPin.forcedOverride).toBeNull();
    sidebarPin.setForced(true);
    expect(sidebarPin.forcedOverride).toBe(true);
    expect(sidebarPin.preferred).toBe(false);   // preferência intocada
    sidebarPin.setForced(null);
    expect(sidebarPin.forcedOverride).toBeNull();
  });
});
