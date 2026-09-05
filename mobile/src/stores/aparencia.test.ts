import { describe, it, expect, beforeEach, vi } from 'vitest';
// Import estático só pelo efeito: transforma o @hangar/core (e as ~2400 mensagens do paraglide que
// ele arrasta) na COLETA, fora do relógio de qualquer teste. Sem isto o primeiro `await import()`
// paga a transformação fria dentro do próprio timeout e estoura sob carga.
import '@hangar/core';
// Mesma razão: o store passou a depender destes dois, e transformá-los dentro do primeiro
// `await import()` estourava o timeout de novo quando a suíte inteira roda em paralelo.
import 'react-native';
import '../theme/aplicarMaterial';

const mem = new Map<string, string | number>();
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (k: string) => mem.get(k) as string | undefined,
    getNumber: (k: string) => mem.get(k) as number | undefined,
    set: (k: string, v: string | number) => void mem.set(k, v),
    remove: (k: string) => void mem.delete(k),
  }),
}));

// Cada caso precisa de um store NOVO (é assim que se testa a leitura das prefs na construção), e
// `vi.resetModules()` faz o grafo inteiro do store ser reavaliado a cada um. Com a suíte inteira
// rodando em paralelo isso passa dos 5s padrão sem que nada esteja errado — medido em 10s por caso.
vi.setConfig({ testTimeout: 30_000 });

const tick = () => new Promise((r) => setTimeout(r, 0));

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

  it('idioma padrão system, persiste e valor desconhecido cai em system', async () => {
    const { useAparencia } = await import('./aparencia');
    expect(useAparencia.getState().idioma).toBe('system');
    useAparencia.getState().setIdioma('en');
    expect(mem.get('aparencia.idioma')).toBe('en');
    vi.resetModules();
    const { useAparencia: deNovo } = await import('./aparencia');
    expect(deNovo.getState().idioma).toBe('en');
    mem.set('aparencia.idioma', 'roxo');
    vi.resetModules();
    const { useAparencia: comLixo } = await import('./aparencia');
    expect(comLixo.getState().idioma).toBe('system');
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

  it('material: padrões, persistência e valor gravado inválido', async () => {
    const { useAparencia } = await import('./aparencia');
    const s = useAparencia.getState();
    expect(s.fundo).toBe('flat');
    expect(s.imagemUri).toBeNull();
    expect(s.panelAlpha).toBe(0.86);
    expect(s.surfaceAlpha).toBe(1);
    expect(s.acento).toBeNull();

    s.setFundo('aurora');
    s.setPanelAlpha(0.5);
    s.setSurfaceAlpha(0.4);
    s.setAcento('#ff0000');
    // O estado muda na hora; a gravação espera o apply do tema (um tick, uma vez pros três).
    expect(useAparencia.getState().panelAlpha).toBe(0.5);
    expect(mem.has('aparencia.panelAlpha')).toBe(false);
    await tick();
    expect(mem.get('aparencia.fundo')).toBe('aurora');
    expect(mem.get('aparencia.panelAlpha')).toBe(0.5);
    expect(mem.get('aparencia.surfaceAlpha')).toBe(0.4);
    expect(mem.get('aparencia.acento')).toBe('#ff0000');

    vi.resetModules();
    const { useAparencia: deNovo } = await import('./aparencia');
    expect(deNovo.getState().fundo).toBe('aurora');
    expect(deNovo.getState().panelAlpha).toBe(0.5);
    expect(deNovo.getState().acento).toBe('#ff0000');

    mem.set('aparencia.fundo', 'roxo');
    mem.set('aparencia.acento', 'azul');
    vi.resetModules();
    const { useAparencia: comLixo } = await import('./aparencia');
    expect(comLixo.getState().fundo).toBe('flat');
    expect(comLixo.getState().acento).toBeNull();
  });

  it('panelAlpha nunca desce do piso, surfaceAlpha não passa de 1 e NaN cai no padrão', async () => {
    const { useAparencia } = await import('./aparencia');
    useAparencia.getState().setPanelAlpha(0);
    expect(useAparencia.getState().panelAlpha).toBe(0.3);
    useAparencia.getState().setSurfaceAlpha(2);
    expect(useAparencia.getState().surfaceAlpha).toBe(1);
    // Math.min(1, NaN) é NaN: sem o guarda o tema receberia rgba(...,NaN) e nenhuma cor pintaria.
    useAparencia.getState().setPanelAlpha(Number.NaN);
    expect(useAparencia.getState().panelAlpha).toBe(0.86);
    useAparencia.getState().setSurfaceAlpha(Number.NaN);
    expect(useAparencia.getState().surfaceAlpha).toBe(1);
  });

  it('vários setters no mesmo tick aplicam o material uma vez só', async () => {
    const { useAparencia } = await import('./aparencia');
    const { UnistylesRuntime } = await import('react-native-unistyles');
    const espia = vi.spyOn(UnistylesRuntime, 'updateTheme');
    const s = useAparencia.getState();
    s.setPanelAlpha(0.7);
    s.setPanelAlpha(0.6);
    s.setSurfaceAlpha(0.5);
    await tick();
    // dois temas × UM apply: sem a coalescência seriam seis chamadas
    expect(espia).toHaveBeenCalledTimes(2);
    expect(mem.get('aparencia.panelAlpha')).toBe(0.6);
    espia.mockRestore();
  });

  it('setImagemUri(null) apaga a foto e volta pro fundo flat', async () => {
    mem.set('aparencia.fundo', 'image');
    mem.set('aparencia.imagemUri', 'file:///x.jpg');
    vi.resetModules();
    const { useAparencia } = await import('./aparencia');
    await useAparencia.getState().setImagemUri(null);
    expect(useAparencia.getState().imagemUri).toBeNull();
    expect(useAparencia.getState().fundo).toBe('flat');
    expect(mem.has('aparencia.imagemUri')).toBe(false);
  });
});
