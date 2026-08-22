import EventSource from 'react-native-sse';
import type { EventSourceLike } from '@hangar/core';

export function createEventSource(
  url: string,
  opts: { withCredentials: boolean; headers?: Record<string, string> },
): EventSourceLike {
  // A lib trata `timeout` como TETO da conexão (error+close aos Ns mesmo com stream saudável —
  // EventSource.js:157-163). Watchdog de inatividade é nosso: qualquer evento rearma; 25s de
  // silêncio (ping do backend é 10s) = fecha e avisa os handlers de error do consumidor.
  const es = new EventSource(url, {
    headers: opts.headers,
    pollingInterval: 3_000,
  });
  // react-native-sse 1.2.1 não expõe readyState/status público (verificado no index.d.ts); rastreamos via eventos.
  let estado = 0; // 0 CONNECTING, 1 OPEN, 2 CLOSED
  let vigia: ReturnType<typeof setTimeout> | null = null;
  const errHandlers = new Set<(ev: unknown) => void>();
  let curOnError: ((ev: unknown) => void) | null = null;
  let curOnOpen: ((ev: unknown) => void) | null = null;
  const fechar = () => {
    if (vigia) clearTimeout(vigia);
    estado = 2;
    // A lib agenda _pollAgain DEPOIS de despachar 'error' (EventSource.js:121-134); um close
    // síncrono de dentro do handler só limpa o timer anterior. Este mata o que nasce em seguida.
    es.removeAllEventListeners();
    es.close();
    setTimeout(() => es.close(), 0);
  };
  const rearmar = () => {
    if (vigia) clearTimeout(vigia);
    vigia = setTimeout(() => {
      fechar();
      errHandlers.forEach((f) => f({ type: 'timeout' }));
    }, 25_000);
  };
  es.addEventListener('open', () => {
    estado = 1;
    rearmar();
  });
  // erro transitório não marca CLOSED — a lib ainda tenta reconectar (pollingInterval);
  // só timeout/watchdog e close explícito marcam 2 (emenda 4).
  es.addEventListener('error', () => {
    // não seta estado=2 aqui — mantido 1 enquanto a lib reconecta
  });
  es.addEventListener('close', () => {
    estado = 2;
  });
  // emenda 2: wrap ramifica por tipo — eventos de dados tem {data,lastEventId}, error/close/open não
  const wrapData =
    (fn: (ev: { data: string; lastEventId?: string }) => void) => (ev: unknown) => {
      rearmar();
      const e = ev as { data: string; lastEventId?: string | null };
      fn({ data: e.data, lastEventId: e.lastEventId ?? undefined });
    };
  const wrapRaw = (fn: (ev: unknown) => void) => (ev: unknown) => {
    fn(ev);
  };
  // emenda 3: chave por type+fn — mesma função em dois tipos não colide
  const maps = new Map<string, WeakMap<Function, Function>>();
  function getMap(type: string): WeakMap<Function, Function> {
    let m = maps.get(type);
    if (!m) { m = new WeakMap(); maps.set(type, m); }
    return m;
  }
  return {
    addEventListener(type, fn) {
      // emenda 2: só eventos de dados passam por wrapData
      const isData = type === 'message' || type === 'state' || type === 'preview' || type === 'reset' || type === 'ping';
      const w = isData ? wrapData(fn as never) : wrapRaw(fn as never);
      getMap(type as string).set(fn as Function, w as unknown as Function);
      es.addEventListener(type as never, w as never);
    },
    removeEventListener(type, fn) {
      const w = getMap(type as string).get(fn as Function);
      if (w) es.removeEventListener(type as never, w as never);
    },
    close() {
      fechar();
    },
    get readyState() {
      return estado;
    },
    // emenda 4: atribuição substitui, não acumula
    set onerror(fn) {
      if (curOnError) {
        es.removeEventListener('error', curOnError as never);
        errHandlers.delete(curOnError);
      }
      curOnError = fn as ((ev: unknown) => void) | null;
      if (fn) {
        es.addEventListener('error', fn as never);
        errHandlers.add(fn as (ev: unknown) => void);
      }
    },
    get onerror() {
      return null;
    },
    set onopen(fn) {
      if (curOnOpen) es.removeEventListener('open', curOnOpen as never);
      curOnOpen = fn as ((ev: unknown) => void) | null;
      if (fn) es.addEventListener('open', fn as never);
    },
    get onopen() {
      return null;
    },
  };
}
