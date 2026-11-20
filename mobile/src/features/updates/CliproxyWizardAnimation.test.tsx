// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('expo-router', () => ({ useIsFocused: () => true, useRouter: () => ({ push: vi.fn() }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
// O assistente vem junto com as peças; a folha e o teclado dele não desenham na animação.
vi.mock('expo-linking', () => ({ openURL: vi.fn() }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('react-native', async original => ({
  ...await original<typeof import('react-native')>(),
  Switch: (props: { accessibilityLabel: string; value: boolean }) =>
    createElement('input', { type: 'checkbox', 'aria-label': props.accessibilityLabel, checked: props.value, readOnly: true }),
}));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
// O palco no DOM do teste é uma div: a medida da tela vem daqui, sempre visível.
(HTMLElement.prototype as HTMLElement & { measureInWindow?: unknown }).measureInWindow =
  (done: (x: number, y: number, w: number, h: number) => void) => done(0, 0, 340, 400);

import { CliproxyWizardAnimation } from './CliproxyWizardAnimation';

it('movimento reduzido para no passo do nome, com o modelo e o Fast', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(CliproxyWizardAnimation, { width: 340 })));
  try {
    const text = container.textContent ?? '';
    expect(text).toContain(m.mobile_cliproxy_wizard_step_name());
    expect(text).toContain(`GPT ${m.news_cliproxy_sample_email().split('@')[0]}`);
    expect(text).toContain('gpt-6.1-sol');
    expect(container.querySelector<HTMLInputElement>(`input[aria-label="${m.mobile_cliproxy_wizard_fast()}"]`)?.checked).toBe(true);
  } finally {
    act(() => root.unmount());
  }
});
