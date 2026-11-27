// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it } from 'vitest';
import * as m from '../../paraglide/messages';
import { TelemetryConsentAnimation } from './TelemetryConsentAnimation';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// O mock padrão do reanimated responde movimento reduzido: o quadro parado é a linha já ligada.
it('para na linha das contagens anônimas ligada ', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(TelemetryConsentAnimation, { width: 340 })));
  try {
    expect(container.textContent).toContain(m.telemetry_setting());
    expect(container.textContent).toContain(m.auto_report_on());
    expect(container.textContent).not.toContain(m.auto_report_value_unanswered());
  } finally {
    act(() => root.unmount());
  }
});
