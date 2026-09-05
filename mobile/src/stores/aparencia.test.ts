import { describe, it, expect, beforeEach, vi } from 'vitest';

const mem = new Map<string, string>();
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (k: string) => mem.get(k),
    set: (k: string, v: string) => void mem.set(k, v),
    remove: (k: string) => void mem.delete(k),
  }),
}));

// O `resetModules` faz cada `import('./aparencia')` reexecutar o grafo inteiro, e o store puxa o
// @hangar/core — a primeira transformação dele passa dos 5s padrão do vitest.
vi.setConfig({ testTimeout: 30_000 });

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

  it('pensamentoTools padrão busca e persiste', async () => {
    const { useAparencia } = await import('./aparencia');
    expect(useAparencia.getState().pensamentoTools).toBe('busca');
    useAparencia.getState().setPensamentoTools('tudo');
    expect(mem.get('aparencia.pensamentoTools')).toBe('tudo');
    vi.resetModules();
    const { useAparencia: deNovo } = await import('./aparencia');
    expect(deNovo.getState().pensamentoTools).toBe('tudo');
    mem.set('aparencia.pensamentoTools', 'roxo');
    vi.resetModules();
    const { useAparencia: comLixo } = await import('./aparencia');
    expect(comLixo.getState().pensamentoTools).toBe('busca');
  });

  it('agrupar padrão server, persiste e valor desconhecido cai em server', async () => {
    const { useAparencia } = await import('./aparencia');
    expect(useAparencia.getState().agrupar).toBe('server');
    useAparencia.getState().setAgrupar('project');
    expect(mem.get('lista.agrupar')).toBe('project');
    vi.resetModules();
    const { useAparencia: deNovo } = await import('./aparencia');
    expect(deNovo.getState().agrupar).toBe('project');
    mem.set('lista.agrupar', 'roxo');
    vi.resetModules();
    const { useAparencia: comLixo } = await import('./aparencia');
    expect(comLixo.getState().agrupar).toBe('server');
  });
});
