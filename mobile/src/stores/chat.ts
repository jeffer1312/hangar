import { create } from 'zustand';
import type { StoreApi, UseBoundStore } from 'zustand';
import {
  getHistory,
  getHistoryDesde,
  openEventStreamForServer,
  prependOlder,
  mergeHistoryWithLiveRetiring,
  hasSeam,
  isAbortError,
  isTimeoutError,
  especificidade,
  donoDaLinha,
  sendInputForServer,
  steerSession,
  queuedMessages,
  registrarDiag,
  parsePluginToast,
  parsePluginUi,
  applyPluginUiDelta,
  type PluginSurfaces,
  parseQueuePanel,
  queueEditPayload,
  queueEditExitText,
  queueRemovedBubbleIds,
  currentQueuePanel,
  beginQueueEdit,
  canUpdateQueueEdit,
  moveTarget,
  editQueuedMessage,
  cancelQueuedMessage,
  descartarDaFila,
  steerQueuedMessage,
  reorderQueuedMessage,
  formataErro,
  composeWithContextRefs,
  splitContextRefs,
  type ContextRef,
} from '@hangar/core';
import type { ChatEvent, StateEvent, PreviewEvent, AskQuestionPayload, StatsEvent, EventSourceLike, Server, QueuePanel, QueueEdit } from '@hangar/core';
import * as m from '../paraglide/messages';
import { reconcilePending, type PendingMsg } from '../chat/pending';
import { useServers } from './servers';
import { readDraft, writeDraft, type ConversationDraft } from './drafts';
import { sendWithDelivery, type DeliveryFeedback, type QueueSendMode } from '../chat/queueDelivery';

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
// Prazo para o `queue` do retrato chegar depois de a conexão abrir.
const QUEUE_SNAPSHOT_MS = 3_000;
const SSE_RETRY_MAX_MS = 30_000;

// Edição da fila com as citações da mensagem: o campo mostra só o texto digitado e elas voltam na
// frente ao gravar (com elas o Atualizar vale mesmo com o campo vazio).
export type ChatQueueEdit = QueueEdit & { refs: readonly ContextRef[] };

export interface ChatState {
  draftUpdate: number;
  events: ChatEvent[];
  // Invalida a leitura incremental quando algo além de append muda o histórico.
  eventsRevision: number;
  stateEvent: StateEvent | null;
  // Prévia do bloco em voo (texto cru/markdown). Full-replace; some quando o assistant_msg
  // real chega ou quando a sessão sai de working.
  preview: string;
  previewMd: boolean;
  previewFull: boolean;
  // Raciocínio em voo (SSE `pensamento`, Claude sem terminal) e a chamada cujo pedido ainda está
  // sendo escrito (SSE `ferramenta`). Quem os tira de cena é o bloco real do transcript; o "" do
  // servidor chega antes dele, então só agenda a saída.
  pensamento: string;
  ferramenta: { nome: string; input: Record<string, unknown> } | null;
  // Quando a virada para `working` foi vista ao vivo (ms epoch); null = aberta no meio do turno.
  turnSeen: number | null;
  askPayload: AskQuestionPayload | null;
  askOpen: boolean;
  askPiId: string | null;
  askPiDismissed: string | null;
  statusLine: string | null;
  // Faixa de estatísticas do último turno (evento SSE `stats`).
  stats: StatsEvent | null;
  pluginUi: PluginSurfaces | null;
  pluginUiError: string | null;
  loading: boolean;
  error: string;
  // Carga do histórico antigo falhou ('failed' = rede/backend, tocar tenta de novo) ou veio
  // de outro transcript ('unjoinable' = sem costura, conversa truncada).
  olderFailed: '' | 'failed' | 'unjoinable';
  // O servidor recusou o stream de vez (401/404): insistir não muda a resposta, então paramos de
  // reconectar e a tela oferece tentar de novo. Sem isto o retry seguia para sempre, calado.
  sseRecusado: boolean;
  // Ecos locais pendentes — já enviados mas ainda sem user_msg real no transcript.
  pending: PendingMsg[];
  // Fila do Hangar pelo evento `queue` (Claude e Codex); null = sem ele, o painel sai das bolhas.
  queuePanel: QueuePanel | null;
  // Mensagem da fila aberta no composer; o rascunho de antes dela fica em `savedDraft`.
  queueEdit: ChatQueueEdit | null;
  // Texto que o composer recebe quando a edição fechou por a entrada ter saído da fila (null = o rascunho guardado).
  queueEditExit: string | null;
  deliveryFeedback: DeliveryFeedback | null;
  // Uma ação da fila por vez: os botões do painel ficam desativados enquanto ela está em voo.
  queueBusy: boolean;
  queueError: string | null;
}

// Quantas mensagens o chip "mandar agora" conta: só no Kimi, cuja fila é a da TUI (promovida inteira).
// Claude e Codex (com ou sem terminal) mostram a fila no painel "Na fila", que substitui o chip.
export function filaCount(state: Pick<ChatState, 'events' | 'pending'>, provider?: string | null): number {
  if (provider !== 'kimi') return 0;
  // O eco local é o envio em voo, não fila: em idle ele acendia "1 na fila" até o POST voltar.
  return queuedMessages(state.events, provider, false).length;
}

export interface ChatApi {
  use: UseBoundStore<StoreApi<ChatState>>;
  retain: () => void;
  release: () => void;
  loadOlder: () => void;
  send: (text: string, draftRevision?: number, mode?: QueueSendMode) => Promise<void>;
  // Recarrega o histórico depois de falha da primeira carga (o SSE segue vivo por conta
  // própria — onerror reconecta —, então retry é só a parte REST).
  retry: () => void;
  // Fundo fecha stream/timers e cancela só leituras; volta sincroniza uma vez e reabre com o cursor.
  setForeground: (active: boolean) => void;
  openAsk: (payload: AskQuestionPayload, piId?: string | null) => void;
  closeAsk: () => void;
  markAskDismissed: () => void;
  // Resposta do "mandar agora": marca as entregues e, com a fila já baixada, tira as bolhas na hora.
  applySteer: (result: { promoted?: boolean; queued_ids?: string[] }) => void;
  // Painel "Na fila". Entrar na edição devolve o texto do composer; toda saída dela passa por `dropQueueEdit`, que
  // deixa em `queueEditExit` o texto do campo (sem ele, o rascunho guardado).
  startQueueEdit: (id: string, draft: string) => string;
  cancelQueueEdit: () => void;
  dropQueueEdit: (text?: string) => void;
  submitQueueEdit: (text: string) => Promise<void>;
  removeQueued: (id: string) => Promise<void>;
  discardQueued: (id: string) => Promise<string | null>;
  steerQueued: (id: string) => Promise<void>;
  moveQueued: (id: string, delta: -1 | 1) => Promise<void>;
  showDeliveryFeedback: (feedback: DeliveryFeedback | null) => void;
}

// Estado do app visto pelos stores que ainda vão nascer: o layout liga antes da primeira tela.
let foregroundAtual = true;
const submitting = new Set<string>();
export const isSubmitting = (serverId: string, name: string) => submitting.has(`${serverId}::${name}`);

function criarChatStore(serverId: string, name: string): ChatApi {
  const useChatStore = create<ChatState>(() => ({
    draftUpdate: 0,
    events: [],
    eventsRevision: 0,
    stateEvent: null,
    preview: '',
    previewMd: false,
    previewFull: false,
    pensamento: '',
    ferramenta: null,
    turnSeen: null,
    askPayload: null,
    askOpen: false,
    askPiId: null,
    askPiDismissed: null,
    statusLine: null,
    stats: null,
    pluginUi: null,
    pluginUiError: null,
    loading: true,
    error: '',
    olderFailed: '',
    sseRecusado: false,
    pending: [],
    queuePanel: null,
    queueEdit: null,
    queueEditExit: null,
    deliveryFeedback: null,
    queueBusy: false,
    queueError: null,
  }));

  // internos — fora do set() para não virar proxy
  let es: EventSourceLike | null = null;
  let lastEventId: string | null = null;
  let etag: string | null = null;
  let foreground = foregroundAtual;
  // Com base carregada a volta pede só a cauda; sem ela (primeira carga falhou, reset) recarrega tudo.
  let baseLoaded = false;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  // Conexão aberta sem o evento `queue` no retrato (a sessão voltou ao Python): a lista antiga do servidor sai.
  let queueAbsentTimer: ReturnType<typeof setTimeout> | undefined;
  let retryDelay = SSE_RETRY_MIN_MS;
  let alive = false;
  let refs = 0;
  // Uma geração por carga: resposta velha nunca cai na sessão nova (padrão do Chat.svelte).
  let histGen = 0;
  let histAbort: AbortController | null = null;
  // Índice id->posição: o SSE re-emite o transcript inteiro a cada reconexão; Map = dedup O(1)
  // em vez de findIndex O(n) por evento (O(n²) por reconexão congelava a PWA em conversa longa).
  const idIndex = new Map<string, number>();
  // Só mensagens com texto participam do dedup; append comum não relê o histórico.
  const queuedTexts = new Map<number, string>();
  const realTexts = new Map<number, string>();
  const retiredQueuedIds = new Set<string>();
  // Fonte da prévia no último frame (md/full): a monotonicidade só vale DENTRO da mesma fonte.
  let previewMd = false;
  let previewFull = false;
  let pendingSeq = 0;
  let prevState: string | null = null;
  // O fim do turno chega antes do bloco real pelo tail do .jsonl: apagar a prévia na hora abria um
  // buraco e a bolha voltava. Sair de working só agenda; o timer vence se o bloco nunca vier.
  let previewDropTimer: ReturnType<typeof setTimeout> | undefined;
  let pensamentoTimer: ReturnType<typeof setTimeout> | undefined;
  let ferramentaTimer: ReturnType<typeof setTimeout> | undefined;
  let queueErrorTimer: ReturnType<typeof setTimeout> | undefined;
  let deliveryFeedbackTimer: ReturnType<typeof setTimeout> | undefined;
  const pluginToastsSeen = new Set<string>();
  // Um setState por token re-renderizava a conversa a cada caractere: o primeiro sai na hora, o resto
  // junta num quadro.
  let previewQueued: { preview: string; previewMd: boolean; previewFull: boolean } | null = null;
  let previewFlushTimer: ReturnType<typeof setTimeout> | undefined;
  const PREVIEW_FRAME_MS = 50;
  function currentPreview(): string {
    return previewQueued?.preview ?? useChatStore.getState().preview;
  }
  function flushPreview(): void {
    previewFlushTimer = undefined;
    if (!previewQueued) return;
    const patch = previewQueued;
    previewQueued = null;
    useChatStore.setState(patch);
    previewFlushTimer = setTimeout(flushPreview, PREVIEW_FRAME_MS);
  }
  function publishPreview(patch: { preview: string; previewMd: boolean; previewFull: boolean }): void {
    previewQueued = patch;
    if (previewFlushTimer === undefined) flushPreview();
  }
  function dropQueuedPreview(): void {
    clearTimeout(previewFlushTimer);
    previewFlushTimer = undefined;
    previewQueued = null;
  }

  function limparPreview(): void {
    clearTimeout(previewDropTimer);
    previewDropTimer = undefined;
    dropQueuedPreview();
    previewMd = false;
    previewFull = false;
    useChatStore.setState({ preview: '', previewMd: false, previewFull: false });
  }
  function dropPreviewSoon(): void {
    if (previewDropTimer !== undefined || !currentPreview()) return;
    previewDropTimer = setTimeout(limparPreview, 5000);
  }
  function cancelPreviewDrop(): void {
    clearTimeout(previewDropTimer);
    previewDropTimer = undefined;
  }
  function limparPensamento(): void {
    clearTimeout(pensamentoTimer);
    pensamentoTimer = undefined;
    useChatStore.setState({ pensamento: '' });
  }
  function limparFerramenta(): void {
    clearTimeout(ferramentaTimer);
    ferramentaTimer = undefined;
    useChatStore.setState({ ferramenta: null });
  }

  function indexText(ev: ChatEvent, i: number) {
    queuedTexts.delete(i);
    realTexts.delete(i);
    if (ev.kind === 'user_msg' && ev.text) {
      (ev.id.startsWith('queued-') ? queuedTexts : realTexts).set(i, ev.text);
    }
  }

  function rebuildIndex(events: ChatEvent[]) {
    idIndex.clear();
    queuedTexts.clear();
    realTexts.clear();
    for (let i = 0; i < events.length; i++) {
      idIndex.set(events[i].id, i);
      indexText(events[i], i);
    }
  }

  function publishEvents(events: ChatEvent[], change: 'append' | 'replace' | 'rebuild' = 'rebuild', patch: Partial<ChatState> = {}) {
    const current = useChatStore.getState();
    const changed = events !== current.events || change === 'replace';
    if (changed && change === 'rebuild') rebuildIndex(events);
    useChatStore.setState({ ...patch, events,
      eventsRevision: current.eventsRevision + (changed && change !== 'append' ? 1 : 0) });
  }

  function aplicarEvents(events: ChatEvent[]) {
    if (retiredQueuedIds.size) events = events.filter((e) => !retiredQueuedIds.has(e.id));
    const pending = events.reduce(reconcilePending, useChatStore.getState().pending);
    publishEvents(events, 'rebuild', { pending });
  }

  // Destino desta conversa, nunca o servidor ativo: sem ele nenhuma leitura cai em outra máquina.
  function destino(): Server | undefined {
    return useServers.getState().servers.find((s) => s.id === serverId);
  }

  function novaLeitura(): { g: number; signal: AbortSignal } {
    histGen++;
    histAbort?.abort();
    histAbort = new AbortController();
    return { g: histGen, signal: histAbort.signal };
  }

  async function loadHistory(): Promise<void> {
    const { g, signal } = novaLeitura();
    const cachedEvents = new Set(useChatStore.getState().events);
    try {
      const target = destino();
      if (!target) throw new Error(m.chat_servidor_removido());
      const tail = await getHistory(name, TAIL_FIRST, signal, undefined, target);
      if (g !== histGen) return;
      // O SSE continua chegando durante o GET, inclusive logo após /clear.
      const merged = mergeHistoryWithLiveRetiring(tail, useChatStore.getState().events,
        { cachedEvents, removedIds: retiredQueuedIds, retireQueuedFromCurrent: true });
      for (const id of merged.retired) retiredQueuedIds.add(id);
      aplicarEvents(merged.events);
      baseLoaded = true;
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

  // Volta do fundo: só o que é novo entra, pelo mesmo caminho do SSE (dedup de fila e pending);
  // sem costura a cauda vira a verdade, e o antigo volta pelo loadOlder.
  async function syncTail(): Promise<void> {
    const { g, signal } = novaLeitura();
    const cachedEvents = new Set(useChatStore.getState().events);
    const target = destino();
    try {
      if (!target) throw new Error(m.chat_servidor_removido());
      const r = await getHistoryDesde(name, TAIL_FIRST, etag, signal, undefined, target);
      if (g !== histGen || !alive || r === 'igual') return;
      etag = r.etag;
      const atual = useChatStore.getState().events;
      if (!r.eventos.length) return;
      if (!atual.length || !hasSeam(r.eventos, atual)) {
        // Lista trocada pela cauda: o cursor antigo reenviaria o buraco DEPOIS dela, fora de ordem.
        lastEventId = null;
        const merged = mergeHistoryWithLiveRetiring(r.eventos, atual,
          { cachedEvents, removedIds: retiredQueuedIds, retireQueuedFromCurrent: true });
        for (const id of merged.retired) retiredQueuedIds.add(id);
        aplicarEvents(merged.events);
        useChatStore.setState({ olderFailed: '' });
        return;
      }
      for (const ev of r.eventos) if (!idIndex.has(ev.id)) ingest(ev);
    } catch (err) {
      if (isAbortError(err) || g !== histGen || !alive) return;
      // A conversa na tela continua válida e o SSE reabre com o cursor; a falha fica no diário.
      if (target) registrarDiag({ evento: 'chat.sincronia_falhou', nivel: 'erro', tela: 'chat',
        detalhe: err instanceof Error ? err.message : String(err) }, target.baseUrl);
    }
  }

  function sincronizar(): void {
    if (!alive || !foreground) return;
    const load = baseLoaded ? syncTail() : loadHistory();
    const g = histGen;
    void load.then(() => {
      if (alive && foreground && g === histGen && !es && !useChatStore.getState().sseRecusado) connectSSE();
    });
  }

  let olderInFlight = false;
  function loadOlder(): void {
    if (olderInFlight || !alive || !foreground) return;
    if (useChatStore.getState().olderFailed === 'unjoinable') return;
    const target = destino();
    if (!target) return;
    olderInFlight = true;
    const g = histGen;
    getHistory(name, undefined, histAbort?.signal, undefined, target)
      .then((full) => {
        olderInFlight = false;
        if (!alive || g !== histGen) return;
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
        if (isAbortError(err) || !alive || g !== histGen) return;
        useChatStore.setState({ olderFailed: 'failed' });
      });
  }

  function ingest(ev: ChatEvent): void {
    let { events } = useChatStore.getState();
    let removedQueued = false;
    if (ev.queued_confirmed && ev.id.startsWith('queued-')) {
      retiredQueuedIds.add(ev.id);
      if (!idIndex.has(ev.id)) return;
      events = events.filter((x) => x.id !== ev.id);
      publishEvents(events);
      return;
    }
    if (retiredQueuedIds.has(ev.id)) return;
    // Dedup cruzado fila<->transcript: a fila durável emite user_msg sintético (id
    // "queued-") e o transcript grava depois o user_msg REAL com texto igual — sem
    // isto, toda msg enfileirada (cp-send, composer working) aparece DOBRADA. O backend
    // documenta que o dedup é do front (sse.py: "O front faz o dedup cruzado").
    // Porte do handler de message do Chat.svelte.
    if (ev.kind === 'user_msg' && ev.text) {
      const filas = [...queuedTexts].sort(([a], [b]) => a - b)
        .map(([i, text]) => ({ i, text }));
      if (ev.id.startsWith('queued-')) {
        // Sintético só entra se NENHUMA bolha real já cobre este texto sendo ela dona.
        const candidatos = [...filas.map((f) => f.text), ev.text];
        const coberto = [...realTexts.values()].some(
          (text) => especificidade(text, ev.text!) >= 0 &&
            donoDaLinha(text, candidatos) === candidatos.length - 1,
        );
        if (coberto) return; // real já cobre e é o dono -> ignora o sintético
      } else {
        // Real chegou: remove SÓ a bolha da fila DONA da linha (não todas).
        const dono = donoDaLinha(ev.text, filas.map((f) => f.text));
        if (dono >= 0) {
          const qi = filas[dono].i;
          retiredQueuedIds.add(events[qi].id);
          events = [...events.slice(0, qi), ...events.slice(qi + 1)];
          rebuildIndex(events);
          removedQueued = true;
        }
      }
    }
    // Dedup por id (replay do SSE + seed do history): existente SUBSTITUI (conteúdo
    // pode ter crescido), novo entra no fim.
    const i = idIndex.get(ev.id);
    if (i !== undefined) {
      const previous = events[i];
      const keys = Object.keys(ev) as (keyof ChatEvent)[];
      if (!removedQueued && keys.length === Object.keys(previous).length &&
          keys.every((key) => Object.hasOwn(previous, key) && (
            Object.is(previous[key], ev[key]) ||
            (typeof previous[key] === 'object' && typeof ev[key] === 'object' &&
              JSON.stringify(previous[key]) === JSON.stringify(ev[key]))
          ))) return;
      const next = events.slice();
      next[i] = ev;
      indexText(ev, i);
      publishEvents(next, 'replace');
      return;
    }
    const next = [...events, ev];
    idIndex.set(ev.id, next.length - 1);
    indexText(ev, next.length - 1);
    // Reconcilia pending com o evento que acabou de chegar (se for user_msg real)
    const curPending = useChatStore.getState().pending;
    const nextPending = curPending.length ? reconcilePending(curPending, ev) : curPending;
    const pendingPatch = nextPending.length !== curPending.length ? { pending: nextPending } : {};
    publishEvents(next, removedQueued ? 'replace' : 'append', pendingPatch);
    if (ev.kind === 'thinking' && useChatStore.getState().pensamento) limparPensamento();
    if (ev.kind === 'tool_use' && useChatStore.getState().ferramenta) limparFerramenta();
    // Swap prévia->bolha: o bloco real chegou, a prévia sai no MESMO flush.
    if (ev.kind === 'assistant_msg' && ev.text && currentPreview()) limparPreview();
  }

  function connectSSE(): void {
    if (!alive || !foreground) return;
    const target = destino();
    const base = target?.baseUrl;
    const quadroFalhou = (codigo: string) => {
      if (base !== undefined) registrarDiag({ evento: 'sse.quadro_falhou', nivel: 'erro',
        tela: 'chat', codigo }, base);
    };
    clearTimeout(retryTimer);
    es?.close();
    es = null;
    if (!target) return;

    es = openEventStreamForServer(target, name, undefined, lastEventId);
    const source = es;
    let queueSeen = false;
    let pluginUiRaw: unknown = null;

    for (const type of ['plugin_ui', 'plugin_ui_delta']) {
      es.addEventListener(type, (e) => {
        if (es !== source || !alive) return;
        try {
          const data: unknown = JSON.parse(e.data);
          const next = type === 'plugin_ui_delta' ? applyPluginUiDelta(pluginUiRaw, data) : data;
          const pluginUi = parsePluginUi(next);
          pluginUiRaw = next;
          useChatStore.setState({ pluginUi, pluginUiError: null });
        } catch {
          quadroFalhou(type);
          useChatStore.setState({ pluginUiError: m.plugin_ui_falhou() });
          onStreamError(new Error(type));
        }
      });
    }

    // Prova de vida pro watchdog do adapter: SEM listener registrado o wrap do adapter
    // não roda rearmar() e o stream saudável morre aos 25s (mesmo padrão do sessions.ts).
    es.addEventListener('ping', () => {});

    const onMessage = (e: { data: string; lastEventId?: string }) => {
      // Só o transcript carrega lastEventId ("<stem>:<offset>"); state/preview/ping vêm sem.
      if (e.lastEventId) lastEventId = e.lastEventId;
      try {
        ingest(JSON.parse(e.data as string) as ChatEvent);
      } catch {
        quadroFalhou('message');
        // evento ilegível não derruba o stream
      }
    };
    es.addEventListener('message', onMessage);
    es.addEventListener('queue_confirmed', onMessage);

    es.addEventListener('state', (e) => {
      try {
        const ev = JSON.parse(e.data as string) as StateEvent;
        // Turno acabou sem bloco de assistente: ninguém mais viria apagar a prévia — mas pela
        // carência, porque o bloco real ainda pode estar a caminho.
        if (ev.state === 'working') cancelPreviewDrop();
        else {
          dropPreviewSoon();
          // Turno parado ou evento vazio perdido: sem isto o raciocínio/ferramenta ao vivo
          // ficava na tela para sempre, escondendo a linha de trabalho.
          const s = useChatStore.getState();
          if (s.pensamento && pensamentoTimer === undefined) pensamentoTimer = setTimeout(limparPensamento, 3000);
          if (s.ferramenta && ferramentaTimer === undefined) ferramentaTimer = setTimeout(limparFerramenta, 3000);
        }
        const turnSeen = ev.state !== 'working' ? null
          : prevState !== null && prevState !== 'working' ? Date.now()
          : useChatStore.getState().turnSeen;
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
          turnSeen,
          ...solidPatch,
        });
      } catch {
        quadroFalhou('state');
        // engolir aqui congela estado/linha de status no valor antigo; stream segue vivo
      }
    });

    es.addEventListener('preview', (e) => {
      try {
        const ev = JSON.parse(e.data as string) as PreviewEvent;
        const t = ev.text ?? '';
        const atual = currentPreview();
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
        if (!t) {
          if (useChatStore.getState().stateEvent?.state !== 'working') dropPreviewSoon();
          return;
        }
        cancelPreviewDrop();
        previewMd = !!ev.md;
        previewFull = !!ev.full;
        publishPreview({ preview: t, previewMd: !!ev.md, previewFull: !!ev.full });
      } catch {
        quadroFalhou('preview');
        // frame ilegível: mantém o último bom
      }
    });

    es.addEventListener('pensamento', (e) => {
      try {
        const t = (JSON.parse(e.data as string) as { text?: string }).text ?? '';
        if (t) {
          clearTimeout(pensamentoTimer);
          pensamentoTimer = undefined;
          useChatStore.setState({ pensamento: t });
        } else if (useChatStore.getState().pensamento && pensamentoTimer === undefined) {
          pensamentoTimer = setTimeout(limparPensamento, 3000);
        }
      } catch {
        quadroFalhou('pensamento');
      }
    });

    es.addEventListener('ferramenta', (e) => {
      try {
        const t = (JSON.parse(e.data as string) as { text?: string }).text ?? '';
        if (t) {
          clearTimeout(ferramentaTimer);
          ferramentaTimer = undefined;
          const v = JSON.parse(t) as { nome?: string; input?: Record<string, unknown> };
          useChatStore.setState({ ferramenta: { nome: v.nome ?? 'tool', input: v.input ?? {} } });
        } else if (useChatStore.getState().ferramenta && ferramentaTimer === undefined) {
          ferramentaTimer = setTimeout(limparFerramenta, 3000);
        }
      } catch {
        quadroFalhou('ferramenta');
      }
    });

    // Aviso (`$.ui.toast`) de um mod: o terminal o desenha por alguns segundos e ele não entra no
    // transcript. A reconexão repõe os que ainda não venceram: o id diz quais já passaram por aqui.
    es.addEventListener('plugin_toast', (e) => {
      try {
        const t = parsePluginToast(JSON.parse(e.data as string));
        if (!t || pluginToastsSeen.has(t.id)) return;
        pluginToastsSeen.add(t.id);
        void import('../ui/Toast').then(({ toast }) => toast.mod(t.text, t.plugin, t.timeoutMs));
      } catch {
        quadroFalhou('plugin_toast');
      }
    });

    // Fila do Hangar (Claude e Codex): lista inteira e ordem a cada mudança, de qualquer aparelho.
    es.addEventListener('queue', (e) => {
      queueSeen = true;
      try {
        const data = JSON.parse(e.data as string);
        // Apagadas noutro aparelho: a bolha delas sai daqui também.
        const events = dropQueuedBubbles(useChatStore.getState().events, queueRemovedBubbleIds(data));
        publishEvents(events, 'rebuild', { queuePanel: parseQueuePanel(data) });
      } catch {
        quadroFalhou('queue');
      }
    });

    es.addEventListener('stats', (e) => {
      try {
        useChatStore.setState({ stats: JSON.parse(e.data as string) as StatsEvent });
      } catch {
        quadroFalhou('stats');
        // faixa ilegível: mantém a última boa
      }
    });

    // O Codex publica a mesma solicitação ao reconectar; só uma nova identidade reabre a folha.
    es.addEventListener('ask_question', (e) => {
      try {
        const payload = JSON.parse(e.data as string) as AskQuestionPayload | null;
        if (!payload) {
          useChatStore.setState({ askPayload: null, askOpen: false, askPiId: null });
          return;
        }
        if (!Array.isArray(payload.questions) || !payload.questions.length) return;
        const current = useChatStore.getState().askPayload;
        if (current && payload.interaction_id === current.interaction_id
            && payload.request_id === current.request_id
            && (payload.interaction_id != null || (payload.provider === 'codex' && current.provider === 'codex'))) return;
        useChatStore.setState({ askPayload: payload, askOpen: true, askPiId: null });
      } catch {
        quadroFalhou('ask_question');
        // payload ilegível: o OptionButtons cru segue como saída
      }
    });

    es.addEventListener('reset', () => {
      pluginUiRaw = null;
      useChatStore.setState({ pluginUi: null, pluginUiError: null });
      // Transcript trocado (/clear): ids do arquivo antigo não valem mais — zera e recarrega.
      lastEventId = null;
      etag = null;
      baseLoaded = false;
      retiredQueuedIds.clear();
      prevState = null;
      previewMd = false;
      previewFull = false;
      cancelPreviewDrop();
      dropQueuedPreview();
      clearTimeout(pensamentoTimer);
      pensamentoTimer = undefined;
      clearTimeout(ferramentaTimer);
      ferramentaTimer = undefined;
      publishEvents([], 'rebuild', {
        // O /clear não vira user_msg na conversa nova para reconciliar seu eco.
        pending: useChatStore.getState().pending.filter((p) => p.text.trim() !== '/clear'),
        pensamento: '',
        ferramenta: null,
        turnSeen: null,
        stateEvent: null,
        statusLine: null,
        stats: null,
        preview: '',
        previewMd: false,
        previewFull: false,
        askPayload: null,
        askOpen: false,
        askPiId: null,
        askPiDismissed: null,
        queuePanel: null,
        queueEdit: null,
        queueEditExit: null,
        error: '',
        olderFailed: '',
        loading: true,
      });
      void loadHistory();
    });

    // reset do backoff quando a conexão estabiliza (paridade com Chat.svelte:1017 noteAlive)
    es.onopen = () => {
      if (es !== source) return;
      retryDelay = SSE_RETRY_MIN_MS;
      // O retrato chega logo depois de abrir: sem `queue` nele, o painel volta a sair das bolhas.
      clearTimeout(queueAbsentTimer);
      queueAbsentTimer = setTimeout(() => {
        if (es === source && !queueSeen) useChatStore.setState({ queuePanel: null });
      }, QUEUE_SNAPSHOT_MS);
    };
    // Fechar a fonte deixa apenas este backoff dono da repetição, sem polling em paralelo.
    function onStreamError(error: unknown) {
      if (es !== source) return;
      const status = (error as { xhrStatus?: number } | null)?.xhrStatus ?? 0;
      const refused = status >= 400 && status < 500 && status !== 408 && status !== 429;
      es = null;
      source.close();
      if (!alive || !foreground) return;
      clearTimeout(retryTimer);
      if (refused) {
        useChatStore.setState({ sseRecusado: true });
        return;
      }
      retryTimer = setTimeout(connectSSE, retryDelay);
      if (base !== undefined) registrarDiag({ evento: 'sse.retentativa', tela: 'chat',
        espera_ms: retryDelay }, base);
      retryDelay = Math.min(retryDelay * 2, SSE_RETRY_MAX_MS);
    }
    es.onerror = onStreamError;
  }

  // Aviso de falha da fila: some sozinho, como o aviso do PWA, sem ficar sobre uma fila que já mudou.
  function showQueueError(message: string): void {
    clearTimeout(queueErrorTimer);
    useChatStore.setState({ queueError: message });
    queueErrorTimer = setTimeout(() => useChatStore.setState({ queueError: null }), 6000);
  }

  // Descreve só o último envio: some sozinho para não ficar sobre uma conversa que já seguiu.
  function showDeliveryFeedback(feedback: DeliveryFeedback | null): void {
    clearTimeout(deliveryFeedbackTimer);
    useChatStore.setState({ deliveryFeedback: feedback });
    if (feedback) deliveryFeedbackTimer = setTimeout(() => useChatStore.setState({ deliveryFeedback: null }), 12_000);
  }

  // Devolve se a ação foi aceita: a edição só sai quando o servidor gravou o texto novo.
  async function queueAction(run: (target: Server) => Promise<unknown>, onError?: (message: string) => void): Promise<boolean> {
    if (useChatStore.getState().queueBusy) return false;
    const target = destino();
    if (!target) {
      const message = m.chat_servidor_removido();
      onError?.(message);
      showQueueError(message);
      return false;
    }
    clearTimeout(queueErrorTimer);
    useChatStore.setState({ queueBusy: true, queueError: null });
    try {
      await run(target);
      return true;
    } catch (err) {
      const message = formataErro(err) ?? (err instanceof Error ? err.message : String(err));
      onError?.(message);
      showQueueError(message);
      return false;
    } finally {
      useChatStore.setState({ queueBusy: false });
    }
  }

  // Toda saída da edição: o Composer põe `text` no campo, ou o rascunho guardado sem ele. As
  // citações moram dentro do `queueEdit`, então saem junto.
  function dropQueueEdit(text?: string) {
    useChatStore.setState({ queueEdit: null, queueEditExit: text ?? null });
  }

  // Tira da conversa as bolhas da fila dadas pelo id de bolha (`queued-<id>`); devolve a lista nova.
  function dropQueuedBubbles(events: ChatEvent[], ids: Iterable<string>): ChatEvent[] {
    const drop = new Set(ids);
    if (!drop.size) return events;
    for (const id of drop) retiredQueuedIds.add(id);
    return events.filter((e) => !drop.has(e.id));
  }

  const queueEntries = () => {
    const s = useChatStore.getState();
    return currentQueuePanel(s.queuePanel, s.events).entries;
  };

  return {
    use: useChatStore,
    startQueueEdit(id: string, draft: string): string {
      const entry = queueEntries().find((e) => e.id === id);
      if (!entry) return draft;
      const next = beginQueueEdit(entry, draft, useChatStore.getState().queueEdit);
      const split = splitContextRefs(entry.text);
      useChatStore.setState({ queueEdit: { ...next.edit, refs: split?.refs ?? [] }, queueEditExit: null });
      return split ? split.typed : next.composerText;
    },
    cancelQueueEdit() {
      dropQueueEdit();
    },
    dropQueueEdit,
    async submitQueueEdit(text: string): Promise<void> {
      const edit = useChatStore.getState().queueEdit;
      if (!edit || !canUpdateQueueEdit(text, edit.refs)) return;
      // Texto igual ao da fila: só sai da edição, sem pedido.
      const next = queueEditPayload(composeWithContextRefs(edit.refs, text), queueEntries().find((e) => e.id === edit.entryId)?.text);
      if (next !== null && !(await queueAction(async (t) => {
        try {
          await editQueuedMessage(name, edit.entryId, next, t);
        } catch (err) {
          // Saiu da fila: a edição fecha já, com o aviso desta recusa; a saída pelo painel não avisa de novo.
          if ((err as { code?: unknown })?.code === 'queue_entry_gone' && useChatStore.getState().queueEdit === edit) {
            dropQueueEdit(queueEditExitText(edit, text));
          }
          throw err;
        }
      }))) return;
      if (useChatStore.getState().queueEdit === edit) dropQueueEdit();
    },
    async removeQueued(id: string) {
      const ok = await queueAction((t) => cancelQueuedMessage(name, id, t));
      if (!ok) return;
      // A bolha da entrada apagada sai junto: sem isto ela voltava à conversa ao deixar o painel.
      publishEvents(dropQueuedBubbles(useChatStore.getState().events, [`queued-${id}`]));
    },
    async discardQueued(id: string) {
      let error: string | null = null;
      const ok = await queueAction((t) => descartarDaFila(name, id, t), (message) => { error = message; });
      if (ok) publishEvents(dropQueuedBubbles(useChatStore.getState().events, [`queued-${id}`]));
      return error;
    },
    // `steered: false` = entrou na frente da fila e a conversa mostra a entrega; só a dica que não
    // apareceu a tempo (a mensagem fica para a próxima pausa) merece aviso.
    async steerQueued(id: string) {
      let hintMissing = false;
      await queueAction(async (t) => {
        hintMissing = (await steerQueuedMessage(name, id, t)).code === 'queue_steer_hint_missing';
      });
      if (hintMissing) showQueueError(m.native_queue_steer_hint_missing());
    },
    async moveQueued(id: string, delta: -1 | 1) {
      const before = moveTarget(queueEntries(), id, delta);
      if (before === undefined) return;
      await queueAction((t) => reorderQueuedMessage(name, id, before, t));
    },
    retain() {
      refs++;
      if (refs === 1) {
        alive = true;
        retryDelay = SSE_RETRY_MIN_MS;
        // Recusa da abertura anterior (sessão morta, 404) não vale para a sessão recriada com o mesmo nome.
        useChatStore.setState({ sseRecusado: false });
        sincronizar();
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
      clearTimeout(queueAbsentTimer);
      showDeliveryFeedback(null);
      if (previewQueued) useChatStore.setState(previewQueued);
      dropQueuedPreview();
      // Soltar a tela fecha leituras; POST, ecos e identidade continuam na origem.
    },
    loadOlder,
    retry: () => {
      const recusado = useChatStore.getState().sseRecusado;
      sincronizar();
      // Recusa definitiva parou o laço de reconexão; tocar em "tentar de novo" é o único caminho
      // de volta, então o retry precisa reabrir o stream, não só recarregar o histórico.
      if (recusado && alive && foreground) {
        useChatStore.setState({ sseRecusado: false });
        retryDelay = SSE_RETRY_MIN_MS;
        connectSSE();
      }
    },
    setForeground(active: boolean) {
      if (active === foreground) return;
      foreground = active;
      if (!active) {
        // Só GET é descartável: POST em voo segue, e transcript, pergunta, pending e cursor ficam.
        histGen++;
        histAbort?.abort();
        histAbort = null;
        clearTimeout(retryTimer);
        es?.close();
        es = null;
        return;
      }
      retryDelay = SSE_RETRY_MIN_MS;
      sincronizar();
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
    showDeliveryFeedback,
    // O user_msg real só chega no fim do turno: sem tirar as "queued-" aqui, o chip seguia aceso
    // sobre uma fila que já foi.
    applySteer(result) {
      const sent = new Set(result.queued_ids ?? []);
      let events = useChatStore.getState().events;
      if (sent.size) events = events.map((e) => (sent.has(e.id) ? { ...e, queued_delivered: true } : e));
      if (result.promoted) events = events.filter((e) => {
        if (e.kind !== 'user_msg' || !e.id.startsWith('queued-')) return true;
        retiredQueuedIds.add(e.id);
        return false;
      });
      publishEvents(events);
    },
    async send(text: string, draftRevision?: number, mode?: QueueSendMode) {
      const trimmed = mode === 'steer' ? text : text.trim();
      if (!trimmed.trim()) return;
      // Destino desta conversa, nunca o servidor ativo: trocar de servidor no meio não desvia o envio.
      const target = useServers.getState().servers.find((s) => s.id === serverId);
      if (!target) throw new Error(m.chat_servidor_removido());
      const wasUnknown = readDraft(serverId, name)?.submission?.status === 'unknown';
      return submitConversationDraft(serverId, name, trimmed, async () => {
        if (wasUnknown) useChatStore.setState((s) => ({ pending: s.pending.filter((p) => p.text !== trimmed) }));
        showDeliveryFeedback(null);
        const id = `pending-${pendingSeq++}`;
        // Em idle a mensagem é digitada na hora: o eco atenuado parecia mensagem entrando na fila.
        const solid = useChatStore.getState().stateEvent?.state === 'idle';
        useChatStore.setState((s) => ({ pending: [...s.pending, { id, text: trimmed, solid }] }));
        try {
          if (mode === 'steer') await steerSession(name, trimmed, target);
          else if (mode) showDeliveryFeedback(await sendWithDelivery(target, name, trimmed, mode));
          else await sendInputForServer(target, name, trimmed);
        } catch (err) {
          // O /steer também usa 409 quando escreveu sem confirmar: não presumir recusa.
          if ((mode === 'steer' || (typeof mode === 'object' && mode.kind === 'direct' && !mode.terminalClaude)) && (err as { status?: number } | null)?.status === 409) {
            throw new Error(m.queue_steer_uncertain(), { cause: err });
          }
          if (isInputRejected(err)) useChatStore.setState((s) => ({ pending: s.pending.filter((p) => p.id !== id) }));
          throw err;
        }
      }, draftRevision);
    },
  };
}

function isInputRejected(error: unknown): boolean {
  const status = (error as { status?: number } | null)?.status;
  return typeof status === 'number' && status >= 400 && status < 500 && status !== 408;
}

// O snapshot precede qualquer POST e já esvazia o campo enviado; só a falha devolve o texto.
export async function submitConversationDraft(serverId: string, name: string, text: string, deliver: () => Promise<void>, draftRevision?: number): Promise<void> {
  const key = `${serverId}::${name}`;
  if (submitting.has(key)) throw new Error(m.chat_envio_incerto());
  submitting.add(key);
  const notify = () => chatStore(serverId, name).use.setState((s) => ({ draftUpdate: s.draftUpdate + 1 }));
  try {
    const current: ConversationDraft = readDraft(serverId, name) ?? {
      // O Composer descarta rascunho vazio: citação sem texto conserva a revisão enviada,
      // sem devolver ao campo o bloco montado só para o transporte.
      version: 1, text: draftRevision === undefined ? text : '', revision: draftRevision ?? 1,
      transcript: null, attachments: [], submission: null,
    };
    if (current.submission && current.submission.text.trim() !== text.trim()) throw new Error(m.composer_submission_recover_first());
    const submission = { text, draftRevision: draftRevision ?? current.revision, status: 'sending' as const };
    // O campo pode já ter outra edição (primeiro input, pedido de plano): essa fica onde está.
    const fromField = draftRevision === undefined ? current.text.trim() === text.trim() : current.revision === draftRevision;
    // A fila já mostra a mensagem: texto e áudios enviados saem do campo antes da resposta.
    writeDraft(serverId, name, { ...current, submission,
      ...(fromField ? { text: '', revision: current.revision + 1, ...(current.recordings ? { recordings: [] } : {}) } : {}) });
    notify();
    const settle = (status: 'unknown' | 'rejected' | null) => {
      const latest = readDraft(serverId, name);
      if (!latest || (current.transcript !== null && latest.transcript !== current.transcript)
        || latest.submission?.draftRevision !== submission.draftRevision || latest.submission.text !== text) return;
      // Falha devolve o texto só ao campo ainda vazio; o que a pessoa digitou depois fica, e o Recuperar traz o outro.
      const restore = status !== null && fromField && !latest.text.trim();
      const sentRecordings = current.recordings ?? [];
      writeDraft(serverId, name, { ...latest,
        ...(restore ? { text: current.text, revision: latest.revision + 1,
          ...(sentRecordings.length ? { recordings: [...sentRecordings,
            ...(latest.recordings ?? []).filter((recording) => !sentRecordings.some((sent) => sent.id === recording.id))] } : {}) } : {}),
        submission: status === null ? null : { ...submission, status },
      });
    };
    try { await deliver(); } catch (error) {
      const rejected = isInputRejected(error);
      settle(rejected ? 'rejected' : 'unknown');
      if (!rejected && error instanceof TypeError) throw new Error(m.chat_envio_incerto(), { cause: error });
      throw error;
    }
    // Entregue: falha local aqui não pode virar "não enviado", senão a pessoa reenvia e duplica.
    try { settle(null); } catch (cause) {
      throw new Error(m.nova_conversa_resultado_salvar_erro(), { cause });
    }
  } finally {
    submitting.delete(key);
    notify();
  }
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

export function setChatsForeground(active: boolean): void {
  foregroundAtual = active;
  for (const api of chats.values()) api.setForeground(active);
}

export function _resetChatsForTests(): void {
  for (const api of chats.values()) api.release();
  chats.clear();
  foregroundAtual = true;
}
