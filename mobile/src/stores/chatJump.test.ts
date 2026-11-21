import { afterEach, beforeEach, expect, it, vi } from 'vitest';

const h = vi.hoisted(() => ({ toast: vi.fn() }));
vi.mock('../ui/Toast', () => ({ toast: { erro: h.toast, ok: () => {} } }));
vi.mock('../paraglide/messages', () => ({ gallery_jump_not_found: () => 'not_found' }));

import { consumeChatJump, JUMP_TTL_MS, requestChatJump, useChatJump } from './chatJump';

beforeEach(() => { vi.useFakeTimers(); h.toast.mockReset(); useChatJump.setState({ request: null }); });
afterEach(() => { vi.useRealTimers(); });

it('a jump no list found says so instead of expiring silently', () => {
  requestChatJump('s1::sess', 'old-msg');
  vi.advanceTimersByTime(JUMP_TTL_MS - 1);
  expect(h.toast).not.toHaveBeenCalled();
  vi.advanceTimersByTime(1);
  expect(h.toast).toHaveBeenCalledExactlyOnceWith('not_found');
  expect(useChatJump.getState().request).toBeNull();
});

it('a jump the list consumed, or one replaced by a newer request, does not warn', () => {
  requestChatJump('s1::sess', 'a');
  consumeChatJump(useChatJump.getState().request!.n);
  requestChatJump('s1::sess', 'b');
  requestChatJump('s1::sess', 'c');
  consumeChatJump(useChatJump.getState().request!.n);
  vi.advanceTimersByTime(JUMP_TTL_MS);
  expect(h.toast).not.toHaveBeenCalled();
});
