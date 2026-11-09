/**
 * @vitest-environment happy-dom
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import React, { act } from 'react';
import { createRoot } from 'react-dom/client';
import type { RecordingStatus } from 'expo-audio';
import { configureDiag, type EventoDiag } from '@hangar/core';
import * as m from '../../paraglide/messages';

const environment = vi.hoisted(() => ({ os: 'android', version: 35, runtime: 'native-3' }));
vi.mock('expo-updates', () => ({ get runtimeVersion() { return environment.runtime; } }));

const recordDiagnostic = vi.fn<(event: EventoDiag, destination?: string) => void>();

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const { fakeRec, createRecorder, requestPermission, setAudioMode, appState, recorderStatus, removeStatusListener } = vi.hoisted(() => {
  const fakeRec: {
    uri: string | null;
    stop: ReturnType<typeof vi.fn>;
    prepareToRecordAsync: ReturnType<typeof vi.fn>;
    record: ReturnType<typeof vi.fn>;
    getStatus: ReturnType<typeof vi.fn>;
    release: ReturnType<typeof vi.fn>;
    addListener: ReturnType<typeof vi.fn>;
  } = {
    uri: 'file://fake.m4a',
    stop: vi.fn(async () => {}),
    prepareToRecordAsync: vi.fn(async () => {}),
    record: vi.fn(() => {}),
    getStatus: vi.fn(() => ({ metering: -160 })),
    release: vi.fn(),
    addListener: vi.fn(),
  };
  return {
    fakeRec,
    createRecorder: vi.fn(function () { return fakeRec; }),
    requestPermission: vi.fn(async () => ({ granted: true })),
    setAudioMode: vi.fn(async () => {}),
    appState: { currentState: 'active', listener: null as ((next: string) => void) | null },
    recorderStatus: { listener: null as ((status: RecordingStatus) => void) | null },
    removeStatusListener: vi.fn(),
  };
});

vi.mock('expo-audio', () => ({
  AudioModule: { AudioRecorder: createRecorder },
  RecordingPresets: { HIGH_QUALITY: { extension: '.m4a', android: { outputFormat: 'mpeg4', audioEncoder: 'aac' } } },
  requestRecordingPermissionsAsync: requestPermission,
  requestNotificationPermissionsAsync: vi.fn(async () => ({ granted: true })),
  setAudioModeAsync: setAudioMode,
}));

const toastAviso = vi.hoisted(() => vi.fn());
vi.mock('../../ui/Toast', () => ({ toast: { aviso: toastAviso } }));

vi.mock('react-native', async () => {
  const actual = (await vi.importActual<typeof import('../../__mocks__/react-native')>('../../__mocks__/react-native')) as Record<string, unknown>;
  return {
    ...actual,
    Platform: { get OS() { return environment.os; }, get Version() { return environment.version; } },
    AppState: {
      addEventListener: vi.fn((_event: string, listener: (next: string) => void) => {
        appState.listener = listener;
        return { remove: () => { appState.listener = null; } };
      }),
      get currentState() { return appState.currentState; },
    },
  };
});

import { requestNotificationPermissionsAsync } from 'expo-audio';
import { useDitado } from './useDitado';
import { useDitadoEstiloStore } from './ditadoEstiloStore';
import { setLiveVoiceAudioActive, useVoiceAudioState } from '../voice/audioSession';
import type { MotivoFim } from '@hangar/core';

function mountHook(opts: { onFim: (f: File, m: MotivoFim, uri: string) => void; onErroParada?: (e: Error) => void }) {
  let renders = 0;
  let hook: ReturnType<typeof useDitado> | null = null;
  function Comp(p: typeof opts) {
    renders++;
    hook = useDitado(p);
    return null;
  }
  const container = document.createElement('div');
  document.body.appendChild(container);
  const root = createRoot(container);
  return {
    renderCount: () => renders,
    // monta e atualiza hook via render
    mount: async (props: typeof opts) => {
      await act(async () => {
        root.render(React.createElement(Comp, props));
      });
      if (!hook) throw new Error('hook not mounted');
    },
    getHook: () => {
      if (!hook) throw new Error('hook not mounted');
      return hook;
    },
    unmount: async () => {
      await act(async () => {
        root.unmount();
      });
      container.remove();
    },
  };
}

beforeEach(() => {
  useVoiceAudioState.setState({ voiceActive: false, dictations: 0 });
  vi.useFakeTimers();
  recordDiagnostic.mockClear();
  configureDiag({ registrar: recordDiagnostic, novoReq: () => '' });
  // Sem servidor no teste: a releitura real do teto falharia e registraria diagnóstico.
  useDitadoEstiloStore.setState({ refreshCeiling: async () => {} });
  document.body.innerHTML = '';
  environment.os = 'android';
  environment.version = 35;
  environment.runtime = 'native-3';
  vi.mocked(requestNotificationPermissionsAsync).mockReset().mockResolvedValue({ granted: true } as never);
  appState.currentState = 'active';
  appState.listener = null;
  createRecorder.mockClear();
  requestPermission.mockReset().mockResolvedValue({ granted: true });
  setAudioMode.mockReset().mockResolvedValue(undefined);
  recorderStatus.listener = null;
  removeStatusListener.mockClear();
  fakeRec.uri = 'file://fake.m4a';
  fakeRec.stop = vi.fn(async () => { emitRecordingStatus(); });
  fakeRec.prepareToRecordAsync = vi.fn(async () => {});
  fakeRec.record = vi.fn(() => {});
  fakeRec.getStatus = vi.fn(() => ({ metering: -160 }));
  fakeRec.release = vi.fn();
  fakeRec.addListener = vi.fn((_event: string, listener: (status: RecordingStatus) => void) => {
    recorderStatus.listener = listener;
    return { remove: () => { removeStatusListener(); recorderStatus.listener = null; } };
  });
  vi.stubGlobal('fetch', vi.fn(async () => ({ blob: async () => new Blob(['audio'], { type: 'audio/m4a' }) } as unknown as Response)));
});

afterEach(async () => {
  document.body.innerHTML = '';
  vi.unstubAllGlobals();
  configureDiag({ registrar: () => {}, novoReq: () => '' });
  vi.clearAllTimers();
  vi.useRealTimers();
  expect(useVoiceAudioState.getState().dictations).toBe(0);
  useVoiceAudioState.setState({ voiceActive: false, dictations: 0 });
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function changeAppState(next: string) {
  appState.currentState = next;
  appState.listener?.(next);
}

function emitRecordingStatus(status: Partial<RecordingStatus> = {}) {
  recorderStatus.listener?.({
    id: 'fake', isFinished: true, hasError: false, error: null, url: fakeRec.uri, ...status,
  });
}

describe('useDitado — coexistência com voz ao vivo', () => {
  it.each(['denied', 'permission', 'mode', 'record'] as const)('libera o áudio depois da falha em %s', async (failure) => {
    const error = new Error(failure === 'denied' ? 'permission_denied' : 'áudio indisponível');
    requestPermission.mockImplementationOnce(async () => {
      expect(useVoiceAudioState.getState().dictations).toBe(1);
      if (failure === 'permission') throw error;
      return { granted: failure !== 'denied' };
    });
    if (failure === 'mode') setAudioMode.mockRejectedValueOnce(error);
    if (failure === 'record') fakeRec.record.mockImplementationOnce(() => { throw error; });
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await expect(h.getHook().iniciar()).rejects.toThrow(error.message); });
    expect(useVoiceAudioState.getState().dictations).toBe(0);
    expect(onFim).not.toHaveBeenCalled();
    await h.unmount();
    expect(useVoiceAudioState.getState().dictations).toBe(0);
  });

  it('avisa início e fim do ditado sem encerrar a chamada já ativa', async () => {
    setLiveVoiceAudioActive(true);
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    expect(useVoiceAudioState.getState()).toEqual({ voiceActive: true, dictations: 1 });
    await act(async () => { await h.getHook().parar('botao'); });
    expect(useVoiceAudioState.getState()).toEqual({ voiceActive: true, dictations: 0 });
    expect(onFim).toHaveBeenCalledTimes(1);
    await h.unmount();
  });
});

describe('useDitado — parar() propaga falha via onErroParada', () => {
  it('stop() rejeita → onErroParada é chamado e onFim não', async () => {
    const onFim = vi.fn();
    const onErroParada = vi.fn();
    const h = mountHook({ onFim, onErroParada });
    await h.mount({ onFim, onErroParada });
    await act(async () => {
      await h.getHook().iniciar();
    });

    const erro = new Error('mic falhou');
    fakeRec.stop = vi.fn(async () => {
      throw erro;
    });

    await act(async () => {
      await h.getHook().parar('botao');
    });

    expect(onErroParada).toHaveBeenCalledTimes(1);
    expect(onErroParada.mock.calls[0][0]).toBe(erro);
    expect(onFim).not.toHaveBeenCalled();

    await h.unmount();
  });

  it('uri nulo → onErroParada é chamado e onFim não', async () => {
    const onFim = vi.fn();
    const onErroParada = vi.fn();
    const h = mountHook({ onFim, onErroParada });
    await h.mount({ onFim, onErroParada });
    await act(async () => {
      await h.getHook().iniciar();
    });

    fakeRec.uri = null;
    fakeRec.stop = vi.fn(async () => { emitRecordingStatus(); });

    await act(async () => {
      await h.getHook().parar('botao');
    });

    expect(onErroParada).toHaveBeenCalledTimes(1);
    expect(onErroParada.mock.calls[0][0].message).toBe('ditado_parada_falhou');
    expect(onFim).not.toHaveBeenCalled();

    await h.unmount();
  });

  it('parar com sucesso chama onFim e não chama onErroParada', async () => {
    const onFim = vi.fn();
    const onErroParada = vi.fn();
    const h = mountHook({ onFim, onErroParada });
    await h.mount({ onFim, onErroParada });
    await act(async () => {
      await h.getHook().iniciar();
    });

    fakeRec.uri = 'file://fake.m4a';
    fakeRec.stop = vi.fn(async () => { emitRecordingStatus(); });

    await act(async () => {
      await h.getHook().parar('botao');
    });

    expect(onFim).toHaveBeenCalledTimes(1);
    expect(onErroParada).not.toHaveBeenCalled();
    const [file, motivo, uri] = onFim.mock.calls[0];
    expect(file).toBeInstanceOf(File);
    expect(motivo).toBe('botao');
    expect(uri).toBe('file://fake.m4a');
    expect(fakeRec.release).toHaveBeenCalledTimes(1);
    expect(removeStatusListener).toHaveBeenCalledTimes(1);

    await h.unmount();
  });

  it('sobrevive a re-render com onErroParada inline (bloqueador 2)', async () => {
    const onFim = vi.fn();
    const onErro = vi.fn();
    const h = mountHook({ onFim, onErroParada: (e) => onErro(e) });
    await h.mount({ onFim, onErroParada: (e) => onErro(e) });
    await act(async () => {
      await h.getHook().iniciar();
    });
    // Atualizar o texto no Composer também pode trocar o callback inline.
    await h.mount({ onFim, onErroParada: (e) => onErro(e) });
    await act(async () => {
      await h.getHook().parar('botao');
    });
    expect(onFim).toHaveBeenCalledTimes(1);
    expect(onErro).not.toHaveBeenCalled();
    await h.unmount();
  });
});

describe('useDitado — interrupção e origem da gravação', () => {
  it('bloqueia preparação dupla e conserva o callback capturado antes da permissão', async () => {
    const permission = deferred<{ granted: boolean }>();
    requestPermission.mockReturnValue(permission.promise);
    const original = vi.fn();
    const replacement = vi.fn();
    const h = mountHook({ onFim: original });
    await h.mount({ onFim: original });
    let starting!: Promise<void>;
    await act(async () => {
      starting = h.getHook().iniciar();
      await h.getHook().iniciar();
    });
    expect(requestPermission).toHaveBeenCalledTimes(1);
    expect(useVoiceAudioState.getState().dictations).toBe(1);
    expect(createRecorder).not.toHaveBeenCalled();
    await h.mount({ onFim: replacement });
    await act(async () => {
      permission.resolve({ granted: true });
      await starting;
      await h.getHook().parar('botao');
    });
    expect(createRecorder).toHaveBeenCalledTimes(1);
    expect(fakeRec.prepareToRecordAsync).toHaveBeenCalledTimes(1);
    expect(original).toHaveBeenCalledTimes(1);
    expect(replacement).not.toHaveBeenCalled();
    await h.unmount();
  });

  it('permite o primeiro ditado após inactive transitório do pedido de permissão', async () => {
    const permission = deferred<{ granted: boolean }>();
    requestPermission.mockReturnValue(permission.promise);
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    let starting!: Promise<void>;
    await act(async () => {
      starting = h.getHook().iniciar();
      changeAppState('inactive');
      changeAppState('active');
      permission.resolve({ granted: true });
      await starting;
    });
    expect(fakeRec.record).toHaveBeenCalledTimes(1);
    await act(async () => { await h.getHook().parar('botao'); });
    expect(onFim).toHaveBeenCalledTimes(1);
    await h.unmount();
  });

  it.each([
    ['permission', 'unmount'], ['permission', 'cancel'],
    ['mode', 'unmount'], ['mode', 'cancel'],
    ['prepare', 'background'], ['prepare', 'unmount'], ['prepare', 'inactive'], ['prepare', 'cancel'],
  ] as const)('cancela início pendente em %s após %s', async (stage, interruption) => {
    const permission = deferred<{ granted: boolean }>();
    const preparation = deferred<void>();
    if (stage === 'permission') requestPermission.mockReturnValue(permission.promise);
    else if (stage === 'mode') setAudioMode.mockReturnValue(preparation.promise);
    else fakeRec.prepareToRecordAsync.mockReturnValue(preparation.promise);
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    let starting!: Promise<void>;
    await act(async () => { starting = h.getHook().iniciar(); });
    if (stage === 'prepare') expect(fakeRec.prepareToRecordAsync).toHaveBeenCalledTimes(1);
    expect(useVoiceAudioState.getState().dictations).toBe(1);
    if (interruption === 'unmount') await h.unmount();
    else if (interruption === 'cancel') await act(async () => { await h.getHook().parar('botao'); });
    else await act(async () => {
      changeAppState(interruption === 'inactive' ? 'inactive' : 'background');
      changeAppState('active');
    });
    expect(useVoiceAudioState.getState().dictations).toBe(1);
    expect(fakeRec.release).not.toHaveBeenCalled();
    await act(async () => {
      permission.resolve({ granted: true });
      preparation.resolve();
      await starting;
    });
    expect(fakeRec.record).not.toHaveBeenCalled();
    expect(fakeRec.stop).not.toHaveBeenCalled();
    expect(onFim).not.toHaveBeenCalled();
    expect(fakeRec.release).toHaveBeenCalledTimes(stage === 'prepare' ? 1 : 0);
    expect(useVoiceAudioState.getState().dictations).toBe(0);
    if (interruption !== 'unmount') {
      await act(async () => {
        await h.getHook().iniciar();
        await h.getHook().parar('botao');
      });
      expect(fakeRec.record).toHaveBeenCalledTimes(1);
      await h.unmount();
    }
  });

  it.each([
    ['Android', 'background'], ['iOS', 'inactive'],
  ] as const)('%s: permissão resolvida antes do active inicia o primeiro ditado', async (_platform, suspended) => {
    const permission = deferred<{ granted: boolean }>();
    requestPermission.mockReturnValue(permission.promise);
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    let starting!: Promise<void>;
    await act(async () => {
      starting = h.getHook().iniciar();
      changeAppState(suspended);
      permission.resolve({ granted: true });
      await starting;
    });
    expect(fakeRec.record).toHaveBeenCalledTimes(1);
    expect(h.getHook().gravando).toBe(true);
    await act(async () => {
      changeAppState('active');
      await h.getHook().parar('botao');
    });
    expect(onFim).toHaveBeenCalledTimes(1);
    expect(onFim.mock.calls[0]?.[1]).toBe('botao');
    await h.unmount();
  });

  it('propaga falha de preparação, libera o objeto e permite outro início', async () => {
    const erro = new Error('prepare falhou');
    fakeRec.prepareToRecordAsync.mockRejectedValueOnce(erro);
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => {
      await expect(h.getHook().iniciar()).rejects.toBe(erro);
    });
    expect(h.getHook().gravando).toBe(false);
    expect(fakeRec.record).not.toHaveBeenCalled();
    expect(fakeRec.release).toHaveBeenCalledTimes(1);
    await act(async () => {
      await h.getHook().iniciar();
      await h.getHook().parar('botao');
    });
    expect(fakeRec.record).toHaveBeenCalledTimes(1);
    await h.unmount();
  });

  it('desmontar aguarda a parada e leitura do áudio antes de liberar o recorder', async () => {
    const stop = deferred<void>();
    const delivered = deferred<void>();
    fakeRec.stop.mockReturnValue(stop.promise);
    const onFim = vi.fn(() => {
      expect(fakeRec.release).not.toHaveBeenCalled();
      delivered.resolve();
    });
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    const beforeStop = Date.now();
    await h.unmount();
    const stopped = Date.now();
    expect(fakeRec.stop).toHaveBeenCalledTimes(1);
    expect(fakeRec.release).not.toHaveBeenCalled();
    expect(useVoiceAudioState.getState().dictations).toBe(1);
    await act(async () => {
      stop.resolve();
      emitRecordingStatus();
      await delivered.promise;
    });
    expect(onFim.mock.calls[0]).toEqual([expect.any(File), 'escondeu', 'file://fake.m4a', { stoppedAt: expect.any(Number), fileAt: expect.any(Number) }]);
    // A parada e o arquivo pronto: `dictation.client_timing` mede entre os dois, como o nativo mede o WAV.
    const { stoppedAt, fileAt } = (onFim.mock.calls[0] as unknown[])[3] as { stoppedAt: number; fileAt: number };
    expect(stoppedAt >= beforeStop && stoppedAt <= stopped).toBe(true);
    expect(fileAt >= stoppedAt && fileAt <= Date.now()).toBe(true);
    expect(fakeRec.release).toHaveBeenCalledTimes(1);

  });

  it('não reinicia nem para duas vezes durante a parada e leitura do áudio', async () => {
    const stop = deferred<void>();
    const audio = deferred<Blob>();
    fakeRec.stop.mockReturnValue(stop.promise);
    vi.stubGlobal('fetch', vi.fn(async () => ({ blob: () => audio.promise })));
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    let ending!: Promise<void>;
    await act(async () => {
      ending = h.getHook().parar('silencio');
      expect(h.getHook().parar('escondeu')).toBe(ending);
      await h.getHook().iniciar();
    });
    expect(fakeRec.record).toHaveBeenCalledTimes(1);
    expect(h.getHook().gravando).toBe(true);
    await act(async () => {
      stop.resolve();
      emitRecordingStatus();
      await h.getHook().iniciar();
    });
    expect(fakeRec.record).toHaveBeenCalledTimes(1);
    expect(fakeRec.release).not.toHaveBeenCalled();
    expect(useVoiceAudioState.getState().dictations).toBe(1);
    expect(h.getHook().gravando).toBe(true);
    await act(async () => {
      audio.resolve(new Blob(['audio']));
      await ending;
    });
    expect(onFim).toHaveBeenCalledTimes(1);
    expect(onFim.mock.calls[0]?.[1]).toBe('escondeu');
    expect(fakeRec.stop).toHaveBeenCalledTimes(1);
    expect(h.getHook().gravando).toBe(false);
    await h.unmount();
  });

  it('aguarda o evento nativo quando stop resolve e depois recebe hasError', async () => {
    fakeRec.stop = vi.fn(async () => {});
    const onFim = vi.fn();
    const onErroParada = vi.fn();
    const h = mountHook({ onFim, onErroParada });
    await h.mount({ onFim, onErroParada });
    await act(async () => { await h.getHook().iniciar(); });
    let ending!: Promise<void>;
    await act(async () => { ending = h.getHook().parar('botao'); });
    expect(onFim).not.toHaveBeenCalled();
    expect(fakeRec.release).not.toHaveBeenCalled();
    await act(async () => {
      emitRecordingStatus({ hasError: true, error: 'stop nativo falhou', url: null });
      await ending;
    });
    expect(onFim).not.toHaveBeenCalled();
    expect(onErroParada).toHaveBeenCalledTimes(1);
    expect(onErroParada.mock.calls[0][0].message).toBe(m.composer_falha_gravacao());
    expect(recordDiagnostic).toHaveBeenCalledExactlyOnceWith({ evento: 'dictation.recording_failed', nivel: 'erro',
      codigo: 'recording_error', detalhe: 'stop nativo falhou' }, undefined);
    expect(fetch).not.toHaveBeenCalled();
    expect(removeStatusListener).toHaveBeenCalledTimes(1);
    expect(fakeRec.release).toHaveBeenCalledTimes(1);
    await h.unmount();
  });

  it('falha nativa durante a gravação interrompe e informa o erro sem entregar áudio', async () => {
    const onFim = vi.fn();
    const onErroParada = vi.fn();
    const h = mountHook({ onFim, onErroParada });
    await h.mount({ onFim, onErroParada });
    await act(async () => { await h.getHook().iniciar(); });
    await act(async () => { emitRecordingStatus({ hasError: true, error: 'microfone interrompido', url: null }); });
    expect(h.getHook().gravando).toBe(false);
    expect(onErroParada).toHaveBeenCalledTimes(1);
    expect(onErroParada.mock.calls[0][0].message).toBe(m.composer_falha_gravacao());
    expect(recordDiagnostic).toHaveBeenCalledExactlyOnceWith({ evento: 'dictation.recording_failed', nivel: 'erro',
      codigo: 'recording_error', detalhe: 'microfone interrompido' }, undefined);
    expect(onFim).not.toHaveBeenCalled();
    expect(fakeRec.stop).toHaveBeenCalledTimes(1);
    expect(removeStatusListener).toHaveBeenCalledTimes(1);
    await h.unmount();
  });

  it.each(['stop', 'status'] as const)('falha de forma finita quando %s não confirma a parada', async (missing) => {
    const stop = deferred<void>();
    fakeRec.stop = vi.fn(() => missing === 'stop' ? stop.promise : Promise.resolve());
    const onFim = vi.fn();
    const onErroParada = vi.fn();
    const h = mountHook({ onFim, onErroParada });
    await h.mount({ onFim, onErroParada });
    await act(async () => { await h.getHook().iniciar(); });
    let ending!: Promise<void>;
    await act(async () => { ending = h.getHook().parar('botao'); vi.advanceTimersByTime(4999); });
    expect(onFim).not.toHaveBeenCalled();
    expect(onErroParada).not.toHaveBeenCalled();
    expect(h.getHook().gravando).toBe(true);
    await act(async () => { vi.advanceTimersByTime(1); await ending; });
    expect(onFim).not.toHaveBeenCalled();
    expect(onErroParada).toHaveBeenCalledTimes(1);
    expect(onErroParada.mock.calls[0][0].cause).toBe('recording_status_timeout');
    expect(h.getHook().gravando).toBe(false);
    expect(removeStatusListener).toHaveBeenCalledTimes(1);
    expect(fakeRec.release).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
    stop.resolve();
    await h.unmount();
  });

  it('erro nativo e rejeição de prepare propagam o erro somente uma vez', async () => {
    fakeRec.prepareToRecordAsync = vi.fn(async () => {
      emitRecordingStatus({ hasError: true, error: 'erro nativo de preparação', url: null });
      throw new Error('prepare rejeitou');
    });
    const onFim = vi.fn();
    const onErroParada = vi.fn();
    const h = mountHook({ onFim, onErroParada });
    await h.mount({ onFim, onErroParada });
    await act(async () => { await expect(h.getHook().iniciar()).resolves.toBeUndefined(); });
    expect(onErroParada).toHaveBeenCalledTimes(1);
    expect(onErroParada.mock.calls[0][0].message).toBe(m.composer_falha_gravacao());
    expect(recordDiagnostic).toHaveBeenCalledExactlyOnceWith({ evento: 'dictation.recording_failed', nivel: 'erro',
      codigo: 'recording_error', detalhe: 'erro nativo de preparação' }, undefined);
    expect(onFim).not.toHaveBeenCalled();
    expect(fakeRec.record).not.toHaveBeenCalled();
    expect(fakeRec.release).toHaveBeenCalledTimes(1);
    await h.unmount();
  });

  it('falha ao liberar no cleanup é informada sem rejeição pendente', async () => {
    const released = deferred<void>();
    const erro = new Error('release falhou');
    fakeRec.release = vi.fn(() => { throw erro; });
    const onFim = vi.fn();
    const onErroParada = vi.fn(() => { released.resolve(); });
    const h = mountHook({ onFim, onErroParada });
    await h.mount({ onFim, onErroParada });
    await act(async () => { await h.getHook().iniciar(); });
    await h.unmount();
    await act(async () => { await released.promise; });
    expect(onErroParada).toHaveBeenCalledTimes(1);
    expect(onErroParada).toHaveBeenCalledWith(erro);
    expect(removeStatusListener).toHaveBeenCalledTimes(1);
  });

  it('desmontar gravando (troca de conversa) para e entrega o áudio ao onFim de quem gravou', async () => {
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    await h.unmount();
    await act(async () => { await vi.runAllTimersAsync(); });
    expect(onFim).toHaveBeenCalledTimes(1);
    expect(onFim.mock.calls[0][1]).toBe('escondeu');
  });
});

describe('useDitado — medidor e segundo plano', () => {
  it('aplica ao teto nativo a configuração relida antes de abrir a captação', async () => {
    const refresh = deferred<void>();
    const old = useDitadoEstiloStore.getState();
    useDitadoEstiloStore.setState({ ceilingMs: 60_000, refreshCeiling: () => refresh.promise });
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    try {
      await h.mount({ onFim });
      let starting!: Promise<void>;
      await act(async () => { starting = h.getHook().iniciar(); });
      expect(fakeRec.record).not.toHaveBeenCalled();
      await act(async () => {
        useDitadoEstiloStore.setState({ ceilingMs: 15_000 });
        refresh.resolve();
        await starting;
      });
      expect(fakeRec.record).toHaveBeenCalledExactlyOnceWith({ forDuration: 15 });
    } finally {
      refresh.resolve();
      await h.unmount();
      useDitadoEstiloStore.setState({ ceilingMs: old.ceilingMs, refreshCeiling: old.refreshCeiling });
    }
  });

  it('Android antigo não tenta abrir o serviço que só existe no native-3', async () => {
    environment.runtime = 'native-2';
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    expect(setAudioMode).toHaveBeenCalledWith(expect.objectContaining({ allowsBackgroundRecording: false }));
    expect(requestNotificationPermissionsAsync).not.toHaveBeenCalled();
    await act(async () => { changeAppState('background'); });
    expect(onFim.mock.calls[0]?.[1]).toBe('escondeu');
    await h.unmount();
  });

  it('iOS já tem áudio em segundo plano e não pede notificações', async () => {
    environment.os = 'ios';
    environment.runtime = 'native-2';
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    expect(setAudioMode).toHaveBeenCalledWith(expect.objectContaining({ allowsBackgroundRecording: true }));
    expect(requestNotificationPermissionsAsync).not.toHaveBeenCalled();
    await act(async () => { changeAppState('background'); });
    expect(fakeRec.stop).not.toHaveBeenCalled();
    await act(async () => { changeAppState('active'); await h.getHook().parar('botao'); });
    await h.unmount();
  });

  it('notificação negada no Android grava só com o app na frente', async () => {
    vi.mocked(requestNotificationPermissionsAsync).mockResolvedValueOnce({ granted: false } as never);
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    expect(setAudioMode).toHaveBeenCalledWith(expect.objectContaining({ allowsBackgroundRecording: false }));
    expect(h.getHook().gravando).toBe(true);
    await act(async () => { changeAppState('background'); });
    expect(fakeRec.stop).toHaveBeenCalledTimes(1);
    expect(onFim.mock.calls[0]?.[1]).toBe('escondeu');
    expect(recordDiagnostic).toHaveBeenCalledWith({ evento: 'dictation.background_unavailable', nivel: 'aviso',
      codigo: 'notifications_denied' }, undefined);
    expect(toastAviso).toHaveBeenCalledExactlyOnceWith(m.composer_ditado_so_na_frente());
    // Segunda gravação ainda sem permissão: o aviso não se repete.
    vi.mocked(requestNotificationPermissionsAsync).mockResolvedValueOnce({ granted: false } as never);
    await act(async () => { changeAppState('active'); await h.getHook().iniciar(); });
    await act(async () => { await h.getHook().parar('botao'); });
    expect(toastAviso).toHaveBeenCalledOnce();
    await h.unmount();
  });

  it('servidor lento não segura o início: grava com o teto anterior e registra', async () => {
    const old = useDitadoEstiloStore.getState();
    useDitadoEstiloStore.setState({ ceilingMs: 60_000, refreshCeiling: () => new Promise<void>(() => {}) });
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    try {
      await h.mount({ onFim });
      let starting!: Promise<void>;
      await act(async () => { starting = h.getHook().iniciar(); });
      expect(fakeRec.record).not.toHaveBeenCalled();
      await act(async () => { vi.advanceTimersByTime(1_000); await starting; });
      expect(fakeRec.record).toHaveBeenCalledExactlyOnceWith({ forDuration: 60 });
      expect(h.getHook().gravando).toBe(true);
      expect(recordDiagnostic).toHaveBeenCalledWith({ evento: 'dictation.ceiling_stale', nivel: 'aviso',
        codigo: 'ceiling_refresh_timeout' }, undefined);
      await act(async () => { await h.getHook().parar('botao'); });
    } finally {
      await h.unmount();
      useDitadoEstiloStore.setState({ ceilingMs: old.ceilingMs, refreshCeiling: old.refreshCeiling });
    }
  });

  it('o gravador nativo recebe o teto mesmo sem timers JS em segundo plano', async () => {
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    const old = useDitadoEstiloStore.getState();
    useDitadoEstiloStore.setState({ ceilingMs: 180_000, refreshCeiling: async () => {} });
    try {
      await h.mount({ onFim });
      await act(async () => { await h.getHook().iniciar(); });
      expect(fakeRec.record).toHaveBeenCalledWith({ forDuration: 180 });
      await act(async () => { changeAppState('background'); });
      // Timers JS suspensos: só o relógio de parede anda, e o nativo termina um pouco antes do teto JS.
      vi.setSystemTime(Date.now() + 179_700);
      await act(async () => { emitRecordingStatus(); });
      expect(h.getHook().gravando).toBe(false);
      expect(onFim).toHaveBeenCalledTimes(1);
      expect(onFim.mock.calls[0]?.[1]).toBe('teto');
      await h.unmount();
    } finally {
      useDitadoEstiloStore.setState({ ceilingMs: old.ceilingMs, refreshCeiling: old.refreshCeiling });
    }
  });

  it('fim nativo bem antes do teto é interrupção e não teto', async () => {
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    const old = useDitadoEstiloStore.getState();
    useDitadoEstiloStore.setState({ ceilingMs: 180_000, refreshCeiling: async () => {} });
    try {
      await h.mount({ onFim });
      await act(async () => { await h.getHook().iniciar(); });
      await act(async () => { changeAppState('background'); });
      vi.setSystemTime(Date.now() + 10_000);
      await act(async () => { emitRecordingStatus(); });
      expect(onFim).toHaveBeenCalledTimes(1);
      expect(onFim.mock.calls[0]?.[1]).toBe('escondeu');
      await h.unmount();
    } finally {
      useDitadoEstiloStore.setState({ ceilingMs: old.ceilingMs, refreshCeiling: old.refreshCeiling });
    }
  });

  it('publica amostras sem redesenhar o Composer a cada nível', async () => {
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    const renders = h.renderCount();
    for (const metering of [-20, -18, -25]) {
      fakeRec.getStatus.mockReturnValue({ metering });
      await act(async () => { vi.advanceTimersByTime(55); });
    }
    expect(h.renderCount()).toBe(renders);
    expect(h.getHook().rms.value).toBeCloseTo(0.28117);
    await act(async () => { await h.getHook().parar('botao'); });
    await h.unmount();
  });

  it('continua gravando ao abrir Fotos e finaliza por botão ao voltar', async () => {
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    await act(async () => { changeAppState('inactive'); changeAppState('background'); });
    expect(fakeRec.stop).not.toHaveBeenCalled();
    expect(h.getHook().gravando).toBe(true);
    fakeRec.getStatus.mockReturnValue({ isRecording: true, metering: -20 });
    await act(async () => { changeAppState('active'); await h.getHook().parar('botao'); });
    expect(onFim.mock.calls[0]?.[1]).toBe('botao');
    await h.unmount();
  });

  it('ao voltar com captação pausada conserva o áudio e fecha a gravação', async () => {
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    await act(async () => { changeAppState('background'); });
    fakeRec.getStatus.mockReturnValue({ isRecording: false, metering: -20 });
    await act(async () => { changeAppState('active'); });
    expect(h.getHook().gravando).toBe(false);
    expect(onFim).toHaveBeenCalledTimes(1);
    expect(onFim.mock.calls[0]?.[1]).toBe('escondeu');
    await h.unmount();
  });
});

describe('useDitado — VAD e teto', () => {
  it('encerra por silêncio depois de ouvir fala usando o RMS bruto', async () => {
    fakeRec.getStatus.mockReturnValue({ metering: -20 });
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => {
      await h.getHook().iniciar();
      vi.advanceTimersByTime(600);
    });
    expect(h.getHook().rms.value).toBeCloseTo(0.5);
    fakeRec.getStatus.mockReturnValue({ metering: -160 });
    await act(async () => { vi.advanceTimersByTime(2300); });
    expect(onFim.mock.calls[0]?.[1]).toBe('silencio');
    expect(fakeRec.stop).toHaveBeenCalledTimes(1);
    await h.unmount();
  });

  it('fora da tela a fala que termina encerra pelo VAD como escondeu', async () => {
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => { await h.getHook().iniciar(); });
    await act(async () => { changeAppState('background'); });
    fakeRec.getStatus.mockReturnValue({ metering: -20 });
    await act(async () => { vi.advanceTimersByTime(600); });
    expect(fakeRec.stop).not.toHaveBeenCalled();
    fakeRec.getStatus.mockReturnValue({ metering: -160 });
    await act(async () => { vi.advanceTimersByTime(2300); });
    expect(fakeRec.stop).toHaveBeenCalledTimes(1);
    expect(onFim.mock.calls[0]?.[1]).toBe('escondeu');
    await h.unmount();
  });

  it('sem metering não usa VAD e encerra pelo teto lido da configuração', async () => {
    // A releitura da config no começo da gravação traria outro teto; aqui o teto é o da store.
    const refreshCeiling = useDitadoEstiloStore.getState().refreshCeiling;
    useDitadoEstiloStore.setState({ ceilingMs: 180_000, refreshCeiling: async () => {} });
    fakeRec.getStatus.mockReturnValue({});
    const onFim = vi.fn();
    const h = mountHook({ onFim });
    await h.mount({ onFim });
    await act(async () => {
      await h.getHook().iniciar();
      vi.advanceTimersByTime(5000);
    });
    expect(h.getHook().rms.value).toBe(0);
    expect(fakeRec.stop).not.toHaveBeenCalled();
    await act(async () => { vi.advanceTimersByTime(175000); });
    expect(onFim.mock.calls[0]?.[1]).toBe('teto');
    expect(fakeRec.stop).toHaveBeenCalledTimes(1);
    await h.unmount();
    useDitadoEstiloStore.setState({ refreshCeiling });
    useDitadoEstiloStore.getState()._zerarParaTeste();
  });
});
