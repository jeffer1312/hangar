// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('expo-router', () => ({ useIsFocused: () => true }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
// O resto da seção da revisão vem junto com a faixa; nada disso desenha na animação.
vi.mock('../../ui/Toast', () => ({ toast: { erro: vi.fn(), ok: vi.fn() } }));
vi.mock('../../stores/servers', () => ({ useServers: () => [] }));
vi.mock('../../ui/MultilineInput', () => ({ MultilineInput: () => null }));
vi.mock('../workReview/CompareCurtain', () => ({ CompareCurtain: () => null }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
// O palco no DOM do teste é uma div: a medida da tela vem daqui, sempre visível.
(HTMLElement.prototype as HTMLElement & { measureInWindow?: unknown }).measureInWindow =
  (done: (x: number, y: number, w: number, h: number) => void) => done(0, 0, 340, 400);

import { WorkReviewAnimation } from './WorkReviewAnimation';

it('movimento reduzido para na primeira decisão escolhida, com a conta em 2', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(WorkReviewAnimation, { width: 340 })));
  try {
    const text = container.textContent ?? '';
    expect(text).toContain(m.native_news_work_review_sample_title_1());
    expect(text).toContain(m.native_news_work_review_sample_badge({ count: 2 }));
    expect(text).toContain(m.work_review_position({ index: 1, total: 3 }));
    expect(text).toContain(m.work_review_pending({ count: 2 }));
  } finally {
    act(() => root.unmount());
  }
});
