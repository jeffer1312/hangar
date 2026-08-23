import { describe, it, expect, vi, beforeEach } from 'vitest';

// mock do core antes de importar o store
vi.mock('@hangar/core', async () => {
  const actual = await vi.importActual<typeof import('@hangar/core')>('@hangar/core');
  return {
    ...actual,
    getConfig: vi.fn(),
    patchConfig: vi.fn(),
  };
});

import { useDitadoEstiloStore } from './ditadoEstiloStore';
import * as core from '@hangar/core';

function campos(valor: string) {
  return {
    campos: {
      ditado_estilo: { valor, definido: true, origem: 'app' as const },
    },
    somente_leitura: {},
  };
}

beforeEach(() => {
  useDitadoEstiloStore.getState()._zerarParaTeste();
  vi.clearAllMocks();
});

describe('ditadoEstiloStore', () => {
  it('GET atrasado não sobrescreve troca', async () => {
    const getConfig = vi.mocked(core.getConfig);
    const patchConfig = vi.mocked(core.patchConfig);

    let resolveGet!: (v: unknown) => void;
    const getPromise = new Promise((res) => {
      resolveGet = res;
    });
    getConfig.mockReturnValue(getPromise as unknown as Promise<never>);
    patchConfig.mockResolvedValue({ campos: {} } as never);

    const carga = useDitadoEstiloStore.getState().carregar(); // GET em voo
    await useDitadoEstiloStore.getState().trocar('briefing');
    expect(useDitadoEstiloStore.getState().valor).toBe('briefing');

    // GET resolve tarde com valor antigo
    resolveGet(campos('limpar'));
    await carga;
    expect(useDitadoEstiloStore.getState().valor).toBe('briefing');
  });

  it('carregar pinta valor do servidor quando nenhuma escrita no meio', async () => {
    const getConfig = vi.mocked(core.getConfig);
    getConfig.mockResolvedValue(campos('limpar') as never);
    await useDitadoEstiloStore.getState().carregar();
    expect(useDitadoEstiloStore.getState().valor).toBe('limpar');
    expect(useDitadoEstiloStore.getState().pronto).toBe(true);
  });

  it('trocar reverte em falha quando ainda é a última', async () => {
    const patchConfig = vi.mocked(core.patchConfig);
    patchConfig.mockRejectedValue(new Error('502'));
    // primeiro carrega pra ter pronto
    const getConfig = vi.mocked(core.getConfig);
    getConfig.mockResolvedValue(campos('prosa') as never);
    await useDitadoEstiloStore.getState().carregar();
    expect(useDitadoEstiloStore.getState().valor).toBe('prosa');
    await expect(useDitadoEstiloStore.getState().trocar('briefing')).rejects.toThrow('502');
    expect(useDitadoEstiloStore.getState().valor).toBe('prosa');
  });

  it('revalidar força nova leitura mesmo já pronto', async () => {
    const getConfig = vi.mocked(core.getConfig);
    getConfig.mockResolvedValue(campos('prosa') as never);
    await useDitadoEstiloStore.getState().carregar();
    expect(useDitadoEstiloStore.getState().pronto).toBe(true);
    getConfig.mockResolvedValue(campos('briefing') as never);
    await useDitadoEstiloStore.getState().revalidar();
    expect(useDitadoEstiloStore.getState().valor).toBe('briefing');
  });
});
