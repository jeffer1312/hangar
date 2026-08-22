import { test, expect, vi, beforeEach, afterEach } from 'vitest';

let ultimo: {
  url: string;
  opts: Record<string, unknown>;
  listeners: Record<string, Function[]>;
  close: ReturnType<typeof vi.fn>;
} | null = null;

vi.mock('react-native-sse', () => ({
  default: class {
    url: string;
    opts: Record<string, unknown>;
    listeners: Record<string, Function[]> = {};
    close = vi.fn();
    removeAllEventListeners = vi.fn();
    constructor(url: string, opts: Record<string, unknown>) {
      this.url = url;
      this.opts = opts;
      ultimo = this as unknown as typeof ultimo;
    }
    addEventListener(t: string, f: Function) {
      (this.listeners[t] ??= []).push(f);
    }
    removeEventListener() {}
  },
}));

import { createEventSource } from './sse';

beforeEach(() => {
  vi.useRealTimers();
  ultimo = null;
});

afterEach(() => {
  vi.useRealTimers();
});

test('repassa data e lastEventId ao listener e não manda timeout de teto', () => {
  const es = createEventSource('http://x/api/sessions/a/events?token=t', { withCredentials: false });
  const got: { data: string; lastEventId?: string }[] = [];
  es.addEventListener('message', (e) => got.push(e));
  const mock = ultimo as unknown as { listeners: Record<string, Function[]>; opts: Record<string, unknown> };
  mock.listeners.message[0]({ data: '{"a":1}', lastEventId: 'x:1' });
  expect(got[0]).toEqual({ data: '{"a":1}', lastEventId: 'x:1' });
  expect(mock.opts.timeout).toBeUndefined();
});

test('watchdog não fecha com ping a cada 10s por 60s', () => {
  vi.useFakeTimers();
  ultimo = null;
  const es2 = createEventSource('http://x/api/sessions/b/events?token=t', { withCredentials: false });
  const mock2 = ultimo as unknown as {
    listeners: Record<string, Function[]>;
    close: ReturnType<typeof vi.fn>;
  };
  const handler = vi.fn();
  es2.addEventListener('message', handler);
  mock2.listeners.open[0]({});
  for (let i = 0; i < 6; i++) {
    vi.advanceTimersByTime(10_000);
    // simula ping do backend como message (rearma via wrap)
    mock2.listeners.message[0]({ data: 'ping', lastEventId: `id-${i}` });
  }
  expect(mock2.close).not.toHaveBeenCalled();
  vi.useRealTimers();
});

test('watchdog fecha após 25s de silêncio e avisa onerror com type timeout', () => {
  vi.useFakeTimers();
  const es = createEventSource('http://x/api/sessions/a/events?token=t', { withCredentials: false });
  const mock = ultimo as unknown as {
    listeners: Record<string, Function[]>;
    close: ReturnType<typeof vi.fn>;
  };
  const onErr = vi.fn();
  es.onerror = onErr;
  // open rearma para 25s
  mock.listeners.open[0]({});
  expect(mock.close).not.toHaveBeenCalled();
  expect(onErr).not.toHaveBeenCalled();
  vi.advanceTimersByTime(25_000);
  expect(mock.close).toHaveBeenCalledTimes(1);
  expect(onErr).toHaveBeenCalledWith({ type: 'timeout' });
  vi.useRealTimers();
});
