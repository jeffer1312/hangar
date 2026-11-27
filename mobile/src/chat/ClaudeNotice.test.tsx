// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ChatEvent } from '@hangar/core';
import * as m from '../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const calls = vi.hoisted(() => ({ cancel: [] as unknown[][] }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(),
  cancelSessionAutoContinue: async (...args: unknown[]) => { calls.cancel.push(args); return true; } }));
vi.mock('../stores/sessions', () => ({ useSessions: (select: (s: { noticeIgnores: Record<string, string[]> }) => unknown) => select({ noticeIgnores: {} }) }));
vi.mock('../features/notices/api', () => ({ serverOf: (id: string) => ({ id }), fixPreview: vi.fn(), putNoticeIgnore: vi.fn(),
  reinstallPlugin: vi.fn(), startFix: vi.fn() }));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('./NoticeFixSheet', () => ({ NoticeFixSheet: () => null }));

import { AUTO_CONTINUE_CANCEL_SETTLE_MS, ClaudeNotice } from './ClaudeNotice';

const roots: Root[] = [];
afterEach(() => {
  for (const root of roots.splice(0)) act(() => root.unmount());
  calls.cancel.length = 0;
  vi.useRealTimers();
});

const waiting = { kind: 'assistant_msg', id: 'n1', text: 'texto do servidor',
  notice_meta: { kind: 'auto_continue', state: 'waiting', stopped_at: 1_800_000_000, count: 1 } } as unknown as ChatEvent;

async function render(event: ChatEvent) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(createElement(ClaudeNotice, { event, serverId: 's1', sessionName: 'sess' })));
  return container;
}

const cancelPill = (container: HTMLElement) =>
  [...container.querySelectorAll('button')].find((button) => button.textContent === m.auto_continue_cancel());

describe('nota do continuar sozinho', () => {
  it('Cancelar aceito sem a nota trocada chegar: o giro para no prazo e o botão volta', async () => {
    vi.useFakeTimers();
    const container = await render(waiting);
    await act(async () => {
      cancelPill(container)!.click();
      for (let i = 0; i < 5; i++) await Promise.resolve();
    });
    expect(calls.cancel).toEqual([['sess', { id: 's1' }]]);
    expect(container.textContent).toContain('loading');
    expect(cancelPill(container)).toBeUndefined();
    act(() => { vi.advanceTimersByTime(AUTO_CONTINUE_CANCEL_SETTLE_MS - 1); });
    expect(container.textContent).toContain('loading');
    act(() => { vi.advanceTimersByTime(1); });
    expect(container.textContent).not.toContain('loading');
    expect(cancelPill(container)).toBeDefined();
  });
});
