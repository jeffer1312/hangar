// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';
import { overwriteGetLocale } from '../../paraglide/runtime';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('expo-router', () => ({ useIsFocused: () => true, useRouter: () => ({ push: vi.fn() }) }));
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: ReactNode }) => children }));
// Como no teste do composer: vidro, campo e onda do ditado são nativos.
vi.mock('../../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('../../ui/MultilineInput', () => ({ MultilineInput: () => null }));
vi.mock('../../ui/RecordingWave', () => ({ RecordingWave: () => null }));
// A folha de ajustes inteira traz módulos nativos; aqui só importa o que a animação entrega ao chip.
vi.mock('../../chat/SessionSettings', () => ({
  SessionSettingsControl: ({ glyph, plan, icon, model }: { glyph?: ReactNode; plan?: boolean; icon?: string; model: { display: string } }) =>
    createElement('div', { 'data-chip': '', 'data-plan': String(!!plan), 'data-icon': icon ?? '', 'data-model': glyph ? '' : model.display }, glyph),
}));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(HTMLElement.prototype as HTMLElement & { measureInWindow?: unknown }).measureInWindow =
  (done: (x: number, y: number, w: number, h: number) => void) => done(0, 0, 340, 400);
overwriteGetLocale(() => 'pt');

import { AntigravityAnimation, frameAt } from './AntigravityAnimation';

it('movimento reduzido para na sessão do Antigravity com o modelo e o modo Automático', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(AntigravityAnimation, { width: 340 })));
  try {
    const chip = container.querySelector('[data-chip]');
    expect(chip?.getAttribute('data-model')).toBe('Gemini 4 Neon (Low)');
    expect(chip?.getAttribute('data-plan')).toBe('false');
    expect(chip?.getAttribute('data-icon')).toBe('Sparkles');
    expect(container.textContent).toContain(m.native_mode_auto());
    expect(container.textContent).toContain(m.composer_mensagem_para({ nome: 'Antigravity' }));
  } finally {
    act(() => root.unmount());
  }
});

it('o roteiro escolhe o Antigravity, abre a sessão em Plano e troca para Automático', () => {
  expect(frameAt(0.5)).toEqual({ provider: 'claude', session: false, mode: 'plan' });
  expect(frameAt(2)).toEqual({ provider: 'antigravity', session: false, mode: 'plan' });
  expect(frameAt(3.5)).toEqual({ provider: 'antigravity', session: true, mode: 'plan' });
  expect(frameAt(5)).toEqual({ provider: 'antigravity', session: true, mode: 'auto' });
});
