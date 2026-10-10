import { create } from 'zustand';
import type { StoreApi, UseBoundStore } from 'zustand';
import {
  getHistory,
  getHistoryDesde,
  openEventStreamForServer,
  prependOlder,
  hasSeam,
  isAbortError,
  isTimeoutError,
  especificidade,
  donoDaLinha,
  sendInputForServer,
  queuedMessages,
  registrarDiag,
  parsePluginToast,
} from '@hangar/core';
import type { ChatEvent, StateEvent, PreviewEvent, AskQuestionPayload, StatsEvent, EventSourceLike, Server } from '@hangar/core';
import * as m from '../paraglide/messages';
import { reconcilePending, type PendingMsg } from '../chat/pending';
import { useServers } from './servers';
import { readDraft, writeDraft, type ConversationDraft } from './drafts';

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
  draftUpdate: number;
  events: ChatEvent[];
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
}

// Quantas bolhas estão "na fila" (translúcidas): ecos locais + sintéticos queued-* da fila durável.
// Só conta onde a fila pode ser mandada agora (Kimi, Codex, Claude), como no PWA.
export function filaCount(state: Pick<ChatState, 'events' | 'pending'>, provider?: string | null, headless = false): number {
  if (provider !== 'kimi' && provider !== 'codex' && provider !== 'claude' && !headless) return 0;
  return state.pending.length + queuedMessages(state.events, provider, headless).length;
}

export interface ChatApi {
  use: UseBoundStore<StoreApi<ChatState>>;
  retain: () => void;
  release: () => void;
  loadOlder: () => void;
  send: (text: string, draftRevision?: number) => Promise<void>;
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
}

// Estado do app visto pelos stores que ainda vão nascer: o layout liga antes da primeira tela.
let foregroundAtual = true;
const submitting = new Set<string>();
export const isSubmitting = (serverId: string, name: string) => submitting.has(`${serverId}::${name}`);

function criarChatStore(serverId: string, name: string): ChatApi {
  const useChatStore = create<ChatState>(() => ({
    draftUpdate: 0,
    events: [],
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
    loading: true,
    error: '',
    olderFailed: '',
    sseRecusado: false,
    pending: [],
  }));

  // internos — fora do set() para não virar proxy
  let es: EventSourceLike | null = null;
  let lastEventId: string | null = null;
  let etag: string | null = null;
  let foreground = foregroundAtual;
  // Com base carregada a volta pede só a cauda; sem ela (primeira carga falhou, reset) recarrega tudo.
  let baseLoaded = false;
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
  // O fim do turno chega antes do bloco real pelo tail do .jsonl: apagar a prévia na hora abria um
  // buraco e a bolha voltava. Sair de working só agenda; o timer vence se o bloco nunca vier.
  let previewDropTimer: ReturnType<typeof setTimeout> | undefined;
  let pensamentoTimer: ReturnType<typeof setTimeout> | undefined;
  let ferramentaTimer: ReturnType<typeof setTimeout> | undefined;
  const pluginToastsSeen = new Set<string>();

  function limparPreview(): void {
    clearTimeout(previewDropTimer);
    previewDropTimer = undefined;
    previewMd = false;
    previewFull = false;
    useChatStore.setState({ preview: '', previewMd: false, previewFull: false });
  }
  function dropPreviewSoon(): void {
    if (previewDropTimer !== undefined || !useChatStore.getState().preview) return;
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

  function rebuildIndex(events: ChatEvent[]) {
    idIndex.clear();
    for (let i = 0; i < events.length; i++) idIndex.set(events[i].id, i);
  }

  function aplicarEvents(events: ChatEvent[]) {
    rebuildIndex(events);
    const pending = events.reduce(reconcilePending, useChatStore.getState().pending);
    useChatStore.setState({ events, pending });
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
    try {
      const target = destino();
      if (!target) throw new Error(m.chat_servidor_removido());
      const tail = await getHistory(name, TAIL_FIRST, signal, undefined, target);
      if (g !== histGen) return;
      aplicarEvents(tail);
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
        aplicarEvents(r.eventos);
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
    if (ev.queued_confirmed && ev.id.startsWith('queued-')) {
      events = events.filter((x) => x.id !== ev.id);
      rebuildIndex(events);
      useChatStore.setState({ events });
      return;
    }
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
    if (ev.kind === 'thinking' && useChatStore.getState().pensamento) limparPensamento();
    if (ev.kind === 'tool_use' && useChatStore.getState().ferramenta) limparFerramenta();
    // Swap prévia->bolha: o bloco real chegou, a prévia sai no MESMO flush.
    if (ev.kind === 'assistant_msg' && ev.text && useChatStore.getState().preview) limparPreview();
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
        if (!t) {
          if (useChatStore.getState().stateEvent?.state !== 'working') dropPreviewSoon();
          return;
        }
        cancelPreviewDrop();
        previewMd = !!ev.md;
        previewFull = !!ev.full;
        useChatStore.setState({ preview: t, previewMd: !!ev.md, previewFull: !!ev.full });
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
        if (payload.provider === 'codex' && current?.provider === 'codex'
            && payload.request_id === current.request_id) return;
        useChatStore.setState({ askPayload: payload, askOpen: true, askPiId: null });
      } catch {
        quadroFalhou('ask_question');
        // payload ilegível: o OptionButtons cru segue como saída
      }
    });

    es.addEventListener('reset', () => {
      // Transcript trocado (/clear): ids do arquivo antigo não valem mais — zera e recarrega.
      lastEventId = null;
      etag = null;
      baseLoaded = false;
      previewMd = false;
      previewFull = false;
      cancelPreviewDrop();
      clearTimeout(pensamentoTimer);
      pensamentoTimer = undefined;
      clearTimeout(ferramentaTimer);
      ferramentaTimer = undefined;
      useChatStore.setState({
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
        loading: true,
      });
      void loadHistory();
    });

    // reset do backoff quando a conexão estabiliza (paridade com Chat.svelte:1017 noteAlive)
    es.onopen = () => {
      if (es !== source) return;
      retryDelay = SSE_RETRY_MIN_MS;
    };
    // Fechar a fonte deixa apenas este backoff dono da repetição, sem polling em paralelo.
    es.onerror = (error) => {
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
    };
  }

  return {
    use: useChatStore,
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
    // O user_msg real só chega no fim do turno: sem tirar as "queued-" aqui, o chip seguia aceso
    // sobre uma fila que já foi.
    applySteer(result) {
      const sent = new Set(result.queued_ids ?? []);
      let events = useChatStore.getState().events;
      if (sent.size) events = events.map((e) => (sent.has(e.id) ? { ...e, queued_delivered: true } : e));
      if (result.promoted) events = events.filter((e) => !(e.kind === 'user_msg' && e.id.startsWith('queued-')));
      rebuildIndex(events);
      useChatStore.setState({ events });
    },
    async send(text: string, draftRevision?: number) {
      const trimmed = text.trim();
      if (!trimmed) return;
      // Destino desta conversa, nunca o servidor ativo: trocar de servidor no meio não desvia o envio.
      const target = useServers.getState().servers.find((s) => s.id === serverId);
      if (!target) throw new Error(m.chat_servidor_removido());
      const wasUnknown = readDraft(serverId, name)?.submission?.status === 'unknown';
      return submitConversationDraft(serverId, name, trimmed, async () => {
        if (wasUnknown) useChatStore.setState((s) => ({ pending: s.pending.filter((p) => p.text !== trimmed) }));
        const id = `pending-${pendingSeq++}`;
        useChatStore.setState((s) => ({ pending: [...s.pending, { id, text: trimmed }] }));
        try {
          await sendInputForServer(target, name, trimmed);
        } catch (err) {
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

// O snapshot precede qualquer POST; só o ACK desta revisão pode limpar o campo.
export async function submitConversationDraft(serverId: string, name: string, text: string, deliver: () => Promise<void>, draftRevision?: number): Promise<void> {
  const key = `${serverId}::${name}`;
  if (submitting.has(key)) throw new Error(m.chat_envio_incerto());
  submitting.add(key);
  const notify = () => chatStore(serverId, name).use.setState((s) => ({ draftUpdate: s.draftUpdate + 1 }));
  try {
    const current: ConversationDraft = readDraft(serverId, name) ?? {
      version: 1, text, revision: 1, transcript: null, attachment: null, submission: null,
    };
    if (current.submission && current.submission.text.trim() !== text.trim()) throw new Error(m.composer_submission_recover_first());
    const submission = { text, draftRevision: draftRevision ?? current.revision, status: 'sending' as const };
    // Primeiro input pode já ter outra edição no campo: aquela revisão não é a enviada.
    writeDraft(serverId, name, { ...current, submission,
      revision: draftRevision !== undefined || current.text.trim() === text.trim() ? current.revision : current.revision + 1 });
    notify();
    const settle = (status: 'unknown' | 'rejected' | null) => {
      const latest = readDraft(serverId, name);
      if (!latest || (current.transcript !== null && latest.transcript !== current.transcript)
        || latest.submission?.draftRevision !== submission.draftRevision || latest.submission.text !== text) return;
      writeDraft(serverId, name, { ...latest,
        text: status === null && latest.revision === submission.draftRevision ? '' : latest.text,
        revision: status === null && latest.revision === submission.draftRevision ? latest.revision + 1 : latest.revision,
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
