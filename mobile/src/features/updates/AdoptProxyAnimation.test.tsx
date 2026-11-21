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

import { AdoptProxyAnimation, frameAt } from './AdoptProxyAnimation';

it('movimento reduzido para no proxy de fora com o Deixar o Hangar cuidar', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(AdoptProxyAnimation, { width: 340 })));
  try {
    expect(container.textContent).toContain('CLIProxyAPI v8.0.56');
    expect(container.querySelector(`[aria-label="${m.mobile_cliproxy_adopt_button()}"]`)).toBeTruthy();
  } finally {
    act(() => root.unmount());
  }
});

it('o roteiro passa pelas quatro etapas e termina com o Hangar cuidando', () => {
  expect(frameAt(0.5).install.state).toBe('external');
  expect([2, 3, 4, 4.5].map(t => frameAt(t).install.job?.step)).toEqual(['download', 'config', 'service', 'start']);
  expect(frameAt(6)).toMatchObject({ install: { state: 'managed', version: 'v8.0.79' }, outcome: { k: 'done', action: 'adopt', tag: 'v8.0.79' } });
});
