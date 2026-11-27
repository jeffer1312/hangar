// @vitest-environment happy-dom
import React, { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('expo-constants', () => ({ default: { deviceName: 'fixture-phone' } }));
vi.mock('expo-router', () => ({ useIsFocused: () => true, useFocusEffect: () => {}, useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('../connect/ConnectHero', () => ({ ConnectMark: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('expo-blur', () => ({ BlurView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('expo-glass-effect', () => ({ GlassView: ({ children }: { children: React.ReactNode }) => children, isLiquidGlassAvailable: () => false }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(HTMLElement.prototype as HTMLElement & { measureInWindow?: unknown }).measureInWindow =
  (done: (x: number, y: number, w: number, h: number) => void) => done(0, 0, 340, 400);

import * as m from '../../paraglide/messages';
import { overwriteGetLocale } from '../../paraglide/runtime';
import { ConnectRemoveComputerAnimation, frameAt } from './ConnectRemoveComputerAnimation';

overwriteGetLocale(() => 'pt');

it('movimento reduzido para no detalhe do computador fora do ar com Remover da conta', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(ConnectRemoveComputerAnimation, { width: 340 })));
  try {
    expect(container.textContent).toContain(m.connect_computers_title());
    expect(container.textContent).toContain(m.connect_computer_remove());
    expect(container.textContent).toContain(m.news_auto_report_sample_machine());
  } finally {
    act(() => root.unmount());
  }
});

it('o roteiro abre o detalhe, remove e tira o computador da lista', () => {
  expect(frameAt(0.3)).toEqual({ tapped: false, detail: false, removing: false, gone: false });
  expect(frameAt(0.9).tapped).toBe(true);
  expect(frameAt(2)).toEqual({ tapped: false, detail: true, removing: false, gone: false });
  expect(frameAt(3)).toEqual({ tapped: false, detail: true, removing: true, gone: false });
  expect(frameAt(4.5)).toEqual({ tapped: false, detail: false, removing: false, gone: true });
});
