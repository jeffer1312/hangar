// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('react-native-svg', () => ({ SvgXml: () => null }));
// O ServerConfigParts traz a casca das páginas (Pagina, Screen, fundo), que não carrega no vitest.
vi.mock('../config/ServerConfigParts', async () => {
  const React = await import('react');
  return {
    BlockHead: ({ title, help }: { title: string; help?: string }) => React.createElement('h3', null, `${title} ${help ?? ''}`),
    Muted: ({ children }: { children: import('react').ReactNode }) => React.createElement('p', null, children),
    useInputStyle: () => ({}),
  };
});
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import { WhatsAppReadonlyAnimation, sampleQr } from './WhatsAppReadonlyAnimation';

it('movimento reduzido para no conectado, com o número mascarado e os totais', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(WhatsAppReadonlyAnimation, { width: 340 })));
  try {
    const text = container.textContent ?? '';
    expect(text).toContain(m.native_whatsapp_connected({ device: '+55 64 •••••-1234' }));
    expect(text).toContain(m.native_whatsapp_usage_title());
    expect(text).not.toContain(m.native_whatsapp_accept());
  } finally {
    act(() => root.unmount());
  }
});

it('o QR de exemplo tem os três cantos', () => {
  const svg = sampleQr();
  for (const corner of ['x="0" y="0"', 'x="20" y="0"', 'x="0" y="20"']) expect(svg).toContain(corner);
});
