// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';
import { overwriteGetLocale } from '../../paraglide/runtime';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('expo-router', () => ({ useIsFocused: () => true, useRouter: () => ({ push: vi.fn() }) }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(HTMLElement.prototype as HTMLElement & { measureInWindow?: unknown }).measureInWindow =
  (done: (x: number, y: number, w: number, h: number) => void) => done(0, 0, 340, 400);
overwriteGetLocale(() => 'pt');

import { ProxyUpdateAnimation, frameAt } from './ProxyUpdateAnimation';

it('movimento reduzido para na versão nova com o Atualizar', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(ProxyUpdateAnimation, { width: 340 })));
  try {
    expect(container.textContent).toContain('CLIProxyAPI v8.1.0');
    expect(container.querySelector(`[aria-label="${m.mobile_cliproxy_update_button()}"]`)).toBeTruthy();
  } finally {
    act(() => root.unmount());
  }
});

it('o roteiro passa pelas três etapas e termina atualizado', () => {
  expect(frameAt(0.5).install.update_available).toBe(true);
  expect([2, 3, 4].map(t => frameAt(t).install.job?.step)).toEqual(['download', 'service', 'start']);
  expect(frameAt(5).outcome).toEqual({ k: 'done', tag: 'v8.1.0' });
});
