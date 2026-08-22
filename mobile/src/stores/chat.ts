import { create } from 'zustand';
import type { StoreApi, UseBoundStore } from 'zustand';
import {
  getHistory,
  openEventStream,
  prependOlder,
  hasSeam,
  isAbortError,
  isTimeoutError,
  especificidade,
  donoDaLinha,
  sendInput,
} from '@hangar/core';
import type { ChatEvent, StateEvent, PreviewEvent, AskQuestionPayload } from '@hangar/core';
import * as m from '../paraglide/messages';
import { reconcilePending, type PendingMsg } from '../chat/pending';

// Store vivo do chat de UMA sessão — porte do núcleo de frontend/src/screens/Chat.svelte
// para zustand. Histórico janelado (cauda primeiro), merge SSE com dedup por id, preview ao
// vivo full-replace, watchdog/reconexão.
//
// Watchdog: fica NO ADAPTER (mobile/src/net/sse.ts) — ele fecha o stream após 25s sem evento
// e dispara onerror; qualquer listener registrado rearma o relógio dele via wrap. Este store
// NÃO cria segundo timer de inatividade (fecharia o MESMO stream duas vezes); só registra um
// listener de 'ping' vazio pro rearmar() rodar — mesmo padrão do sessions.ts.

// Cauda da primeira carga: mesma régua da PWA (400 > _BACKFILL_LINES=200 do SSE, fecha o
// buraco do resume sem re-baixar tudo).
export const TAIL_FIRST = 400;

const SSE_RETRY_MIN_MS = 3_000;
const SSE_RETRY_MAX_MS = 30_000;

export interface ChatState {
  events: ChatEvent[];
  stateEvent: StateEvent | null;
  // Prévia do bloco em voo (texto cru/markdown). Full-replace; some quando o assistant_msg
  // real chega ou quando a sessão sai de working.
  preview: string;
  previewMd: boolean;
  previewFull: boolean;
  askPayload: AskQuestionPayload | null;
  askOpen: boolean;
  askPiId: string | null;
  askPiDismissed: string | null;
  statusLine: string | null;
  loading: boolean;
  error: string;
  // Carga do histórico antigo falhou ('failed' = rede/backend, tocar tenta de novo) ou veio
  // de outro transcript ('unjoinable' = sem costura, conversa truncada).
  olderFailed: '' | 'failed' | 'unjoinable';
  // Ecos locais pendentes — já enviados mas ainda sem user_msg real no transcript.
  pending: PendingMsg[];
}

// Quantas bolhas estão "na fila" (translúcidas): ecos locais + sintéticos queued-* da fila durável.
// Extraído pra não duplicar a regra entre Composer (chip) e teste (file: chat.ts é a fonte).
export function filaCount(state: Pick<ChatState, 'events' | 'pending'>): number {
  return state.pending.length + state.events.filter((e) => e.kind === 'user_msg' && e.id.startsWith('queued-')).length;
}

export interface ChatApi {
  use: UseBoundStore<StoreApi<ChatState>>;
  retain: () => void;
  release: () => void;
  loadOlder: () => void;
  send: (text: string) => Promise<void>;
  // Recarrega o histórico depois de falha da primeira carga (o SSE segue vivo por conta
  // própria — onerror reconecta —, então retry é só a parte REST).
  retry: () => void;
  openAsk: (payload: AskQuestionPayload, piId?: string | null) => void;
  closeAsk: () => void;
  markAskDismissed: () => void;
}

function criarChatStore(serverId: string, name: string): ChatApi {
  const useChatStore = create<ChatState>(() => ({
    events: [],
    stateEvent: null,
    preview: '',
    previewMd: false,
    previewFull: false,
    askPayload: null,
    askOpen: false,
    askPiId: null,
    askPiDismissed: null,
    statusLine: null,
    loading: true,
    error: '',
    olderFailed: '',
    pending: [],
  }));

  // internos — fora do set() para não virar proxy
  let es: ReturnType<typeof openEventStream> | null = null;
  let lastEventId: string | null = null;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let retryDelay = SSE_RETRY_MIN_MS;
  let alive = false;
  let refs = 0;
  // Uma geração por carga: resposta velha nunca cai na sessão nova (padrão do Chat.svelte).
  let histGen = 0;
  let histAbort: AbortController | null = null;
  // Índice id->posição: o SSE re-emite o transcript inteiro a cada reconexão; Map = dedup O(1)
  // em vez de findIndex O(n) por evento (O(n²) por reconexão congelava a PWA em conversa longa).
  const idIndex = new Map<string, number>();
  // Fonte da prévia no último frame (md/full): a monotonicidade só vale DENTRO da mesma fonte.
  let previewMd = false;
  let previewFull = false;
  let pendingSeq = 0;
  let prevState: string | null = null;

  function rebuildIndex(events: ChatEvent[]) {
    idIndex.clear();
    for (let i = 0; i < events.length; i++) idIndex.set(events[i].id, i);
  }

  function aplicarEvents(events: ChatEvent[]) {
    rebuildIndex(events);
    useChatStore.setState({ events });
  }

  async function loadHistory(): Promise<void> {
    histGen++;
    histAbort?.abort();
    histAbort = new AbortController();
    const signal = histAbort.signal;
    const g = histGen;
    try {
      const tail = await getHistory(name, TAIL_FIRST, signal);
      if (g !== histGen) return;
      aplicarEvents(tail);
      useChatStore.setState({ error: '', olderFailed: '' });
      // Histórico antigo NÃO vem automático (diferente da PWA): entra sob demanda no
      // loadOlder(), chamado pelo onStartReached da lista — menos banda em link móvel.
      useChatStore.setState({ loading: false });
    } catch (err) {
      if (isAbortError(err) || g !== histGen || !alive) return;
      const msg = isTimeoutError(err)
        ? m.chat_historico_sem_resposta()
        : err instanceof Error
          ? err.message
          : m.chat_nao_carregou_historico();
      useChatStore.setState({ error: msg, loading: false });
    }
  }

  let olderInFlight = false;
  function loadOlder(): void {
    if (olderInFlight || !alive) return;
    if (useChatStore.getState().olderFailed === 'unjoinable') return;
    olderInFlight = true;
    getHistory(name, undefined, histAbort?.signal)
      .then((full) => {
        olderInFlight = false;
        if (!alive) return;
        const events = useChatStore.getState().events;
        const merged = prependOlder(full, events);
        if (!merged) {
          // null tem dois motivos: "já temos desde o começo" (feliz, calado) ou sem costura
          // (transcript trocado no meio do voo -> avisa).
          useChatStore.setState({
            olderFailed: hasSeam(full, events) ? '' : 'unjoinable',
          });
          return;
        }
        aplicarEvents(merged);
        useChatStore.setState({ olderFailed: '' });
      })
      .catch((err) => {
        olderInFlight = false;
        if (isAbortError(err) || !alive) return;
        useChatStore.setState({ olderFailed: 'failed' });
      });
  }

  function connectSSE(): void {
    if (!alive) return;
    clearTimeout(retryTimer);
    es?.close();

    es = openEventStream(name, lastEventId);

    // Prova de vida pro watchdog do adapter: SEM listener registrado o wrap do adapter
    // não roda rearmar() e o stream saudável morre aos 25s (mesmo padrão do sessions.ts).
    es.addEventListener('ping', () => {});

    es.addEventListener('message', (e) => {
      // Só o transcript carrega lastEventId ("<stem>:<offset>"); state/preview/ping vêm sem.
      if (e.lastEventId) lastEventId = e.lastEventId;
      try {
        const ev = JSON.parse(e.data as string) as ChatEvent;
        let { events } = useChatStore.getState();
        // Dedup cruzado fila<->transcript: a fila durável emite user_msg sintético (id
        // "queued-") e o transcript grava depois o user_msg REAL com texto igual — sem
        // isto, toda msg enfileirada (cp-send, composer working) aparece DOBRADA. O backend
        // documenta que o dedup é do front (sse.py: "O front faz o dedup cruzado").
        // Porte do handler de message do Chat.svelte.
        if (ev.kind === 'user_msg' && ev.text) {
          const filas: { i: number; text: string }[] = [];
          for (let i = 0; i < events.length; i++) {
            const x = events[i];
            if (x.kind === 'user_msg' && x.id.startsWith('queued-') && x.text) {
              filas.push({ i, text: x.text });
            }
          }
          if (ev.id.startsWith('queued-')) {
            // Sintético só entra se NENHUMA bolha real já cobre este texto sendo ela dona.
            const candidatos = [...filas.map((f) => f.text), ev.text];
            const coberto = events.some(
              (x) =>
                x.kind === 'user_msg' &&
                !x.id.startsWith('queued-') &&
                !!x.text &&
                especificidade(x.text, ev.text!) >= 0 &&
                donoDaLinha(x.text!, candidatos) === candidatos.length - 1,
            );
            if (coberto) return; // real já cobre e é o dono -> ignora o sintético
          } else {
            // Real chegou: remove SÓ a bolha da fila DONA da linha (não todas).
            const dono = donoDaLinha(ev.text, filas.map((f) => f.text));
            if (dono >= 0) {
              const qi = filas[dono].i;
              events = [...events.slice(0, qi), ...events.slice(qi + 1)];
            }
          }
        }
        // Dedup por id (replay do SSE + seed do history): existente SUBSTITUI (conteúdo
        // pode ter crescido), novo entra no fim.
        const i = idIndex.get(ev.id);
        if (i !== undefined) {
          const next = events.slice();
          next[i] = ev;
          rebuildIndex(next);
          useChatStore.setState({ events: next });
          return;
        }
        const next = [...events, ev];
        idIndex.set(ev.id, next.length - 1);
        // Reconcilia pending com o evento que acabou de chegar (se for user_msg real)
        const curPending = useChatStore.getState().pending;
        const nextPending = curPending.length ? reconcilePending(curPending, ev) : curPending;
        const pendingPatch = nextPending.length !== curPending.length ? { pending: nextPending } : {};
        useChatStore.setState({ events: next, ...pendingPatch });
        // Swap prévia->bolha: o bloco real chegou, a prévia sai no MESMO flush.
        if (ev.kind === 'assistant_msg' && ev.text && useChatStore.getState().preview) {
          previewMd = false;
          previewFull = false;
          useChatStore.setState({ preview: '', previewMd: false, previewFull: false });
        }
      } catch {
        // evento ilegível não derruba o stream
      }
    });

    es.addEventListener('state', (e) => {
      try {
        const ev = JSON.parse(e.data as string) as StateEvent;
        // Turno acabou sem bloco de assistente: ninguém mais viria apagar a prévia.
        const limparPreview = ev.state !== 'working' && useChatStore.getState().preview !== '';
        if (limparPreview) {
          previewMd = false;
          previewFull = false;
        }
        // Solidifica pending quando volta a idle (igual ao Chat.svelte): msgs enviadas
        // enquanto working que não viraram entrada gravada viram bolha sólida.
        const curPending = useChatStore.getState().pending;
        const solidPatch =
          prevState !== 'idle' && ev.state === 'idle' && curPending.some((p) => !p.solid)
            ? { pending: curPending.map((p) => ({ ...p, solid: true })) }
            : {};
        prevState = ev.state;
        useChatStore.setState({
          stateEvent: ev,
          statusLine: ev.status_line ?? null,
          ...(limparPreview ? { preview: '', previewMd: false, previewFull: false } : {}),
          ...solidPatch,
        });
      } catch {
        // engolir aqui congela estado/linha de status no valor antigo; stream segue vivo
      }
    });

    es.addEventListener('preview', (e) => {
      try {
        const ev = JSON.parse(e.data as string) as PreviewEvent;
        const t = ev.text ?? '';
        const atual = useChatStore.getState().preview;
        // Frame transitório do pane às vezes chega como PREFIXO do texto já mostrado:
        // ignorar, senão o texto recua e re-cresce. Só vale DENTRO da mesma fonte
        // (md/full trocados = fonte nova, passa sempre).
        if (
          t &&
          !!ev.md === previewMd &&
          !!ev.full === previewFull &&
          t.length < atual.length &&
          atual.startsWith(t)
        ) {
          return;
        }
        // VAZIO enquanto working não apaga a bolha (entre ferramentas o extrator manda "");
        // quem apaga de verdade são o assistant_msg real e a saída de working, acima.
        if (!t && useChatStore.getState().stateEvent?.state === 'working') return;
        previewMd = !!ev.md;
        previewFull = !!ev.full;
        useChatStore.setState({ preview: t, previewMd: !!ev.md, previewFull: !!ev.full });
      } catch {
        // frame ilegível: mantém o último bom
      }
    });

    // Stepper nativo (Claude): o hook askq_capture.py publica a pergunta e o SSE a entrega.
    es.addEventListener('ask_question', (e) => {
      try {
        const payload = JSON.parse(e.data as string) as AskQuestionPayload;
        if (!Array.isArray(payload.questions) || !payload.questions.length) return;
        useChatStore.setState({ askPayload: payload, askOpen: true, askPiId: null });
      } catch {
        // payload ilegível: o OptionButtons cru segue como saída
      }
    });

    es.addEventListener('reset', () => {
      // Transcript trocado (/clear): ids do arquivo antigo não valem mais — zera e recarrega.
      lastEventId = null;
      previewMd = false;
      previewFull = false;
      useChatStore.setState({
        stateEvent: null,
        statusLine: null,
        preview: '',
        previewMd: false,
        previewFull: false,
        askPayload: null,
        askOpen: false,
        askPiId: null,
        askPiDismissed: null,
        loading: true,
      });
      void loadHistory();
    });

    // reset do backoff quando a conexão estabiliza (paridade com Chat.svelte:1017 noteAlive)
    es.onopen = () => {
      retryDelay = SSE_RETRY_MIN_MS;
    };
    // Erro REAL (TCP RST ou watchdog do adapter): FECHA e reagenda com backoff. O adapter
    // mobile já tem auto-retry nativo desligado? Não: react-native-sse reconecta sozinho —
    // fechar aqui impede a 2ª máquina de retry martelar em paralelo (mesma lição da PWA).
    // Atribuição ÚNICA por conexão (objeto novo a cada connectSSE) — o setter do adapter
    // acumula listeners a cada atribuição (regra do grupo, achado da T4 r2).
    es.onerror = () => {
      es?.close();
      es = null;
      if (!alive) return;
      clearTimeout(retryTimer);
      retryTimer = setTimeout(connectSSE, retryDelay);
      retryDelay = Math.min(retryDelay * 2, SSE_RETRY_MAX_MS);
    };
  }

  return {
    use: useChatStore,
    retain() {
      refs++;
      if (refs === 1) {
        alive = true;
        retryDelay = SSE_RETRY_MIN_MS;
        void loadHistory().then(() => {
          if (alive) connectSSE();
        });
      }
    },
    release() {
      if (refs === 0) return;
      refs--;
      if (refs > 0) return;
      alive = false;
      histGen++;
      histAbort?.abort();
      es?.close();
      es = null;
      clearTimeout(retryTimer);
      idIndex.clear();
      pendingSeq = 0;
      prevState = null;
      useChatStore.setState({
        events: [],
        stateEvent: null,
        preview: '',
        previewMd: false,
        previewFull: false,
        askPayload: null,
        askOpen: false,
        askPiId: null,
        askPiDismissed: null,
        statusLine: null,
        loading: true,
        error: '',
        olderFailed: '',
        pending: [],
      });
    },
    loadOlder,
    retry: () => {
      void loadHistory();
    },
    openAsk(payload: AskQuestionPayload, piId: string | null = null) {
      useChatStore.setState({ askPayload: payload, askOpen: true, askPiId: piId });
    },
    // Fechar SEM responder: pergunta do Pi/Kimi fica marcada pra não reabrir sozinha (Chat.svelte:728).
    closeAsk() {
      const { askPiId } = useChatStore.getState();
      useChatStore.setState({ askOpen: false, ...(askPiId ? { askPiDismissed: askPiId, askPiId: null } : {}) });
    },
    // Respondida com sucesso: mesma marcação — o tool_result demora ~1s a aterrissar (Chat.svelte:1622-1626).
    markAskDismissed() {
      const { askPiId } = useChatStore.getState();
      useChatStore.setState({ askOpen: false, askPayload: null, ...(askPiId ? { askPiDismissed: askPiId, askPiId: null } : {}) });
    },
    async send(text: string) {
      const trimmed = text.trim();
      if (!trimmed) return;
      const id = `pending-${pendingSeq++}`;
      useChatStore.setState((s) => ({ pending: [...s.pending, { id, text: trimmed }] }));
      try {
        await sendInput(name, trimmed);
      } catch (err) {
        // falhou -> remove o eco que acabamos de pôr (não ficou enfileirado)
        useChatStore.setState((s) => ({ pending: s.pending.filter((p) => p.id !== id) }));
        throw err;
      }
    },
  };
}

// Um store por sessão, registry global — remonta só quando todos os consumers soltam.
const chats = new Map<string, ChatApi>();

export function chatStore(serverId: string, name: string): ChatApi {
  const chave = `${serverId}::${name}`;
  let api = chats.get(chave);
  if (!api) {
    api = criarChatStore(serverId, name);
    chats.set(chave, api);
  }
  return api;
}

export function _resetChatsForTests(): void {
  for (const api of chats.values()) api.release();
  chats.clear();
}
