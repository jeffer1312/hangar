import { beforeEach, describe, expect, it, vi } from 'vitest';

// Follow-up visual round 2: modo da navegação com a sidebar RECOLHIDA. Default = 'rail'
// (trilho vertical de iniciais — decisão do usuário); 'tabs' é a faixa horizontal de abas.
// Mesmo padrão do sidebarPin/sidebarPrefs: chave localStorage + $state + persistência sem reload.

function fakeStorage(initial: Record<string, string> = {}) {
  const store = new Map<string, string>(Object.entries(initial));
  return {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => { store.set(k, v); },
    removeItem: (k: string) => { store.delete(k); },
  };
}

describe('navMode', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.unstubAllGlobals();
  });

  it('SEM chave salva: default é RAIL (decisão do usuário)', async () => {
    vi.stubGlobal('localStorage', fakeStorage());
    const { navMode } = await import('./navMode.svelte');
    expect(navMode.mode).toBe('rail');
  });

  it('chave "tabs" salva restaura o modo abas', async () => {
    vi.stubGlobal('localStorage', fakeStorage({ cp_nav_mode: 'tabs' }));
    const { navMode } = await import('./navMode.svelte');
    expect(navMode.mode).toBe('tabs');
  });

  it('set "tabs" persiste a chave; set "rail" remove (default implícito)', async () => {
    const storage = fakeStorage();
    vi.stubGlobal('localStorage', storage);
    const { navMode } = await import('./navMode.svelte');
    navMode.mode = 'tabs';
    expect(storage.getItem('cp_nav_mode')).toBe('tabs');
    navMode.mode = 'rail';
    expect(storage.getItem('cp_nav_mode')).toBeNull();
    expect(navMode.mode).toBe('rail');
  });
});
