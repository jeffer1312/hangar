// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it } from 'vitest';
import * as m from '../../paraglide/messages';
import { PlacementSuggestAnimation } from './PlacementSuggestAnimation';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// O mock padrão do reanimated responde movimento reduzido: o quadro parado é a sugerida, com o motivo.
it('para na máquina sugerida com o motivo na linha dela', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(PlacementSuggestAnimation, { width: 340 })));
  try {
    expect(container.textContent).toContain(m.native_news_placement_free());
    expect(container.textContent).toContain(m.native_placement_exclusion_working());
  } finally {
    act(() => root.unmount());
  }
});
