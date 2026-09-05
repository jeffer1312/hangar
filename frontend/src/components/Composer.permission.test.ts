// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import Harness from './Composer.permission.harness.svelte';
import * as api from '@hangar/core';

vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: [], sondavel: true }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  getCommands: vi.fn().mockResolvedValue([]),
  setModelEffort: vi.fn(),
  uploadFile: vi.fn(),
  transcribeFile: vi.fn(),
  getCodexModels: vi.fn().mockResolvedValue([]),
  getPiModels: vi.fn().mockResolvedValue([]),
  // O prefetch do Composer (cache de catálogo) chama estes ao montar — sem eles no mock o
  // módulo nem sobe.
  getKimiModels: vi.fn().mockResolvedValue({ models: [], default: null }),
  getModelOptions: vi.fn().mockResolvedValue({ kind: 'claude', models: [] }),
  isTimeoutError: vi.fn(() => false),
  isAbortError: vi.fn(() => false),
}));

describe('Composer — pílula de permissão reage a sessionState', () => {
  beforeEach(() => vi.clearAllMocks());

  it('monta com working e rele ao virar idle', async () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    const harness = mount(Harness as never, { target: el }) as unknown as { setState: (s: string) => void };
    // espera o $effect inicial disparar
    await tick(); await tick(); await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    expect(vi.mocked(api.getPermissionModes)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.getPermissionModes)).toHaveBeenCalledWith('perm-test', false);

    // muda working -> idle: deve disparar segunda leitura
    (harness as unknown as { setState: (v: string) => void }).setState('idle');
    await tick(); await tick(); await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    expect(vi.mocked(api.getPermissionModes)).toHaveBeenCalledTimes(2);
    // segunda chamada também sem sondar
    expect(vi.mocked(api.getPermissionModes).mock.calls[1]).toEqual(['perm-test', false]);

    unmount(harness as never);
    document.body.innerHTML = '';
  });

  it('Alt+Shift+P passa pro PRÓXIMO modo do ciclo vivo (e dá a volta no fim)', async () => {
    vi.mocked(api.getPermissionModes).mockResolvedValue({ current: 'acceptEdits', modes: ['plan', 'acceptEdits'], sondavel: true });
    vi.mocked(api.setPermissionMode).mockResolvedValue({ mode: 'plan', current: 'plan' });
    // desktop: o atalho é só de tela larga
    window.matchMedia = ((q: string) => ({ matches: true, media: q, addEventListener() {}, removeEventListener() {} })) as never;
    const el = document.createElement('div');
    document.body.appendChild(el);
    const harness = mount(Harness as never, { target: el });
    await tick(); await tick(); await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    document.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyP', key: 'P', altKey: true, shiftKey: true, bubbles: true }));
    await tick(); await new Promise((r) => setTimeout(r, 0)); await tick();
    // acceptEdits é o último da lista de 2 -> volta pro primeiro
    expect(vi.mocked(api.setPermissionMode)).toHaveBeenCalledWith('perm-test', 'plan');

    // sem Shift não é o atalho
    document.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyP', key: 'p', altKey: true, bubbles: true }));
    await tick(); await new Promise((r) => setTimeout(r, 0));
    expect(vi.mocked(api.setPermissionMode)).toHaveBeenCalledTimes(1);

    unmount(harness as never);
    document.body.innerHTML = '';
  });
});
