import { describe, test, expect, vi } from 'vitest';

let ultimo: { url: string; opts: Record<string, unknown>; listeners: Record<string, Function[]> } | null = null;

vi.mock('react-native-sse', () => ({
  default: class {
    url: string;
    opts: Record<string, unknown>;
    listeners: Record<string, Function[]> = {};
    constructor(url: string, opts: Record<string, unknown>) {
      this.url = url;
      this.opts = opts;
      ultimo = this as unknown as typeof ultimo;
    }
    addEventListener(t: string, f: Function) {
      (this.listeners[t] ??= []).push(f);
    }
    removeEventListener() {}
    close() {}
  },
}));

import { createEventSource } from './sse';

test('repassa data e lastEventId ao listener e manda o timeout do watchdog', () => {
  const es = createEventSource('http://x/api/sessions/a/events?token=t', { withCredentials: false });
  const got: { data: string; lastEventId?: string }[] = [];
  es.addEventListener('message', (e) => got.push(e));
  // simula evento do react-native-sse
  const mock = ultimo as unknown as { listeners: Record<string, Function[]>; opts: Record<string, unknown> };
  mock.listeners.message[0]({ data: '{"a":1}', lastEventId: 'x:1' });
  expect(got[0]).toEqual({ data: '{"a":1}', lastEventId: 'x:1' });
  expect(mock.opts.timeout).toBe(25_000);
});
