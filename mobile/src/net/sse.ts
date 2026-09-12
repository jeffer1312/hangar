import EventSource from 'react-native-sse';
import type { EventSourceLike } from '@hangar/core';
import { registrarDiag, novoReqDiag, rotaGenerica } from '@hangar/core';

const quedas = new Map<string, { inicio: number; tentativa: number }>();

// Status que não muda de resposta por insistir: token inválido, sem permissão, sessão que não
// existe, método/rota errados. 408 e 429 ficam de fora porque pedem exatamente o contrário —
// tentar de novo mais tarde —, e 5xx também: backend reiniciando volta sozinho.
function recusaDefinitiva(status: number): boolean {
  return status >= 400 && status < 500 && status !== 408 && status !== 429;
}

export function createEventSource(
  url: string,
  opts: { withCredentials: boolean; headers?: Record<string, string> },
): EventSourceLike {
  const inicio = Date.now();
  const req = novoReqDiag();
  let destino = '';
  let chave = '';
  let rota = '';
  try {
    const endereco = new URL(url);
    const api = endereco.pathname.indexOf('/api/');
    if (api >= 0) {
      destino = endereco.origin + endereco.pathname.slice(0, api);
      chave = destino + endereco.pathname.slice(api);
      rota = rotaGenerica(endereco.pathname.slice(api));
    }
  } catch { /* URL inválida será tratada pelo transporte; nunca usar o servidor ativo como destino. */ }
  const anterior = quedas.get(chave);
  const tentativa = (anterior?.tentativa ?? 0) + 1;
  if (anterior) anterior.tentativa = tentativa;
  const anotar = (evento: string, codigo?: string, ms?: number) => {
    if (destino) registrarDiag({ evento, req, tentativa, codigo, ms, detalhe: rota,
      nivel: evento === 'sse.caiu' ? 'aviso' : 'ok' }, destino);
  };
  const falhou = (codigo: string) => {
    if (chave && !quedas.has(chave)) {
      // ponytail: até 200 conexões interrompidas em memória; histórico durável pertence ao diário.
      if (quedas.size >= 200) quedas.delete(quedas.keys().next().value!);
      quedas.set(chave, { inicio: Date.now(), tentativa });
    }
    anotar('sse.caiu', codigo, Date.now() - inicio);
  };
  let primeiroQuadro = true;
  const chegou = () => {
    const queda = quedas.get(chave);
    if (!primeiroQuadro && !queda) return;
    primeiroQuadro = false;
    anotar(queda ? 'sse.voltou' : 'sse.conectou', undefined, Date.now() - (queda?.inicio ?? inicio));
    quedas.delete(chave);
  };
  anotar(anterior ? 'sse.reconectando' : 'sse.abrir');
  // A lib trata `timeout` como TETO da conexão (error+close aos Ns mesmo com stream saudável —
  // EventSource.js:157-163). Watchdog de inatividade é nosso: qualquer evento rearma; 25s de
  // silêncio (ping do backend é 10s) = fecha e avisa os handlers de error do consumidor.
  let es: EventSource;
  try {
    es = new EventSource(url, { headers: opts.headers, pollingInterval: 3_000 });
  } catch (e) {
    falhou('abertura');
    throw e;
  }
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
      falhou('silencio');
      fechar();
      errHandlers.forEach((f) => f({ type: 'timeout' }));
    }, 25_000);
  };
  es.addEventListener('open', () => {
    estado = 1;
    rearmar();
  });
  // Erro de REDE não marca CLOSED — a lib reconecta sozinha (pollingInterval) e insistir é o certo.
  // Erro de STATUS, não: a lib despacha 'error' com o status e segue no `_pollAgain` para SEMPRE,
  // sem nunca despachar 'close'. Sem separar os dois, um 401/404 vira laço infinito — o mesmo que
  // a PWA mediu em 2h14, e que ela evita lendo `readyState === 2`. Aqui a leitura só existe se
  // alguém marcar, então é este handler que marca.
  es.addEventListener('error', (ev: unknown) => {
    const status = (ev as { xhrStatus?: number } | null)?.xhrStatus ?? 0;
    falhou(Number.isInteger(status) && status >= 100 && status <= 599 ? String(status) : 'rede');
    if (recusaDefinitiva(status)) fechar();
  });
  es.addEventListener('close', () => {
    if (estado !== 2) falhou('servidor_fechou');
    estado = 2;
  });
  // emenda 2: wrap ramifica por tipo — eventos de dados tem {data,lastEventId}, error/close/open não
  const wrapData =
    (fn: (ev: { data: string; lastEventId?: string }) => void) => (ev: unknown) => {
      chegou();
      rearmar();
      const e = ev as { data: string; lastEventId?: string | null };
      fn({ data: e.data, lastEventId: e.lastEventId ?? undefined });
    };
  const wrapRaw = (fn: (ev: unknown) => void, type: string) => (ev: unknown) => {
    if (type !== 'error' && type !== 'close' && type !== 'open') chegou();
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
      const isData =
        type === 'message' || type === 'state' || type === 'preview' || type === 'reset' ||
        type === 'ping' || type === 'ask_question' || type === 'stats';
      const w = isData ? wrapData(fn as never) : wrapRaw(fn as never, type);
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
