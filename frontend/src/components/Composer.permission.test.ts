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

  it('sem ciclo em cache, o atalho SONDA (sondar=1) em vez de morrer calado', async () => {
    vi.mocked(api.getPermissionModes)
      .mockResolvedValueOnce({ current: 'plan', modes: [], sondavel: true })          // poll da montagem
      .mockResolvedValueOnce({ current: 'plan', modes: ['plan', 'auto'], sondavel: true }); // sonda do atalho
    vi.mocked(api.setPermissionMode).mockResolvedValue({ mode: 'auto', current: 'auto' });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const harness = mount(Harness as never, { target: el });
    await tick(); await tick(); await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    document.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyP', key: 'P', altKey: true, shiftKey: true, bubbles: true }));
    await tick(); await new Promise((r) => setTimeout(r, 0)); await tick();
    await new Promise((r) => setTimeout(r, 0)); await tick();
    expect(vi.mocked(api.getPermissionModes).mock.calls.at(-1)).toEqual(['perm-test', true]);
    expect(vi.mocked(api.setPermissionMode)).toHaveBeenCalledWith('perm-test', 'auto');

    unmount(harness as never);
    document.body.innerHTML = '';
  });

  it('Shift+Tab com o foco no campo cicla; Ctrl+L foca o campo de qualquer lugar', async () => {
    vi.mocked(api.getPermissionModes).mockResolvedValue({ current: 'plan', modes: ['plan', 'auto'], sondavel: true });
    vi.mocked(api.setPermissionMode).mockResolvedValue({ mode: 'auto', current: 'auto' });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const harness = mount(Harness as never, { target: el });
    await tick(); await tick(); await tick();
    await new Promise((r) => setTimeout(r, 0));
    await tick();

    const ta = el.querySelector('textarea')!;
    ta.blur();
    expect(document.activeElement).not.toBe(ta);
    document.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyL', key: 'l', ctrlKey: true, bubbles: true }));
    expect(document.activeElement).toBe(ta);

    const ev = new KeyboardEvent('keydown', { key: 'Tab', code: 'Tab', shiftKey: true, bubbles: true, cancelable: true });
    ta.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    await tick(); await new Promise((r) => setTimeout(r, 0)); await tick();
    expect(vi.mocked(api.setPermissionMode)).toHaveBeenCalledWith('perm-test', 'auto');

    unmount(harness as never);
    document.body.innerHTML = '';
  });
});
