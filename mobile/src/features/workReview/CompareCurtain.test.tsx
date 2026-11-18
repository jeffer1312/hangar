// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('react-native-gesture-handler', () => {
  // Gesto encadeado: cada chamada devolve o mesmo objeto; o teste não arrasta.
  const chain: unknown = new Proxy({}, { get: () => () => chain });
  return {
    Gesture: { Pan: () => chain },
    GestureDetector: ({ children }: { children: ReactNode }) => children,
    GestureHandlerRootView: ({ children }: { children: ReactNode }) => createElement('div', null, children),
  };
});
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));

import { CompareCurtain, type CurtainItem } from './CompareCurtain';

const img = (uri: string) => ({ uri, headers: {} });
const items: CurtainItem[] = ['Cabeçalho', 'Janela', 'Título'].map((title, k) => ({
  title, before: img(`b${k}`), after: img(`a${k}`), leftTag: m.work_review_before(), rightTag: m.work_review_after(),
}));

let root: Root;
let container: HTMLDivElement;
beforeEach(() => { container = document.createElement('div'); root = createRoot(container); });
afterEach(() => act(() => root.unmount()));
const button = (label: string) => [...container.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label)!;

describe('CompareCurtain', () => {
  it('anterior e próxima dão a volta na lista; fechar fecha', () => {
    const onIndex = vi.fn();
    const onClose = vi.fn();
    act(() => root.render(createElement(CompareCurtain, { items, index: 0, onIndex, onClose })));
    expect(container.textContent).toContain('Cabeçalho');
    expect(container.textContent).toContain(m.work_review_position({ index: 1, total: 3 }));
    expect(container.textContent).toContain(m.work_review_drag_hint());
    act(() => button(m.work_review_previous_short()).click());
    expect(onIndex).toHaveBeenLastCalledWith(2);
    act(() => button(m.work_review_next_short()).click());
    expect(onIndex).toHaveBeenLastCalledWith(1);
    act(() => [...container.querySelectorAll('button')].filter((b) => b.getAttribute('aria-label') === m.native_close())[1].click());
    expect(onClose).toHaveBeenCalled();
  });

  it('um item só, sem depois: nem navegação nem dica da divisória', () => {
    act(() => root.render(createElement(CompareCurtain, {
      items: [{ title: 'Diferença', before: img('x'), after: null, leftTag: 'Mock', rightTag: '' }], index: 0, onIndex: vi.fn(), onClose: vi.fn(),
    })));
    expect(container.textContent).toContain('Diferença');
    expect(container.textContent).not.toContain(m.work_review_drag_hint());
    expect(button(m.work_review_next_short())).toBeUndefined();
  });
});
