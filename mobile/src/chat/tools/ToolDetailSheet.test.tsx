// @vitest-environment happy-dom
import { act, createElement, createRef, useImperativeHandle, type ReactNode, type Ref } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import type { ChatEvent } from '@hangar/core';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('../../stores/servers', () => ({ useServers: (select: (state: unknown) => unknown) => select({ servers: [] }) }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ ref, children }: { ref: Ref<unknown>; children: ReactNode }) => {
  useImperativeHandle(ref, () => ({ present: async () => {} }));
  return createElement('div', null, children);
} }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { erro: vi.fn() } }));

import { ToolDetailSheet, type ToolDetailHandle } from './ToolDetailSheet';
import * as m from '../../paraglide/messages';

it('cada trecho do patch recebe seu arquivo, inclusive vários trechos do mesmo arquivo', async () => {
  const patch = `*** Begin Patch
*** Update File: /repo/first.ts
@@
-oldFirst
+newFirst
@@
-oldAgain
+newAgain
*** Add File: /repo/second.ts
+newSecond
*** End Patch`;
  const use: ChatEvent = { id: 'patch', kind: 'tool_use', tool_name: 'apply_patch', tool_input: {
    patch, file_path: ['/repo/first.ts', '/repo/second.ts'],
  } };
  const result: ChatEvent = { id: 'result', kind: 'tool_result', result: 'ok' };
  const ref = createRef<ToolDetailHandle>();
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(createElement(ToolDetailSheet, { ref, resultOf: () => result })));
  await act(async () => ref.current!.abrir(use));
  const paths = [...container.querySelectorAll('span')].filter((el) => el.textContent?.startsWith('/repo/'));
  expect(paths.map((el) => el.textContent)).toEqual(['/repo/first.ts', '/repo/second.ts']);
  expect(paths[0].parentElement!.textContent).toContain('newFirst');
  expect(paths[0].parentElement!.textContent).toContain('newAgain');
  expect(paths[1].parentElement!.textContent).toContain('newSecond');
  act(() => root.unmount());
});

it('mostra o código de saída acima da saída', async () => {
  const use: ChatEvent = { id: 'shell', kind: 'tool_use', tool_name: 'Bash', tool_input: { command: 'false' } };
  const result: ChatEvent = { id: 'result', kind: 'tool_result', result: 'Exit code 2\nboom', is_error: true };
  const ref = createRef<ToolDetailHandle>();
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(createElement(ToolDetailSheet, { ref, resultOf: () => result })));
  await act(async () => ref.current!.abrir(use));
  // Em inglês a etiqueta repete a primeira linha da saída: a prova é ela logo antes do texto cru.
  expect(container.textContent).toContain(`${m.ferr_desktop_codigo_saida({ n: 2 })}${result.result}`);
  act(() => root.unmount());
});
