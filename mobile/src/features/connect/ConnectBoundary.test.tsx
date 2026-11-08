// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, expect, it, vi } from 'vitest';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const diag = vi.hoisted(() => ({ events: [] as Record<string, unknown>[], broken: false }));
vi.mock('@hangar/core', async (original) => ({
  ...await original<object>(),
  registrarDiag: (event: Record<string, unknown>) => {
    if (diag.broken) throw new Error('diário fora');
    diag.events.push(event);
  },
}));

vi.mock('../config/colors', () => ({ useSettingsColors: () => ({ text: '#000' }) }));

import { ConnectBoundary } from './ConnectBoundary';
import * as m from '../../paraglide/messages';

let broken = true;
function Area() {
  if (broken) throw new Error('máquina sem nome');
  return createElement('span', null, 'ok');
}

const show = (root: Root, resetKey: string) => act(async () => root.render(createElement(ConnectBoundary, { resetKey, children: createElement(Area) })));

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => {});
  (diag.events.length = 0, diag.broken = false, broken = true);
});

it('um erro ao desenhar a área do Connect vira aviso com nova tentativa, e não derruba o app', async () => {
  const container = document.createElement('div');
  await show(createRoot(container), 'conta-a');
  expect(container.textContent).toContain(m.connect_screen_failed());
  expect(diag.events).toEqual([expect.objectContaining({ evento: 'connect.screen_failed', codigo: 'connect_screen_failed' })]);
  expect(String(diag.events[0].detalhe)).toContain('Area');
  broken = false;
  const retry = container.querySelector('[role="button"]') as HTMLElement;
  await act(async () => retry.click());
  expect(container.textContent).toContain('ok');
});

it('outra conta desenha a área de novo, sem o aviso da anterior', async () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  await show(root, 'conta-a');
  broken = false;
  await show(root, 'conta-b');
  expect(container.textContent).toContain('ok');
});

it('o diário fora do ar não derruba o aviso', async () => {
  diag.broken = true;
  const container = document.createElement('div');
  await show(createRoot(container), 'conta-a');
  expect(container.textContent).toContain(m.connect_screen_failed());
});
