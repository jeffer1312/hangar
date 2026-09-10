// @vitest-environment happy-dom
import { beforeEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import CodexVoice from './CodexVoice.svelte';
import * as m from '../paraglide/messages';

const fake = vi.hoisted(() => ({
  changed: (_state: string) => {}, stop: vi.fn(),
  loadVoices: vi.fn<() => Promise<string[]>>(),
}));
vi.mock('../lib/auth', () => ({
  getActiveId: () => 'local', listServers: () => [{ id: 'local', baseUrl: '', token: 'test' }],
}));
vi.mock('@hangar/core', async original => ({
  ...await original<object>(), getCodexVoicesForServer: () => fake.loadVoices(),
}));
vi.mock('../lib/codexVoice', () => ({ CodexVoiceCall: class {
  constructor(_audio: unknown, changed: typeof fake.changed) { fake.changed = changed; }
  start() { fake.changed('connecting'); }
  setMuted() {}
  stop() { fake.stop(); fake.changed('idle'); }
} }));

async function flush() { await tick(); await new Promise(resolve => setTimeout(resolve, 0)); await tick(); }
const button = (label: string) => document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
beforeEach(() => {
  localStorage.clear(); fake.stop.mockClear();
  fake.loadVoices.mockReset().mockResolvedValue(['coral', 'sol']);
});

it('guarda a voz escolhida e restaura ao reabrir a sessão', async () => {
  const target = document.createElement('div'); document.body.append(target);
  const props = { sessionName: 'voice-test', onPrepare: vi.fn(), onBusyChange: vi.fn() };
  let component = mount(CodexVoice, { target, props });
  try {
    await flush(); button(m.codex_voice_title()).click(); await flush();
    const select = document.querySelector('select')!;
    select.value = 'sol'; select.dispatchEvent(new Event('change', { bubbles: true }));
    await flush();
    expect(localStorage.getItem('cp_codex_voice')).toBe('sol');
    await unmount(component);
    let loaded!: (voices: string[]) => void;
    fake.loadVoices.mockImplementationOnce(() => new Promise(resolve => { loaded = resolve; }));
    component = mount(CodexVoice, { target, props });
    await flush(); button(m.codex_voice_title()).click(); await flush();
    loaded(['coral', 'sol']); await flush();
    expect(document.querySelector('select')!.value).toBe('sol');
  } finally { await unmount(component); target.remove(); }
});

it('conectar recolhe o modal sem encerrar; fechar controles também preserva a chamada', async () => {
  const target = document.createElement('div'); document.body.append(target);
  const busy = vi.fn();
  const component = mount(CodexVoice, { target, props: {
    sessionName: 'voice-test', onPrepare: vi.fn(), onBusyChange: busy,
  } });
  try {
    await flush();
    button(m.codex_voice_title()).click(); await flush();
    button(m.codex_voice_connect()).click(); await flush();
    fake.changed('connected'); await flush();
    expect(button(m.codex_voice_active()).getAttribute('aria-expanded')).toBe('false');
    expect(busy).toHaveBeenLastCalledWith(true);
    expect(fake.stop).not.toHaveBeenCalled();

    button(m.codex_voice_active()).click(); await flush();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await flush();
    expect(button(m.codex_voice_active()).getAttribute('aria-expanded')).toBe('false');
    expect(fake.stop).not.toHaveBeenCalled();

    button(m.codex_voice_active()).click(); await flush();
    button(m.codex_voice_stop()).click(); await flush();
    expect(fake.stop).toHaveBeenCalledOnce();
    expect(busy).toHaveBeenLastCalledWith(false);
  } finally { await unmount(component); target.remove(); }
});
