// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { DesktopToolLookAnimation } from './DesktopToolLookAnimation';
import * as m from '../../paraglide/messages';

const ui = vi.hoisted(() => ({ reduced: false }));
vi.mock('react-native-reanimated', async () => {
  const { createElement } = await import('react');
  return {
    default: { View: ({ children }: { children: ReactNode }) => createElement('div', null, children) },
    FadeIn: { duration: () => ({}) },
    useReducedMotion: () => ui.reduced,
  };
});
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function render(): { text: () => string; done: () => void } {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(DesktopToolLookAnimation, { width: 320 })));
  return { text: () => container.textContent ?? '', done: () => act(() => root.unmount()) };
}

it('começa só com a linha do resumo e abre no cartão com as falhas', () => {
  vi.useFakeTimers();
  ui.reduced = false;
  const view = render();
  try {
    expect(view.text()).toContain(`${m.ferr_desktop_rodou({ n: 4 })} ${m.ferr_desktop_falhas({ n: 2 })}`);
    expect(view.text()).not.toContain('npm run build');
    act(() => { vi.advanceTimersByTime(1700); });
    expect(view.text()).toContain('npm run build');
    expect(view.text().split(m.formato_tool_falhou()).length - 1).toBe(2);
  } finally {
    view.done();
    vi.useRealTimers();
  }
});

it('movimento reduzido para no cartão aberto', () => {
  ui.reduced = true;
  const view = render();
  try {
    expect(view.text()).toContain('cargo test -p hangar-api');
  } finally {
    view.done();
  }
});
