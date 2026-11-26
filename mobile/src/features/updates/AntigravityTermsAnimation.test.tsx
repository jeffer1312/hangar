// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';
import { overwriteGetLocale } from '../../paraglide/runtime';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('expo-router', () => ({ useIsFocused: () => true, useRouter: () => ({ push: vi.fn() }) }));
// Como no teste da folha dos Termos: ícone, folha, vibração e aviso são nativos.
vi.mock('expo-linking', () => ({ openURL: vi.fn() }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: () => null }));
vi.mock('../../ui/haptics', () => ({ haptic: { selection: vi.fn(), success: vi.fn(), error: vi.fn() } }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(HTMLElement.prototype as HTMLElement & { measureInWindow?: unknown }).measureInWindow =
  (done: (x: number, y: number, w: number, h: number) => void) => done(0, 0, 340, 400);
overwriteGetLocale(() => 'pt');

import { AntigravityTermsAnimation, frameAt } from './AntigravityTermsAnimation';

it('movimento reduzido para nos Termos, com os links e as duas escolhas', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(AntigravityTermsAnimation, { width: 340 })));
  try {
    expect(container.textContent).toContain(m.mobile_antigravity_terms_title());
    expect(container.textContent).toContain('https://antigravity.google/terms');
    expect(container.querySelector(`[aria-label="${m.mobile_antigravity_terms_accept_share()}"]`)).toBeTruthy();
    expect(container.querySelector(`[aria-label="${m.mobile_antigravity_terms_accept_private()}"]`)).toBeTruthy();
    expect(container.textContent).not.toContain(m.mobile_antigravity_terms_answering());
  } finally {
    act(() => root.unmount());
  }
});

it('o roteiro abre os Termos e depois confirma a escolha no agy', () => {
  expect([0.5, 2, 5].map(frameAt)).toEqual(['starting', 'terms', 'answering']);
});
