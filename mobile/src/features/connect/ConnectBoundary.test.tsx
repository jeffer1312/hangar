// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const diag = vi.hoisted(() => ({ events: [] as Record<string, unknown>[] }));
vi.mock('@hangar/core', async (original) => ({
  ...await original<object>(),
  registrarDiag: (event: Record<string, unknown>) => { diag.events.push(event); },
}));

vi.mock('../config/colors', () => ({ useSettingsColors: () => ({ text: '#000' }) }));

import { ConnectBoundary } from './ConnectBoundary';
import * as m from '../../paraglide/messages';

let broken = true;
function Area() {
  if (broken) throw new Error('máquina sem nome');
  return createElement('span', null, 'ok');
}

it('um erro ao desenhar a área do Connect vira aviso com nova tentativa, e não derruba o app', async () => {
  vi.spyOn(console, 'error').mockImplementation(() => {});
  const container = document.createElement('div');
  await act(async () => createRoot(container).render(createElement(ConnectBoundary, null, createElement(Area))));
  expect(container.textContent).toContain(m.connect_screen_failed());
  expect(diag.events).toEqual([expect.objectContaining({ evento: 'connect.tela_falhou', codigo: 'connect_screen_failed' })]);
  broken = false;
  const retry = container.querySelector('[role="button"]') as HTMLElement;
  await act(async () => retry.click());
  expect(container.textContent).toContain('ok');
});
