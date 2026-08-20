// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import Harness from './Composer.permission.harness.svelte';
import * as api from '../lib/api';

vi.mock('../lib/api', () => ({
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
});
