// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('expo-router', () => ({ useIsFocused: () => true, useRouter: () => ({ push: vi.fn() }) }));
vi.mock('expo-linear-gradient', () => ({ LinearGradient: ({ children }: { children?: unknown }) => createElement('div', null, children) }));
// O visor e a grade trazem o cache de arquivos e o player de áudio, que são nativos; a animação não os usa.
vi.mock('../attachments/mediaCache', () => ({ canShareFile: false, localCopy: vi.fn(), saveImage: vi.fn(), shareFile: vi.fn(),
  downloadKept: vi.fn(), deliverLocalFile: vi.fn(), keptCopy: () => null }));
vi.mock('../attachments/LightboxModal', () => ({ LightboxModal: () => null }));
vi.mock('../../chat/AudioChip', () => ({ AudioChip: () => null }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { erro: () => {}, ok: () => {} } }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(HTMLElement.prototype as HTMLElement & { measureInWindow?: unknown }).measureInWindow =
  (done: (x: number, y: number, w: number, h: number) => void) => done(0, 0, 340, 400);

import { ConversationGalleryAnimation, viewerIndex } from './ConversationGalleryAnimation';

it('movimento reduzido para o visor na terceira mídia, com a faixa e o "Na conversa"', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(ConversationGalleryAnimation, { width: 340 })));
  try {
    const text = container.textContent ?? '';
    expect(text).toContain(m.gallery_counter({ n: 3, total: 6 }));
    expect(text).toContain(m.news_gallery_sample_3());
    expect(text).toContain(m.gallery_in_chat());
    expect(text).not.toContain(m.media_title());
  } finally {
    act(() => root.unmount());
  }
});

it('o roteiro passa pelas mídias e fica na última durante a grade', () => {
  expect([0, 1, 2, 2.8, 5].map(viewerIndex)).toEqual([0, 1, 2, 3, 3]);
});
