/**
 * @vitest-environment happy-dom
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import React, { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { FilesState } from './filesStore';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const mocks = vi.hoisted(() => {
  let snapshot = {
    ready: false,
    servers: [{ id: 'server-1' }],
  };
  const listeners = new Set<() => void>();
  const ensureActive = vi.fn((id: string) => snapshot.servers.some((server) => server.id === id));
  const setReady = (ready: boolean) => {
    snapshot = { ...snapshot, ready };
    for (const listener of listeners) listener();
  };
  const reset = () => {
    snapshot = { ready: false, servers: [{ id: 'server-1' }] };
    ensureActive.mockClear();
  };
  const useServers = Object.assign(
    (selector: (value: typeof snapshot) => unknown) => {
      const current = React.useSyncExternalStore(
        (listener) => {
          listeners.add(listener);
          return () => listeners.delete(listener);
        },
        () => snapshot,
        () => snapshot,
      );
      return selector(current);
    },
    {
      getState: () => ({ ...snapshot, ensureActive }),
    },
  );
  const fileState: FilesState = {
    abertos: new Set<string>(),
    porPasta: new Map(),
    cortePorPasta: new Map(),
    selecionado: 'README.md',
    conteudo: null,
    diff: null,
    escopo: 'branch' as const,
    resultados: [],
    erro: null,
    soModificados: true,
    buscaCortada: false,
    loading: false,
  };
  const setFileState = (patch: Partial<typeof fileState>) => Object.assign(fileState, patch);
  const use = Object.assign((selector: (value: typeof fileState) => unknown) => selector(fileState), {
    setState: vi.fn(),
  });
  const api = {
    use,
    retain: vi.fn(),
    release: vi.fn(),
    abrir: vi.fn().mockResolvedValue(undefined),
    buscar: vi.fn().mockResolvedValue(undefined),
    salvar: vi.fn().mockResolvedValue(null),
    recarregar: vi.fn().mockResolvedValue(undefined),
    recarregarDiff: vi.fn().mockResolvedValue(undefined),
    descartar: vi.fn().mockResolvedValue(undefined),
    alternarPasta: vi.fn().mockResolvedValue(undefined),
    trocarEscopo: vi.fn().mockResolvedValue(undefined),
  };
  return { get state() { return snapshot; }, reset, setReady, setFileState, useServers, api };
});

vi.mock('expo-router', () => ({
  useLocalSearchParams: () => ({ server: 'server-1', name: 'fixture-files', path: 'README.md' }),
}));
vi.mock('../../stores/servers', () => ({ useServers: mocks.useServers }));
vi.mock('./filesStore', () => ({
  filesStore: () => mocks.api,
  entriesOf: () => [],
  listaCortadaOf: () => false,
}));
vi.mock('./FileTree', () => ({ FileTree: () => React.createElement('div', null, 'tree') }));
vi.mock('./FileSearchBar', () => ({ FileSearchBar: () => React.createElement('div', null, 'search') }));
vi.mock('./FileViewer', () => ({ FileViewer: () => React.createElement('div', null, 'viewer') }));
vi.mock('./FileEditor', () => ({ FileEditor: () => React.createElement('div', null, 'editor') }));
vi.mock('../../paraglide/messages', () => ({
  comum_carregando: () => 'Carregando…',
  arq_sessao_encerrada: () => 'Sessão encerrada',
  comum_voltar: () => 'Voltar',
  arq_aba: () => 'Arquivos',
  arq_buscar: () => 'Buscar',
  arq_ver_arquivo: () => 'Arquivo',
  arq_nada_mudou: () => 'Nada mudou',
}));

import FilesSheet from '../../../app/s/[server]/[name]/files';

describe('FilesSheet — deep-link', () => {
  beforeEach(() => {
    mocks.reset();
    mocks.setFileState({ selecionado: 'README.md', erro: null });
    mocks.api.retain.mockClear();
    mocks.api.release.mockClear();
    mocks.api.abrir.mockClear();
    document.body.innerHTML = '';
  });

  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('espera ready antes de abrir o caminho do deep-link', async () => {
    const container = document.createElement('div');
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(React.createElement(FilesSheet));
    });
    expect(mocks.api.abrir).not.toHaveBeenCalled();

    await act(async () => {
      mocks.setReady(true);
      await Promise.resolve();
    });
    expect(mocks.api.retain).toHaveBeenCalledTimes(1);
    expect(mocks.api.abrir).toHaveBeenCalledWith('README.md');
    expect(container.textContent).not.toContain('Nada mudou');

    mocks.setFileState({ selecionado: null, erro: 'erro_arq_inexistente' });
    await act(async () => {
      root.render(React.createElement(FilesSheet));
    });
    expect(container.textContent).toContain('erro_arq_inexistente');

    root.unmount();
  });
});
