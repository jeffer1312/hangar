/**
 * @vitest-environment happy-dom
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import React, { act } from 'react';
import { createRoot } from 'react-dom/client';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const { fakeRec } = vi.hoisted(() => {
  const fakeRec: {
    uri: string | null;
    stop: ReturnType<typeof vi.fn>;
    prepareToRecordAsync: ReturnType<typeof vi.fn>;
    record: ReturnType<typeof vi.fn>;
    getStatus: ReturnType<typeof vi.fn>;
  } = {
    uri: 'file://fake.m4a',
    stop: vi.fn(async () => {}),
    prepareToRecordAsync: vi.fn(async () => {}),
    record: vi.fn(() => {}),
    getStatus: vi.fn(() => ({ metering: -160 })),
  };
  return { fakeRec };
});

vi.mock('expo-audio', () => ({
  useAudioRecorder: () => fakeRec,
  RecordingPresets: { HIGH_QUALITY: {} },
  requestRecordingPermissionsAsync: vi.fn(async () => ({ granted: true })),
  setAudioModeAsync: vi.fn(async () => {}),
}));

vi.mock('react-native', async () => {
  const actual = (await vi.importActual<typeof import('../../__mocks__/react-native')>('../../__mocks__/react-native')) as Record<string, unknown>;
  return {
    ...actual,
    AppState: {
      addEventListener: vi.fn(() => ({ remove: vi.fn() })),
      currentState: 'active',
    },
  };
});

import { useDitado } from './useDitado';
import type { MotivoFim } from '@hangar/core';

function mountHook(opts: { onFim: (f: File, m: MotivoFim) => void; onErroParada?: (e: Error) => void }) {
  let hook: ReturnType<typeof useDitado> | null = null;
  function Comp(p: typeof opts) {
    hook = useDitado(p);
    return null;
  }
  const container = document.createElement('div');
  document.body.appendChild(container);
  const root = createRoot(container);
  return {
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
  document.body.innerHTML = '';
  fakeRec.uri = 'file://fake.m4a';
  fakeRec.stop = vi.fn(async () => {});
  fakeRec.prepareToRecordAsync = vi.fn(async () => {});
  fakeRec.record = vi.fn(() => {});
  fakeRec.getStatus = vi.fn(() => ({ metering: -160 }));
  vi.stubGlobal('fetch', vi.fn(async () => ({ blob: async () => new Blob(['audio'], { type: 'audio/m4a' }) } as unknown as Response)));
});

afterEach(async () => {
  document.body.innerHTML = '';
  vi.unstubAllGlobals();
  // vi.restoreAllMocks limpa fns do fakeRec; recria
  // mas precisa preservar hoisted fakeRec
  if (!fakeRec.stop || typeof (fakeRec.stop as unknown as { mockReset?: unknown }).mockReset !== 'function') {
    fakeRec.stop = vi.fn(async () => {});
  }
  if (!fakeRec.prepareToRecordAsync || typeof (fakeRec.prepareToRecordAsync as unknown as { mockReset?: unknown }).mockReset !== 'function') {
    fakeRec.prepareToRecordAsync = vi.fn(async () => {});
  }
  if (!fakeRec.record || typeof (fakeRec.record as unknown as { mockReset?: unknown }).mockReset !== 'function') {
    fakeRec.record = vi.fn(() => {});
  }
  if (!fakeRec.getStatus || typeof (fakeRec.getStatus as unknown as { mockReset?: unknown }).mockReset !== 'function') {
    fakeRec.getStatus = vi.fn(() => ({ metering: -160 }));
  }
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
    fakeRec.stop = vi.fn(async () => {});

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
    fakeRec.stop = vi.fn(async () => {});

    await act(async () => {
      await h.getHook().parar('botao');
    });

    expect(onFim).toHaveBeenCalledTimes(1);
    expect(onErroParada).not.toHaveBeenCalled();
    const [file, motivo] = onFim.mock.calls[0];
    expect(file).toBeInstanceOf(File);
    expect(motivo).toBe('botao');

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
    // força UM re-render com NOVA inline arrow (nova identidade) — como o Composer faz a cada setRms/setText
    await h.mount({ onFim, onErroParada: (e) => onErro(e) });
    await act(async () => {
      await h.getHook().parar('botao');
    });
    expect(onFim).toHaveBeenCalledTimes(1);
    expect(onErro).not.toHaveBeenCalled();
    await h.unmount();
  });
});
