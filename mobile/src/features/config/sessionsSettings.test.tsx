// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AutoHideResult, Server } from '@hangar/core';
import * as m from '../../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const a: Server = { id: 'a', label: 'Casa', baseUrl: 'https://a.test', token: 'ta' };
const b: Server = { id: 'b', label: 'VPS', baseUrl: 'https://b.test', token: 'tb' };
const calls = vi.hoisted(() => ({
  read: vi.fn(), save: vi.fn(), dismissTo: vi.fn(), openHidden: vi.fn(),
  servers: [] as Server[], rows: [] as Array<{ hidden_from_list?: boolean; state: string }>,
}));

vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  readAutoHideAll: calls.read,
  saveAutoHideAll: calls.save,
}));
vi.mock('../../stores/servers', () => ({
  useServers: (selector: (s: { servers: Server[]; ready: boolean }) => unknown) => selector({ servers: calls.servers, ready: true }),
}));
vi.mock('../../stores/sessions', () => ({
  useSessions: Object.assign((selector: (s: { rows: typeof calls.rows }) => unknown) => selector({ rows: calls.rows }),
    { getState: () => ({ retain: () => () => {} }) }),
}));
vi.mock('../sessions/hiddenSectionPref', () => ({ setHiddenSectionOpen: calls.openHidden }));
vi.mock('expo-router', () => ({ useRouter: () => ({ dismissTo: calls.dismissTo }), Stack: { Screen: () => null } }));
vi.mock('./Pagina', () => ({ Pagina: ({ children }: { children: ReactNode }) => createElement('main', null, children) }));
vi.mock('./SectionCard', () => ({ SectionCard: ({ children }: { children: ReactNode }) => createElement('article', null, children) }));
vi.mock('./InfoNotice', () => ({ InfoNotice: ({ text }: { text: string }) => createElement('p', { 'data-info': true }, text) }));
vi.mock('../machines/parts', () => ({ Notice: ({ text, error }: { text: string; error?: boolean }) => createElement('p', { 'data-error': String(!!error) }, text) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/haptics', () => ({ haptic: { selection: () => {}, success: () => {}, error: () => {} } }));
vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(),
  Switch: ({ value, onValueChange, accessibilityLabel }: { value: boolean; onValueChange: (v: boolean) => void; accessibilityLabel: string }) =>
    createElement('button', { role: 'switch', 'aria-checked': value, 'aria-label': accessibilityLabel, onClick: () => onValueChange(!value) }),
  TextInput: ({ value, onChangeText, onEndEditing }: { value: string; onChangeText: (v: string) => void; onEndEditing: () => void }) =>
    createElement('input', { 'data-hours': true, value, onInput: (e: { currentTarget: HTMLInputElement }) => onChangeText(e.currentTarget.value), onBlur: onEndEditing }),
}));

import SessionsSettings from '../../../app/config/sessions';

const roots = new Set<Root>();
async function mount() {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.add(root);
  await act(async () => root.render(createElement(SessionsSettings)));
  return container;
}
const byLabel = (c: HTMLElement, label: string) => c.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
const ok = (serverId: string, enabled: boolean, after_minutes = 180): AutoHideResult => ({ serverId, ok: true, config: { enabled, after_minutes } });

beforeEach(() => {
  vi.resetAllMocks();
  calls.servers = [a, b];
  calls.rows = [];
});
afterEach(async () => {
  for (const root of roots) await act(async () => root.unmount());
  roots.clear();
});

describe('página Sessões', () => {
  it('sem máquina própria mostra o vazio; lendo, o carregando', async () => {
    calls.servers = [];
    expect((await mount()).textContent).toContain(m.maquinas_vazio());
    calls.servers = [a];
    calls.read.mockReturnValue(new Promise(() => {}));
    expect((await mount()).textContent).toContain(m.native_server_loading());
  });

  it('todas as leituras falhando: o erro de cada máquina e tentar de novo', async () => {
    calls.read.mockResolvedValue([{ serverId: 'a', ok: false, error: 'fora do ar' }, { serverId: 'b', ok: false, error: 'recusou' }]);
    const c = await mount();
    expect(c.textContent).toContain('fora do ar');
    expect(c.textContent).toContain('recusou');
    expect(c.querySelector('[role="switch"]')).toBeNull();
    calls.read.mockResolvedValue([ok('a', false), ok('b', false)]);
    await act(async () => byLabel(c, m.mobile_machines_retry())!.click());
    expect(calls.read).toHaveBeenCalledTimes(2);
    expect(c.querySelector('[role="switch"]')).not.toBeNull();
  });

  it('aponta a máquina diferente e "Aplicar em todas" grava a configuração da maioria em todas', async () => {
    calls.servers = [a, b, { ...b, id: 'c', label: 'Lab' }];
    calls.read.mockResolvedValue([ok('a', true, 240), ok('b', false), ok('c', true, 240)]);
    calls.save.mockResolvedValue([ok('a', true, 240), { serverId: 'b', ok: false, error: 'sem rota' }, ok('c', true, 240)]);
    const c = await mount();
    expect(c.querySelector('[role="switch"]')?.getAttribute('aria-checked')).toBe('true');
    expect(c.querySelector<HTMLInputElement>('[data-hours]')?.value).toBe('4');
    expect(c.textContent).toContain(m.session_auto_hide_differs({ servers: 'VPS' }));
    await act(async () => byLabel(c, m.session_auto_hide_apply_all())!.click());
    expect(calls.save).toHaveBeenCalledWith(calls.servers, { enabled: true, after_minutes: 240 });
    expect(c.textContent).toContain(m.session_auto_hide_saved_on({ server: 'Casa' }));
    expect(c.textContent).toContain(m.session_auto_hide_not_saved_on({ server: 'VPS', error: 'sem rota' }));
    expect(c.textContent).not.toContain(m.session_auto_hide_differs({ servers: 'VPS' }));
  });

  it('ligar e mudar o prazo gravam em todas, em horas inteiras de 1 a 168', async () => {
    calls.read.mockResolvedValue([ok('a', false), ok('b', false)]);
    calls.save.mockImplementation(async (_servers: Server[], cfg: { enabled: boolean; after_minutes: number }) => [
      { serverId: 'a', ok: true, config: cfg }, { serverId: 'b', ok: true, config: cfg }]);
    const c = await mount();
    expect(c.querySelector<HTMLInputElement>('[data-hours]')?.value).toBe('3');
    await act(async () => c.querySelector<HTMLButtonElement>('[role="switch"]')!.click());
    expect(calls.save).toHaveBeenLastCalledWith([a, b], { enabled: true, after_minutes: 180 });
    const input = c.querySelector<HTMLInputElement>('[data-hours]')!;
    await act(async () => {
      input.value = '500';
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    // O onBlur do React escuta `focusout`.
    await act(async () => input.dispatchEvent(new FocusEvent('focusout', { bubbles: true })));
    expect(calls.save).toHaveBeenLastCalledWith([a, b], { enabled: true, after_minutes: 168 * 60 });
  });

  it('"Ver sessões ocultas" abre a lista com a seção expandida', async () => {
    calls.read.mockResolvedValue([ok('a', false), ok('b', false)]);
    calls.rows = [{ state: 'idle', hidden_from_list: true }, { state: 'dead', hidden_from_list: true }, { state: 'idle' }];
    const c = await mount();
    await act(async () => byLabel(c, m.session_hidden_see({ n: 1 }))!.click());
    expect(calls.openHidden).toHaveBeenCalledWith(true);
    expect(calls.dismissTo).toHaveBeenCalledWith('/?hidden=1');
  });
});
