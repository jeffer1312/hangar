// @vitest-environment happy-dom
import React, { act, useEffect, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const calls = vi.hoisted(() => ({
  openDrawer: vi.fn(), closeDrawer: vi.fn(), dismiss: vi.fn(), push: vi.fn(),
  focused: true, back: null as null | (() => boolean),
  params: {} as { compose?: string | string[] }, setParams: vi.fn(),
  pan: null as null | { onMoveShouldSetPanResponder: (e: unknown, g: { dx: number; dy: number }) => boolean; onPanResponderRelease: (e: unknown, g: { dy: number }) => void },
}));
vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(),
  Keyboard: { dismiss: calls.dismiss },
  BackHandler: { addEventListener: (_event: string, callback: () => boolean) => {
    calls.back = callback;
    return { remove: () => { if (calls.back === callback) calls.back = null; } };
  } },
  PanResponder: { create: (handlers: typeof calls.pan) => { calls.pan = handlers; return { panHandlers: {} }; } },
}));
vi.mock('expo-router', () => ({
  useRouter: () => ({ push: calls.push, setParams: calls.setParams }),
  useLocalSearchParams: () => calls.params,
  useFocusEffect: (callback: () => void | (() => void)) => useEffect(() => calls.focused ? callback() : undefined, [callback, calls.focused]),
}));
vi.mock('../../stores/servers', async () => {
  const { create } = await import('zustand');
  return { useServers: create(() => ({ activeId: 'server' })) };
});
vi.mock('../../stores/sessions', async () => {
  const { create } = await import('zustand');
  return { useSessions: create(() => ({ rows: [{ state: 'working' }, { state: 'idle' }, { state: 'dead' }] })) };
});
vi.mock('../../stores/aparencia', async () => {
  const { create } = await import('zustand');
  return { useAparencia: create(() => ({ homeScreen: 'sessions' })) };
});
vi.mock('../../ui/Screen', () => ({ Screen: ({ children }: { children: ReactNode }) => children }));
vi.mock('../../ui/Icon', () => ({ Icon: ({ name }: { name: string }) => React.createElement('span', { 'data-icon': name }) }));
vi.mock('../voice/LiveVoiceChatButton', () => ({ LiveVoiceChatButton: () => null }));
vi.mock('../sessions/SessionList', () => ({ SessionList: ({ variant }: { variant?: string }) => React.createElement('div', { 'data-list': variant }) }));
vi.mock('../sessions/ServerSheet', () => ({ ServerSheet: () => null }));
vi.mock('../sessions/SessionsDrawer', () => ({
  SessionsDrawer: ({ ref, children, onNewConversation }: { ref: React.RefObject<unknown>; children: ReactNode; onNewConversation?: () => void }) => {
    React.useImperativeHandle(ref, () => ({ openDrawer: calls.openDrawer, closeDrawer: calls.closeDrawer }));
    return React.createElement('div', null, children,
      React.createElement('button', { 'aria-label': 'drawer-new', onClick: onNewConversation }));
  },
}));
vi.mock('./CreateSessionSheet', () => ({
  CreateSessionSheet: ({ home }: { home?: { mode: string; focusRequest: number; sessions: ReactNode; onNewConversation: (focus: boolean) => void } }) =>
    React.createElement('div', { 'data-mode': home?.mode, 'data-focus': home?.focusRequest }, home?.sessions,
      React.createElement('button', { 'aria-label': 'open-text', onClick: () => home?.onNewConversation(true) }),
      React.createElement('button', { 'aria-label': 'open-mic', onClick: () => home?.onNewConversation(false) })),
}));
vi.mock('../../paraglide/messages', () => ({
  native_sessions: () => 'Sessões', maquinas_este_aparelho: () => 'machines', config_modal_titulo: () => 'settings',
}));

import Index from '../../../app/index';
import { useAparencia } from '../../stores/aparencia';
let root: Root;
let container: HTMLDivElement;
const click = (label: string) => container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!.click();
const mode = () => container.querySelector('[data-mode]')?.getAttribute('data-mode');
beforeEach(() => {
  vi.clearAllMocks();
  calls.focused = true;
  calls.back = null;
  calls.params = {};
  calls.setParams.mockImplementation((params) => { calls.params = { ...calls.params, ...params }; });
  useAparencia.setState({ homeScreen: 'sessions' });
  container = document.createElement('div');
  root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); });

describe('inicial Expo', () => {
  it('retorno explícito do chat abre criação na mesma inicial só ao ganhar foco e consome compose', async () => {
    calls.focused = false;
    calls.params = { compose: '1' };
    await act(async () => root.render(React.createElement(Index)));
    expect(mode()).toBe('sessions');
    const form = container.querySelector('[data-mode]');
    calls.focused = true;
    await act(async () => root.render(React.createElement(Index)));
    expect(mode()).toBe('newConversation');
    expect(container.querySelector('[data-mode]')).toBe(form);
    expect(form!.getAttribute('data-focus')).toBe('1');
    expect(calls.setParams).toHaveBeenCalledExactlyOnceWith({ compose: undefined });
    expect(calls.push).not.toHaveBeenCalled();
    await act(async () => click('Sessões'));
    calls.focused = false;
    await act(async () => root.render(React.createElement(Index)));
    calls.focused = true;
    await act(async () => root.render(React.createElement(Index)));
    expect(mode()).toBe('sessions');
    expect(calls.setParams).toHaveBeenCalledTimes(1);
  });

  it('abre em sessões com contador vivo e mantém a criação montada ao abrir e voltar', async () => {
    await act(async () => root.render(React.createElement(Index)));
    expect(mode()).toBe('sessions');
    expect(container.querySelector('[data-list="home"]')).not.toBeNull();
    expect(container.querySelector('[data-icon="PanelLeft"]')).not.toBeNull();
    expect(container.textContent).toContain('Sessões2');
    const form = container.querySelector('[data-mode]');
    await act(async () => click('Sessões'));
    expect(calls.openDrawer).toHaveBeenCalledTimes(1);
    await act(async () => click('open-text'));
    expect(mode()).toBe('newConversation');
    expect(container.querySelector('[data-mode]')).toBe(form);
    expect(container.querySelector('[data-icon="ChevronLeft"]')).not.toBeNull();
    expect(container.querySelector('[data-mode]')!.getAttribute('data-focus')).toBe('1');
    await act(async () => click('Sessões'));
    expect(mode()).toBe('sessions');
    expect(calls.dismiss).toHaveBeenCalled();
  });

  it('preferência Nova conversa e botão voltar Android respeitam foco da rota', async () => {
    useAparencia.setState({ homeScreen: 'newConversation' });
    await act(async () => root.render(React.createElement(Index)));
    expect(mode()).toBe('newConversation');
    expect(container.querySelector('[data-mode]')!.getAttribute('data-focus')).toBe('1');
    await act(async () => { expect(calls.back?.()).toBe(true); });
    expect(mode()).toBe('sessions');
    expect(calls.back?.()).toBe(false);
    calls.focused = false;
    await act(async () => root.render(React.createElement(Index)));
    expect(calls.back).toBeNull();
  });

  it('gaveta abre Nova conversa e microfone não pede foco; gesto só começa para baixo', async () => {
    await act(async () => root.render(React.createElement(Index)));
    await act(async () => click('drawer-new'));
    expect(mode()).toBe('newConversation');
    expect(calls.closeDrawer).toHaveBeenCalledTimes(1);
    expect(calls.pan?.onMoveShouldSetPanResponder(null, { dx: 30, dy: 15 })).toBe(false);
    expect(calls.pan?.onMoveShouldSetPanResponder(null, { dx: 1, dy: 50 })).toBe(true);
    await act(async () => calls.pan?.onPanResponderRelease(null, { dy: 60 }));
    expect(mode()).toBe('sessions');
    await act(async () => click('open-mic'));
    expect(mode()).toBe('newConversation');
    expect(container.querySelector('[data-mode]')!.getAttribute('data-focus')).toBe('1');
  });
});
