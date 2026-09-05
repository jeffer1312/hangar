import { describe, it, expect, beforeEach, vi } from 'vitest';

const mem = new Map<string, string>();
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (k: string) => mem.get(k),
    set: (k: string, v: string) => void mem.set(k, v),
    remove: (k: string) => void mem.delete(k),
  }),
}));

describe('aparencia', () => {
  beforeEach(() => { mem.clear(); vi.resetModules(); });

  it('tema padrão é system', async () => {
    const { useAparencia } = await import('./aparencia');
    expect(useAparencia.getState().tema).toBe('system');
  });

  it('setTema persiste e valor desconhecido cai em system', async () => {
    const { useAparencia } = await import('./aparencia');
    useAparencia.getState().setTema('dark');
    expect(mem.get('aparencia.tema')).toBe('dark');
    mem.set('aparencia.tema', 'roxo');
    vi.resetModules();
    const { useAparencia: deNovo } = await import('./aparencia');
    expect(deNovo.getState().tema).toBe('system');
  });
});
