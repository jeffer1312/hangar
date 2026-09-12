import { test, expect, vi, beforeEach, afterEach } from 'vitest';
import { configureApi, configureDiag, openEventStream } from '@hangar/core';

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
  configureDiag({ registrar: () => {}, novoReq: () => '' });
  vi.clearAllTimers();
  vi.useRealTimers();
});

test('reutiliza o ID do core na conexão e nos eventos sem gerar outro', () => {
  vi.useFakeTimers();
  const registrar = vi.fn();
  const novoReq = vi.fn(() => 'core-mobile-1');
  configureDiag({ registrar, novoReq });
  configureApi({ getBaseUrl: () => 'https://correlation.test', getToken: () => 'token-privado',
    origin: null, onUnauthorized() {}, createEventSource });
  const es = openEventStream('sess', 'thread:42');
  const url = new URL(ultimo!.url);
  expect(novoReq).toHaveBeenCalledTimes(1);
  expect(url.searchParams.get('diag_req')).toBe('core-mobile-1');
  expect(url.searchParams.get('last_event_id')).toBe('thread:42');
  expect(url.searchParams.get('token')).toBe('token-privado');
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'sse.abrir', req: 'core-mobile-1' }), 'https://correlation.test');
  es.close();
});

test('fallback inclui o ID na URL e preserva os headers recebidos', () => {
  vi.useFakeTimers();
  const registrar = vi.fn();
  configureDiag({ registrar, novoReq: () => 'mobile-fallback' });
  const headers = { Authorization: 'Bearer privado' };
  const es = createEventSource('https://fallback.test/api/sessions/events', { withCredentials: false, headers });
  expect(new URL(ultimo!.url).searchParams.get('diag_req')).toBe('mobile-fallback');
  expect(ultimo!.opts.headers).toBe(headers);
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ req: 'mobile-fallback' }), 'https://fallback.test');
  es.close();
});

test('registra queda, reconexão e primeiro quadro no destino original sem payload ou token', () => {
  vi.useFakeTimers();
  const registrar = vi.fn();
  let contador = 0;
  configureDiag({ registrar, novoReq: () => `req-${++contador}` });
  const endereco = 'https://privado.test/hangar/api/sessions/nome-privado/events';
  const primeiro = createEventSource(`${endereco}?token=segredo`, { withCredentials: false });
  primeiro.addEventListener('ping', () => {});
  let mock = ultimo!;
  mock.listeners.open[0]({});
  mock.listeners.ping[0]({ data: 'corpo privado' });
  mock.listeners.ping[0]({ data: 'corpo privado' });
  expect(registrar.mock.calls.filter(([e]) => e.evento === 'sse.conectou')).toHaveLength(1);
  mock.listeners.error[0]({ xhrStatus: 503, message: 'segredo https://privado.test' });
  primeiro.close();
  vi.advanceTimersByTime(1000);
  const segundo = createEventSource(`${endereco}?token=novo`, { withCredentials: false });
  segundo.addEventListener('ping', () => {});
  mock = ultimo!;
  mock.listeners.ping[0]({ data: 'corpo privado' });
  mock.listeners.ping[0]({ data: 'corpo privado' });
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'sse.caiu', codigo: '503' }), 'https://privado.test/hangar');
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'sse.reconectando', tentativa: 2 }), 'https://privado.test/hangar');
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'sse.voltou', ms: 1000 }), 'https://privado.test/hangar');
  expect(registrar.mock.calls.filter(([e]) => e.evento === 'sse.voltou')).toHaveLength(1);
  expect(JSON.stringify(registrar.mock.calls.map(([e]) => e))).not.toMatch(/segredo|novo|privado|https:/);
  segundo.close();
});

test('registra silêncio de 25s sem tratar fechamento voluntário como falha', () => {
  vi.useFakeTimers();
  const registrar = vi.fn();
  configureDiag({ registrar, novoReq: () => 'req-timeout' });
  const es = createEventSource('https://silencio.test/api/sessions/events?token=segredo', { withCredentials: false });
  ultimo!.listeners.open[0]({});
  vi.advanceTimersByTime(25000);
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'sse.caiu', codigo: 'silencio', ms: 25000 }), 'https://silencio.test');
  registrar.mockClear();
  es.close();
  expect(registrar).not.toHaveBeenCalled();
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

test('ask_question chega como {data,lastEventId} e rearma watchdog como ping', () => {
  vi.useFakeTimers();
  ultimo = null;
  const es = createEventSource('http://x/api/sessions/a/events?token=t', { withCredentials: false });
  const got: { data: string; lastEventId?: string }[] = [];
  es.addEventListener('ask_question', (e) => got.push(e));
  const mock = ultimo as unknown as {
    listeners: Record<string, Function[]>;
    close: ReturnType<typeof vi.fn>;
  };
  const onErr = vi.fn();
  es.onerror = onErr;
  mock.listeners.open[0]({});
  // manda ask_question a cada 10s por 60s — watchdog não pode fechar
  for (let i = 0; i < 6; i++) {
    vi.advanceTimersByTime(10_000);
    mock.listeners.ask_question[0]({ data: '{"questions":[]}', lastEventId: `id-${i}` });
  }
  expect(got).toHaveLength(6);
  expect(got[0]).toEqual({ data: '{"questions":[]}', lastEventId: 'id-0' });
  expect(mock.close).not.toHaveBeenCalled();
  expect(onErr).not.toHaveBeenCalled();
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
