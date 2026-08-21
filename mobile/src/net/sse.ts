import EventSource from 'react-native-sse';
import type { EventSourceLike } from '@hangar/core';

export function createEventSource(
  url: string,
  opts: { withCredentials: boolean; headers?: Record<string, string> },
): EventSourceLike {
  // timeout 25s = watchdog do ping de 10s do backend; pollingInterval = reconexão automática (manda Last-Event-ID sozinho)
  const es = new EventSource(url, {
    headers: opts.headers,
    timeout: 25_000,
    pollingInterval: 3_000,
  });
  // react-native-sse 1.2.1 não expõe readyState/status público (verificado no index.d.ts); rastreamos via eventos.
  let estado = 0; // 0 CONNECTING, 1 OPEN, 2 CLOSED
  es.addEventListener('open', () => {
    estado = 1;
  });
  es.addEventListener('error', () => {
    estado = 2;
  });
  es.addEventListener('close', () => {
    estado = 2;
  });
  const wrap =
    (fn: (ev: { data: string; lastEventId?: string }) => void) => (ev: unknown) => {
      const e = ev as { data: string; lastEventId?: string | null };
      // react-native-sse MessageEvent tem { data, lastEventId } mas pode ser null
      fn({ data: e.data, lastEventId: e.lastEventId ?? undefined });
    };
  const map = new WeakMap<Function, Function>();
  return {
    addEventListener(type, fn) {
      const w = wrap(fn);
      map.set(fn, w);
      es.addEventListener(type as never, w as never);
    },
    removeEventListener(type, fn) {
      const w = map.get(fn);
      if (w) es.removeEventListener(type as never, w as never);
    },
    close() {
      estado = 2;
      es.close();
    },
    get readyState() {
      return estado;
    },
    set onerror(fn) {
      if (fn) es.addEventListener('error', fn as never);
    },
    get onerror() {
      return null;
    },
    set onopen(fn) {
      if (fn) es.addEventListener('open', fn as never);
    },
    get onopen() {
      return null;
    },
  };
}
