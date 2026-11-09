/**
 * @vitest-environment happy-dom
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import React, { act } from 'react';
import { createRoot } from 'react-dom/client';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const { core, ditado } = vi.hoisted(() => ({
  core: { transcribe: vi.fn(), diag: vi.fn() },
  ditado: { onFim: null as null | ((file: File, motivo: string, uri: string, clock: { stoppedAt: number; fileAt: number }) => void) },
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  transcribeDictationForServer: core.transcribe,
  registrarDiag: core.diag,
}));
vi.mock('../ditado/useDitado', () => ({
  useDitado: (opts: { onFim: NonNullable<typeof ditado.onFim> }) => {
    ditado.onFim = opts.onFim;
    return { gravando: false, rms: { value: 0 }, iniciar: vi.fn(), parar: vi.fn() };
  },
}));
vi.mock('../ditado/ditadoEstiloStore', () => ({ dictationStyleStore: () => ({ getState: () => ({ pronto: false }) }) }));

import { useHomeDictation } from './useHomeDictation';

const server = { id: 's1', label: 'A', baseUrl: 'https://a.test', token: 't' };

function Probe({ onText }: { onText: (text: string) => void }) {
  useHomeDictation(server, onText);
  return null;
}

describe('useHomeDictation: dictation.client_timing', () => {
  let root: ReturnType<typeof createRoot>;
  beforeEach(async () => {
    core.transcribe.mockReset(); core.diag.mockReset();
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(10_000);
    root = createRoot(document.createElement('div'));
    await act(async () => { root.render(React.createElement(Probe, { onText: () => vi.setSystemTime(11_040) })); });
  });
  afterEach(async () => {
    await act(async () => { root.unmount(); });
    vi.useRealTimers();
  });

  it('o ditado da tela inicial mede como o da conversa: arquivo, transcrição, fechamento e motivo', async () => {
    core.transcribe.mockImplementationOnce(async () => { vi.setSystemTime(11_000); return { text: 'olá', aviso: null }; });
    await act(async () => { ditado.onFim!(new File(['a'], 'd.m4a'), 'teto', 'file://d.m4a', { stoppedAt: 9_800, fileAt: 9_900 }); });
    expect(core.diag).toHaveBeenCalledExactlyOnceWith({ evento: 'dictation.client_timing', nivel: 'ok', ms: 1240, motivo: 'teto',
      detalhe: 'stop_to_file_ms=100 prepare_ms=100 check_ms=0 upload_ms=0 save_ms=0 transcribe_ms=1000 finish_ms=40 total_ms=1240' },
    'https://a.test');
  });

  it('a falha leva o código e a etapa', async () => {
    core.transcribe.mockImplementationOnce(async () => { vi.setSystemTime(10_300); throw new Error('503: fora'); });
    await act(async () => { ditado.onFim!(new File(['a'], 'd.m4a'), 'botao', 'file://d.m4a', { stoppedAt: 10_000, fileAt: 10_000 }); });
    expect(core.diag).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ nivel: 'erro', codigo: '503', etapa: 'transcribe', ms: 300 }),
      'https://a.test');
  });
});
