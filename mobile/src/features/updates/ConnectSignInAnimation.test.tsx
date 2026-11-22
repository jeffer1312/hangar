// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';
import { overwriteGetLocale } from '../../paraglide/runtime';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('expo-router', () => ({ useIsFocused: () => true }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../connect/ConnectHero', () => ({ ConnectMark: () => null }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
// O palco no DOM do teste é uma div: a medida da tela vem daqui, sempre visível.
(HTMLElement.prototype as HTMLElement & { measureInWindow?: unknown }).measureInWindow =
  (done: (x: number, y: number, w: number, h: number) => void) => done(0, 0, 340, 400);
overwriteGetLocale(() => 'pt');

import { ConnectSignInAnimation, frameAt } from './ConnectSignInAnimation';

it('movimento reduzido para na lista de máquinas, com as ligadas marcadas', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(ConnectSignInAnimation, { width: 340 })));
  try {
    expect(container.textContent).toContain(m.connect_your_machines());
    expect(container.textContent).toContain(m.login_guest_enter_many({ n: '2' }));
    const checked = (name: string) => container.querySelector(`[aria-label="${name}"]`)?.getAttribute('aria-checked');
    expect(checked(m.news_connect_sample_machine())).toBe('true');
    expect(checked(m.news_tags_sample_machine())).toBe('true');
    expect(checked(m.news_auto_report_sample_machine())).toBe('false');
    expect(container.textContent).toContain(m.connect_machine_off_now());
  } finally {
    act(() => root.unmount());
  }
});

it('o roteiro digita a conta letra a letra, preenche a senha, toca em Entrar e abre a lista', () => {
  expect(frameAt(0.2, 'ana')).toEqual({ user: '', password: false, pressed: false, list: false });
  expect([0.5, 0.7, 0.9].map((t) => frameAt(t, 'ana').user)).toEqual(['a', 'an', 'ana']);
  expect(frameAt(1.6, 'ana')).toMatchObject({ user: 'ana', password: true, pressed: false });
  expect(frameAt(2.4, 'ana')).toMatchObject({ pressed: true, list: false });
  expect(frameAt(3.2, 'ana').list).toBe(true);
});
