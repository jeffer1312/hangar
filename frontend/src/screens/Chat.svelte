<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import NavBar from '../components/NavBar.svelte';
  import MessageList from '../components/MessageList.svelte';
  import Composer from '../components/Composer.svelte';
  import SessionSwitcherSheet from '../components/SessionSwitcherSheet.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
  import UsageSheet from '../components/UsageSheet.svelte';
  import GitSheet from '../components/GitSheet.svelte';
  import PreviewSheet from '../components/PreviewSheet.svelte';
  import ActivitySheet from '../components/ActivitySheet.svelte';
  import TerminalMirror from '../components/TerminalMirror.svelte';
  import AskQuestionSheet from '../components/AskQuestionSheet.svelte';
  import RunSheet from '../components/RunSheet.svelte';
  import {
    getHistory,
    sendInput,
    selectOption,
    interrupt,
    openEventStream,
    getSessions,
    createSession,
    getWorkflows,
    answerQuestions,
    getRunners,
  } from '../lib/api';
  import { parseStatusLine } from '../lib/statusline';
  import { listServers, getActiveId } from '../lib/auth';
  import { createActivityFolder } from '../lib/activity';
  import type { ChatEvent, StateEvent, State, SessionInfo, AskQuestionPayload, AnswerItem } from '../lib/types';
  import { stateLabels, stateColors, countAwaiting, nextAwaiting } from '../lib/format';

  interface Props {
    sessionName: string;
    onBack: () => void;
    onNavigateToChat: (name: string) => void;
    desktop?: boolean;   // montado no DesktopShell -> header sem "voltar"/switcher + atalhos de teclado
  }
  let { sessionName, onBack, onNavigateToChat, desktop = false }: Props = $props();

  let events = $state<ChatEvent[]>([]);
  // Índice id->posição em `events`. O SSE re-emite o transcript INTEIRO a cada (re)conexão; sem isto
  // o dedup fazia findIndex O(n) por evento = O(n²) por reconexão -> em conversa longa (n grande), no
  // celular (reconecta a cada background/foreground), congelava a main thread. Map = lookup O(1).
  const idIndex = new Map<string, number>();
  function rebuildIndex() {
    idIndex.clear();
    for (let i = 0; i < events.length; i++) idIndex.set(events[i].id, i);
  }
  // Ids de assistant_msg que SUBSTITUIRAM um preview visivel: entram sem animacao (o texto ja
  // estava na tela via preview — re-animar era o "pisca" que fazia perder a posicao de leitura).
  const swapIds = new Set<string>();
  let stateEvent = $state<StateEvent | null>(null);
  let loading = $state(true);
  let error = $state('');
  let es: EventSource | null = null;
  let watchdog: ReturnType<typeof setTimeout> | undefined;     // liveness: reconecta se a conexao morrer calada
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let screenEl: HTMLElement | undefined = $state();
  let pending = $state<{ id: string; text: string; solid?: boolean }[]>([]);
  // Draft do composer (bindable): o interrupt devolve a msg pendente aqui pra editar e reenviar.
  // PERSISTIDO por sessao no localStorage: o iOS mata/recarrega o PWA em background e o texto
  // digitado evaporava (ir buscar algo noutro app = perder o rascunho); trocar de sessao remonta
  // o Chat e zerava tambem. Restaura no mount; enviar limpa o campo -> remove a chave junto.
  // Snapshot do mount de proposito: o App remonta o Chat por {#key sessionName} a cada troca.
  // svelte-ignore state_referenced_locally
  const draftKey = `cp-draft:${sessionName}`;
  let composerText = $state(localStorage.getItem(draftKey) ?? '');
  $effect(() => {
    if (composerText) localStorage.setItem(draftKey, composerText);
    else localStorage.removeItem(draftKey);
  });
  // Preview AO VIVO do bloco de assistente em voo (lido do pane via SSE 'preview'). Texto-completo,
  // full-replace; some quando o assistant_msg canonico (do .jsonl) cobre o texto, ou ao sair de working.
  let previewText = $state('');
  let dockEl: HTMLElement | undefined = $state();
  // Altura real do dock (composer) -> vira padding da lista pra ultima msg sempre limpar o glass.
  let dockH = $state(150);
  let navEl: HTMLElement | undefined = $state();
  // Altura real da navbar (overlay colado no topo) -> --nav-h: padding-top da lista pra 1a msg limpar a
  // navbar e o resto rolar POR BAIXO dela (= efeito glass). Mesmo modelo do dock.
  let navH = $state(56);


  // ── Switcher de sessoes (NavBar -> sheet) + criar nova sem voltar ──────────
  let switcherOpen = $state(false);
  let createOpen = $state(false);
  let usageOpen = $state(false);
  let gitOpen = $state(false);
  let runOpen = $state(false);
  let runRunning = $state(false);
  onMount(() => { getRunners(sessionName).then((r) => (runRunning = !!r.running)).catch(() => {}); });
  let previewOpen = $state(false);
  let activityOpen = $state(false);
  let askPayload = $state<AskQuestionPayload | null>(null);
  let askOpen = $state(false);
  // Viewport largo → pergunta vira card inline no chat (contexto visível); estreito → bottom-sheet.
  let isWide = $state(typeof window !== 'undefined' && window.matchMedia('(min-width: 768px)').matches);
  onMount(() => {
    const mq = window.matchMedia('(min-width: 768px)');
    const on = () => (isWide = mq.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });
  let allSessions = $state<SessionInfo[]>([]);

  async function openSwitcher() {
    switcherOpen = true;
    try {
      allSessions = await getSessions();
    } catch {
      // sem lista -> o sheet ainda oferece "Nova sessão"
    }
  }

  function pickSession(name: string) {
    switcherOpen = false;
    if (name !== sessionName) onNavigateToChat(name);
  }

  function startNew() {
    switcherOpen = false;
    createOpen = true;
  }

  async function handleCreate(name: string, cwd?: string) {
    await createSession(name, cwd);
    onNavigateToChat(name);
  }

  // ── Atalhos de teclado (so desktop) ────────────────────────────────────────
  let composerRef = $state<{ focus: () => void } | undefined>();

  // Lista pra navegar sessao com Ctrl/Cmd+setas (desktop) e pra pilula "N aguardando" (mobile,
  // feature #4). Carregada no mount nos dois; no mobile reconsulta a cada 5s pra o contador da
  // pilula refletir sessoes que entram/saem de awaiting_input enquanto o usuario fica parado aqui
  // (esta tela nao tem SSE agregado de sessoes — reusa o mesmo REST de sempre, so com poll).
  async function loadSessionsForNav() {
    try { allSessions = await getSessions(); } catch { /* sem lista -> setas/pilula viram no-op */ }
  }
  onMount(() => {
    loadSessionsForNav();
    if (desktop) return;
    const id = setInterval(loadSessionsForNav, 5000);
    return () => clearInterval(id);
  });

  function switchRelative(delta: number) {
    const names = allSessions.map((s) => s.name);
    if (names.length < 2) { loadSessionsForNav(); return; }
    const i = names.indexOf(sessionName);
    const next = names[((i < 0 ? 0 : i + delta) + names.length) % names.length];
    if (next && next !== sessionName) onNavigateToChat(next);
  }

  // Pilula de triage "N aguardando" (mobile only, feature #4): conta e pula direto pra proxima
  // sessao awaiting_input (wrap-around), reusando o MESMO onNavigateToChat do switchRelative acima
  // — so a escolha do alvo muda (filtrada+ordenada por nextAwaiting em vez de delta sequencial).
  // $derived de allSessions -> nunca cacheia a contagem; some sozinha quando ninguem mais aguarda.
  const awaitingCount = $derived(countAwaiting(allSessions));
  function goNextAwaiting() {
    const next = nextAwaiting(allSessions, sessionName);
    if (next && next !== sessionName) onNavigateToChat(next);
  }

  const anyOverlayOpen = () =>
    switcherOpen || createOpen || usageOpen || gitOpen || runOpen || previewOpen || activityOpen || mirrorOpen || askOpen;
  function closeOverlays() {
    switcherOpen = createOpen = usageOpen = gitOpen = runOpen = previewOpen = activityOpen = false;
    if (mirrorOpen) closeMirror();
    askOpen = false;
  }

  function onGlobalKey(e: KeyboardEvent) {
    if (!desktop) return;
    const mod = e.ctrlKey || e.metaKey;
    if (e.key === 'Escape' && anyOverlayOpen()) { e.preventDefault(); closeOverlays(); return; }
    if (mod && (e.key === 'k' || e.key === 'K')) { e.preventDefault(); openSwitcher(); return; }
    if (mod && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      e.preventDefault(); switchRelative(e.key === 'ArrowDown' ? 1 : -1); return;
    }
    const el = e.target as HTMLElement | null;
    const typing = !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
    if (typing) return;   // "/" foca o composer so quando NAO ja digitando num campo
    if (e.key === '/') { e.preventDefault(); composerRef?.focus(); }
  }

  const currentState = $derived<State>(stateEvent?.state ?? 'idle');
  // Espelho do pane (overlays so-TUI: /status, /config, /help, pickers). MANUAL: o usuario abre pelo
  // botao da NavBar ou pela pilula de aviso — NUNCA toma a tela sozinho (auto-takeover assustava +
  // prendia). tuiOverlay = ha um overlay aberto que SO da pra interagir pela TUI (sem opcoes nativas;
  // awaiting_input vira OptionButtons na lista). Serve so pra DESTACAR (pulsar o botao + mostrar a
  // pilula), nao pra abrir.
  // login: sessao parada no welcome/login do Claude Code (sem .jsonl -> chat vazio). Reusa o mesmo
  // affordance do overlay (pulsa o botao do terminal + pill -> abre o espelho), so com texto proprio.
  const needsLogin = $derived(!!stateEvent?.login && currentState !== 'awaiting_input');
  const tuiOverlay = $derived((!!stateEvent?.overlay || needsLogin) && currentState !== 'awaiting_input');
  let mirrorOpen = $state(false);
  function openMirror() { mirrorOpen = true; }
  // "Voltar ao chat" = SO esconde o espelho. NAO manda Escape -> a TUI fica como esta (nao fecha o
  // painel que o usuario queria ler). Sair do overlay de proposito = tecla Esc na barra do espelho.
  function closeMirror() { mirrorOpen = false; }
  // Statusline crua -> campos tipados (modelo, contexto, custo, tempo de sessao).
  const status = $derived(parseStatusLine(stateEvent?.status_line ?? null));

  // Header desktop: breadcrumb (servidor › sessao › branch) + pilula de estado. Dados ja vem do
  // status; server label vem do auth. So computa no desktop.
  const serverLabel = $derived(
    desktop ? (listServers().find((s) => s.id === getActiveId())?.label ?? '') : ''
  );
  const crumbs = $derived(
    desktop ? { server: serverLabel, session: sessionName, branch: status?.branch, dirty: status?.dirty ?? false } : null
  );
  // Anuncio de estado pra screen reader: a transicao que pede acao humana (awaiting_input) nao
  // tinha NENHUM sinal nao-visual. role="status" (aria-live polite) num no visualmente escondido.
  let stateAnnounce = $state('');
  let prevAnnounced: State | null = null;
  $effect(() => {
    const s = currentState;
    if (prevAnnounced !== null && s !== prevAnnounced) {
      if (s === 'awaiting_input') stateAnnounce = `${sessionName} aguardando sua resposta`;
      else if (s === 'dead') stateAnnounce = `Sessão ${sessionName} encerrada`;
      else stateAnnounce = '';
    }
    prevAnnounced = s;
  });
  // Painel de atividade: tarefas (TaskCreate/Update) + agentes rodando. Fold INCREMENTAL: o handler
  // do SSE dá push evento a evento — deriveActivity(events) como $derived re-varria o histórico
  // INTEIRO a cada mensagem (O(n) por evento em sessão longa).
  const actFolder = createActivityFolder();
  let activity = $state(actFolder.snapshot());
  const activityBadge = $derived(activity.inProgress + activity.runningAgents);
  const hasActivity = $derived(activity.tasks.length > 0 || activity.agents.length > 0);

  // Workflow roda em BACKGROUND -> nao da pra inferir "rodando" so pelos eventos (activity.ts marca
  // workflow running:false). Pergunta ao backend (le os arquivos do run) SÓ com motivo: sheet de
  // atividade aberto ou run ainda ativo; um workflow NOVO no transcript (wfCount muda) dispara UM
  // poll (kick) que, se estiver rodando, liga o loop de 4s até terminar. Antes: qualquer workflow
  // no histórico (mesmo finalizado há dias) pollava a cada 4s pra sempre.
  let workflowRunning = $state(false);
  const wfCount = $derived(activity.agents.filter((a) => a.kind === 'workflow').length);
  const activityRunning = $derived(workflowRunning || activity.runningAgents > 0);
  $effect(() => {
    if (!wfCount) { workflowRunning = false; return; }
    const sustain = activityOpen || workflowRunning;
    let alive = true;
    async function poll() {
      try {
        const ws = await getWorkflows(sessionName);
        if (alive) workflowRunning = ws.some((w) => w.running);
      } catch { /* offline / sem run -> ignora */ }
    }
    poll(); // kick: roda 1x a cada mudança de wfCount/activityOpen/workflowRunning
    const id = sustain ? setInterval(poll, 4000) : undefined;
    return () => { alive = false; if (id !== undefined) clearInterval(id); };
  });

  // Contadores/folds incrementais re-semeados junto com `events` (reseed completo).
  function reseedDerived() {
    actFolder.reset(events);
    activity = actFolder.snapshot();
    asstCount = countAssts(events);
  }

  // Classifica o erro de carga: 404 / "not found" = transcript trocado ou backend reiniciou (o caso
  // mais comum) -> copy propria + saida. O resto mostra a mensagem crua.
  const errorInfo = $derived({ notFound: /(^|\D)404(\D|$)/.test(error) || /not found/i.test(error) });

  async function loadHistory() {
    try {
      events = await getHistory(sessionName);
      rebuildIndex();
      reseedDerived();
    } catch (err) {
      error = err instanceof Error ? err.message : 'Erro ao carregar histórico';
    } finally {
      loading = false;
    }
  }

  // Watchdog de liveness: o backend manda um evento 'ping' a cada 10s. 25s sem NADA (msg/state/ping)
  // = conexao morta sem aviso (half-open: mobile trocou de rede / app no background / backend caiu).
  // O EventSource.onerror NAO dispara em half-open -> sem isto o front congela no ultimo estado.
  function armWatchdog() {
    clearTimeout(watchdog);
    watchdog = setTimeout(() => connectSSE(), 25000);
  }

  function connectSSE() {
    clearTimeout(reconnectTimer);
    if (es) { es.close(); es = null; }

    es = openEventStream(sessionName);
    armWatchdog();

    es.addEventListener('message', (e: MessageEvent) => {
      armWatchdog();
      try {
        const ev = JSON.parse(e.data) as ChatEvent;
        // Dedup cruzado fila<->transcript: a fila duravel emite user_msg sintetico (id "queued-").
        // Quando o Claude Code grava o prompt real, chega o user_msg real -> tira o sintetico de
        // mesmo texto (por linha, pq ele pode fundir varias). E nao adiciona sintetico se o real
        // ja existe. (covers: a "cobre" b se forem iguais ou b for uma linha de a.)
        if (ev.kind === 'user_msg' && ev.text) {
          const covers = (a: string, b: string) => {
            const at = a.trim(), bt = b.trim();
            if (at === bt || at.split('\n').some((ln) => ln.trim() === bt)) return true;
            // Msg com imagem: eco/fila carrega "📎 imagem: <path>", o transcript grava so a legenda ->
            // casa pela legenda canonica (senao a bolha com foto fica pendente eterna).
            const ac = _cap(a), bc = _cap(b);
            return !!bc && ac === bc;
          };
          if (ev.id.startsWith('queued-')) {
            // Dedup INTEGRAL (todos os events): o follow re-emite a fila INTEIRA a cada reconexao;
            // limitar a janela (tentado em 2026-07-02) deixava entradas antigas escaparem e
            // aparecerem soltas no fim do chat. O falso-positivo raro (um "ok" antigo engolindo a
            // bubble de um "ok" novo na fila) e o custo aceito — cosmetico e a entrega nao muda.
            if (events.some((x) => x.kind === 'user_msg' && !x.id.startsWith('queued-') && x.text && covers(x.text, ev.text!))) {
              return; // real ja cobre este texto -> ignora o sintetico
            }
          } else {
            // Remove SO a 1a queued- que casar (nao todas): com duas "ok" na fila e uma real
            // commitada, a 2a continua pendente e visivel.
            const qi = events.findIndex((x) => x.kind === 'user_msg' && x.id.startsWith('queued-') && x.text && covers(ev.text!, x.text));
            if (qi >= 0) {
              events = [...events.slice(0, qi), ...events.slice(qi + 1)];
              rebuildIndex();
            }
          }
        }
        // Dedupe by id: the SSE replays the whole transcript on every (re)connect and
        // loadHistory() also seeds events — without this, messages double up and the
        // keyed {#each} chokes on duplicate ids.
        const i = idIndex.get(ev.id);
        if (i !== undefined) {
          const next = events.slice();
          next[i] = ev;
          events = next;
        } else {
          idIndex.set(ev.id, events.length);
          events = [...events, ev];
          // Folds incrementais: evento NOVO alimenta o painel de atividade e o contador de
          // assistant_msg (replaces do replay não passam aqui -> não contam dobrado).
          if (ev.kind === 'tool_use' || ev.kind === 'tool_result') {
            actFolder.push(ev);
            activity = actFolder.snapshot();
          } else if (ev.kind === 'assistant_msg' && ev.text) {
            asstCount += 1;
            // Swap preview->bolha ATOMICO: o bloco real entra SEM animacao (swapIds) e o preview
            // zera AQUI, sincrono, no mesmo flush do append -> UM paint so, sem frame vazio nem
            // texto duplicado. A bolha nasce no mesmo y do preview: o que ja foi lido nao se move.
            // (Antes: append num flush + limpeza do preview num $effect pos-render = 2 repaints,
            // bolha re-animando e scroll pulando — o usuario perdia o ponto da leitura.)
            if (previewText) {
              swapIds.add(ev.id);
              previewText = '';
            }
          }
        }
      } catch {}
    });

    es.addEventListener('state', (e: MessageEvent) => {
      armWatchdog();
      try {
        stateEvent = JSON.parse(e.data) as StateEvent;
      } catch {}
    });

    // Heartbeat do backend: so prova de vida (reseta o watchdog numa conexao ociosa, sem msgs).
    es.addEventListener('ping', () => armWatchdog());

    // Stepper nativo AskUserQuestion: abre o sheet com as perguntas recebidas via SSE
    es.addEventListener('ask_question', (e: MessageEvent) => {
      try { askPayload = JSON.parse(e.data); askOpen = true; } catch {}
    });

    // Preview ao vivo (best-effort) do bloco de assistente em voo. Full-replace; tambem e prova de
    // vida (mas NAO a unica — entre turnos nao ha preview, por isso o ping ancora o watchdog).
    es.addEventListener('preview', (e: MessageEvent) => {
      armWatchdog();
      try {
        const t = (JSON.parse(e.data) as { text?: string }).text ?? '';
        // Guard de monotonicidade: frame TRANSITORIO do pane (mid-redraw) as vezes chega como
        // PREFIXO do texto ja mostrado -> ignorar, senao o texto recua e re-cresce (stuttering).
        // Vazio (drop) e conteudo realmente novo passam.
        if (t && t.length < previewText.length && previewText.startsWith(t)) return;
        previewText = t;
      } catch {}
    });

    // Reset de sessao (ex: /clear): o backend trocou de transcript. O dedup-por-id NAO limparia as
    // bolhas antigas (ids diferentes) -> zera tudo e recarrega o history do jsonl novo (vem limpo).
    es.addEventListener('reset', () => {
      armWatchdog();
      events = [];
      idIndex.clear();
      reseedDerived();          // zera activity/asstCount junto (loadHistory re-semeia com o novo)
      previewText = '';
      stateEvent = null;
      loadHistory();
    });

    es.onerror = () => {
      // Erro REAL (TCP RST): reconecta em 3s. Half-open nao cai aqui -> coberto pelo watchdog.
      if (currentState !== 'dead') {
        clearTimeout(reconnectTimer);
        reconnectTimer = setTimeout(connectSSE, 3000);
      }
    };
  }

  // App voltou pro foreground (mobile suspende a conexao no background). Agora o backfill do SSE so
  // traz o TAIL (ultimas _BACKFILL_LINES linhas), entao um background LONGO pode ter perdido mais que
  // isso. Re-seed do history (REST, completo e ordenado) ANTES de reconectar fecha o buraco; o backfill
  // tail do SSE so faz a ponte ate a subscricao (dedup por id, sem reordenar). Falha aqui NAO trava a
  // tela (o connectSSE/onerror re-sincroniza) -> ignora e segue. Reconexoes de blip (watchdog/onerror)
  // continuam SO com o tail-K: cobrem poucos segundos sem re-shippar o arquivo inteiro.
  async function onVisible() {
    if (document.visibilityState !== 'visible') return;
    try {
      const fresh = await getHistory(sessionName);
      events = fresh;
      rebuildIndex();
      reseedDerived();
    } catch { /* offline momentaneo: o connectSSE/onerror cuida do re-sync */ }
    connectSSE();
  }

  onMount(async () => {
    await loadHistory();
    connectSSE();
    document.addEventListener('visibilitychange', onVisible);
  });

  onDestroy(() => {
    es?.close();
    clearTimeout(watchdog);
    clearTimeout(reconnectTimer);
    document.removeEventListener('visibilitychange', onVisible);
  });

  // Layout teclado-safe: a .chat-screen acompanha a ALTURA da viewport visivel. Quando o
  // teclado abre, vv.height encolhe -> o container encolhe pra area acima do teclado, com a
  // NavBar colada no topo e o composer no rodape (ambos flex-shrink:0) e a MessageList (flex:1)
  // como UNICO scroller. offsetTop compensa o pan do iOS (senao o composer some pro topo).
  $effect(() => {
    const vv = window.visualViewport;
    if (!vv || !screenEl) return;
    function fit() {
      if (!screenEl || !vv) return;
      // Ignora valores transientes (a animacao do teclado reporta alturas minusculas por 1 frame).
      if (vv.height < 120) return;
      const h = vv.height + 'px';
      // offsetTop = quanto o iOS PANEIA a visual viewport ao abrir o teclado (body travado -> e pan
      // VISUAL). Compensamos via `top` em position:relative (sem transform: nao promove layer com
      // tiled-backing -> SEM retangulo preto; nao cria containing-block que prenda os sheets fixed).
      // EXPERIMENTO teclado iOS (#1): o pan (offsetTop) e bugado no iOS 26 (Apple #800125) e deixava o
      // composer com um vao acima do teclado. Mata o pan (scrollTo 0) e ancora top=0 -> a tela passa a
      // ser SO a altura visivel (vv.height), com o dock colado no rodape dela = topo do teclado.
      // Guard: so scrolla se houver scroll REAL. scrollTo a cada evento do viewport (toda tecla)
      // disparava o dialog "Desfazer" (shake-to-undo) do iOS toda hora.
      if (window.scrollY !== 0) window.scrollTo(0, 0);
      if (screenEl.style.height !== h) screenEl.style.height = h;
      if (screenEl.style.top !== '0px') screenEl.style.top = '0px';
      if (screenEl.style.transform) screenEl.style.transform = '';
      // Cola mais o composer no teclado: aberto -> zera o padding-bottom de safe-area (home indicator,
      // inutil com teclado) que deixava um vao; fechado -> volta a safe-area (fallback do --composer-pb).
      if (vv.height < window.innerHeight - 100) screenEl.style.setProperty('--composer-pb', 'var(--space-2)');
      else screenEl.style.removeProperty('--composer-pb');
    }
    function onFocusIn() {
      requestAnimationFrame(fit);
      setTimeout(fit, 300); // iOS as vezes so estabiliza apos a animacao do teclado
    }
    // iOS 26: offsetTop/height as vezes NAO zeram ao fechar o teclado. No blur sem outro campo focado,
    // forca estado limpo (senao sobra um vao no rodape).
    function onFocusOut() {
      setTimeout(() => {
        if (!screenEl) return;
        const a = document.activeElement;
        if (a && (a.tagName === 'TEXTAREA' || a.tagName === 'INPUT')) return;
        screenEl.style.top = '0px';
        screenEl.style.height = '';   // volta pro height do CSS (100vh)
        screenEl.style.transform = '';
      }, 50);
    }
    fit();
    vv.addEventListener('resize', fit);
    vv.addEventListener('scroll', fit);
    screenEl.addEventListener('focusin', onFocusIn);
    screenEl.addEventListener('focusout', onFocusOut);
    return () => {
      vv.removeEventListener('resize', fit);
      vv.removeEventListener('scroll', fit);
      screenEl?.removeEventListener('focusin', onFocusIn);
      screenEl?.removeEventListener('focusout', onFocusOut);
    };
  });

  // Mede a altura do dock (composer) e expoe via prop pra lista. ResizeObserver dispara SO quando
  // o composer muda de tamanho (anexo, multilinha, botao stop) — NAO na animacao do teclado
  // (composer mantem a altura) — entao nao reintroduz o glitch da NavBar.
  $effect(() => {
    if (!dockEl) return;
    let raf = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        if (!dockEl) return;
        const h = Math.round(dockEl.getBoundingClientRect().height);
        if (Math.abs(h - dockH) > 2) dockH = h;
      });
    });
    ro.observe(dockEl);
    return () => { cancelAnimationFrame(raf); ro.disconnect(); };
  });

  // Mede a navbar (overlay) -> --nav-h, pra lista clarear a 1a msg e rolar por baixo. Igual ao dock.
  $effect(() => {
    if (!navEl) return;
    let raf = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        if (!navEl) return;
        const h = Math.round(navEl.getBoundingClientRect().height);
        if (Math.abs(h - navH) > 2) navH = h;
      });
    });
    ro.observe(navEl);
    return () => { cancelAnimationFrame(raf); ro.disconnect(); };
  });

  // Legenda canonica de uma msg (sem o marcador "📎 imagem:/arquivo: <path>" + o "—" que liga). Pro
  // dedup de pending/fila: o eco local carrega o marcador, mas o transcript grava SO a legenda -> sem
  // normalizar, msg COM ANEXO nunca casava e ficava pendente pra sempre.
  function _cap(text: string): string {
    const i = text.search(/(?:\s*—\s*)?📎\s*(?:imagem|arquivo):/u);
    return (i >= 0 ? text.slice(0, i) : text).trim();
  }

  let pendingSeq = 0;

  async function handleSend(text: string) {
    // Enviou enquanto o Claude trabalha -> entra na fila (Claude Code enfileira no tmux).
    // Eco imediato como bubble pendente; solidifica quando o transcript trouxer a msg real.
    let pendingId: string | null = null;
    if (currentState === 'working') {
      pendingId = `pending-${pendingSeq++}`;
      pending = [...pending, { id: pendingId, text }];
    }
    try {
      await sendInput(sessionName, text);
    } catch (err) {
      console.error('sendInput error:', err);
      // Falhou o envio -> remove o pending que adicionamos (nao ficou enfileirado).
      if (pendingId) pending = pending.filter((p) => p.id !== pendingId);
      throw err; // propaga pro Composer mostrar o erro e NAO limpar o input
    }
  }

  // Dedup: solta o pending quando o transcript (SSE) traz a msg real. Casa por texto NORMALIZADO
  // e tambem por LINHA — o Claude Code funde varias msgs enfileiradas numa so (separadas por \n),
  // entao "msg1\nmsg2" casa tanto "msg1" quanto "msg2". Idempotente (length estabiliza).
  $effect(() => {
    if (pending.length === 0) return;
    const committed = new Set<string>();
    for (const e of events) {
      if (e.kind !== 'user_msg' || !e.text) continue;
      const t = e.text.trim();
      committed.add(t);
      committed.add(_cap(t));                       // legenda canonica (msg com imagem grava so ela)
      for (const line of t.split('\n')) committed.add(line.trim());
    }
    // Remove o eco quando o texto cru OU a legenda (sem "📎 imagem: <path>") ja commitou. Legenda
    // vazia (imagem sem texto) nao casa por texto -> cai no solidify do idle, nao trava aqui.
    const next = pending.filter((p) => {
      const cap = _cap(p.text);
      return !committed.has(p.text.trim()) && !(cap && committed.has(cap));
    });
    if (next.length !== pending.length) pending = next;
  });

  // Solidificar a fila: msgs enviadas enquanto o Claude trabalha NEM sempre viram entrada no
  // transcript dele (so as enviadas com ele ocioso viram um prompt gravado). Entao nao da pra
  // apagar o eco (sumiria — era o bug). Quando ele volta a idle (consumiu a fila), SOLIDIFICA o
  // pending no lugar: vira bubble normal (sem opacidade), parte do fluxo. O reconcile acima ainda
  // remove os que casarem com o transcript (evita duplicar quando o Claude Code de fato grava).
  let prevState: State = 'idle';
  $effect(() => {
    const s = currentState;
    if (prevState !== 'idle' && s === 'idle' && pending.some((p) => !p.solid)) {
      pending = pending.map((p) => ({ ...p, solid: true }));
    }
    prevState = s;
  });

  // Dropa o preview quando: (a) um NOVO bloco de assistente COMMITA (vira bubble real), ou (b) sai de
  // working (turn acabou / só-tool / interrompido). (a) é o sinal timing-safe: o FRONT sabe o que já
  // mostrou — não depende de QUANDO o texto cai no .jsonl. Quando o bloco vira bubble, o preview dele
  // não é mais necessário e some; reaparece sozinho quando o PRÓXIMO bloco começa a streamar (o broker
  // reemite). Mata a duplicata (preview + bubble do mesmo bloco) na raiz, sem comparar texto.
  // Tira crase E marcadores de markdown (* _ ~ # >): o preview vem do pane JÁ RENDERIZADO (sem
  // markdown), o .jsonl tem o markdown cru -> sem tirar, "**Confirma**" != "Confirma" e o preview
  // duplicado de uma msg com formatação NÃO casava com a commitada (ficava como bolha fantasma).
  const _norm = (s: string) => s.replace(/[`*_~#>]/g, '').replace(/\s+/g, ' ').trim();
  // Contador INCREMENTAL de assistant_msg (mantido no handler do SSE + reseedDerived): o effect
  // abaixo rodava um loop no `events` inteiro A CADA frame de preview (~150ms em streaming) só pra
  // detectar um commit novo — O(n) por frame em sessão longa.
  let asstCount = $state(0);
  function countAssts(evs: ChatEvent[]): number {
    let c = 0;
    for (const e of evs) if (e.kind === 'assistant_msg' && e.text) c++;
    return c;
  }
  let _asstSeen = 0;
  $effect(() => {
    // CRÍTICO: ler previewText AQUI no topo, SEMPRE -> em Svelte 5 a dep só é rastreada se LIDA na
    // execução. Se a gente retornasse antes de ler (caminho idle), o effect não re-rodaria quando o
    // broker REEMITISSE o preview no idle -> o tail ficava (a duplicata que não saía). Lendo aqui, o
    // effect re-roda a cada update do preview e limpa.
    const pv = previewText;
    const committed = asstCount > _asstSeen;
    _asstSeen = asstCount;
    if (!pv) return;
    // (a) bloco novo commitou OU (b) saiu de working -> dropa.
    if (committed || currentState !== 'working') { previewText = ''; return; }
    // (c) residual coberto por QUALQUER das últimas msgs commitadas (não só a última): entre turnos o
    // pane ainda mostra o bloco anterior como "● tail" e o broker reemite -> dropa se já é bolha.
    const p = _norm(pv);
    if (p.length >= 16) {
      let seen = 0;
      for (let i = events.length - 1; i >= 0 && seen < 6; i--) {
        const e = events[i];
        if (e.kind === 'assistant_msg' && e.text) {
          seen++;
          if (_norm(e.text).includes(p)) { previewText = ''; return; }
        }
      }
    }
  });

  // Slash commands gerais do Claude Code (ex: /clear, /compact) -> sessao viva. Modelo e
  // esforco NAO passam por aqui: vao pelo ModelEffortSheet -> endpoint /model-effort.
  async function handleCommand(cmd: string) {
    try {
      await sendInput(sessionName, cmd);
    } catch (err) {
      console.error('sendInput (command) error:', err);
    }
  }

  async function handleSelect(option: number) {
    try {
      await selectOption(sessionName, option);
    } catch (err) {
      console.error('selectOption error:', err);
    }
  }

  async function handleInterrupt() {
    // Ao interromper, o Claude Code MANTEM a msg enfileirada no input -> proximo envio concatenava.
    // Se ha pendente, devolve o texto pro composer (editavel) e remove a bubble; pede clear ao backend
    // (2o Esc) pra limpar o input do terminal. Sem pendente: interrupt simples (sem clear -> sem rewind).
    const last = pending.length ? pending[pending.length - 1] : null;
    if (last) {
      composerText = last.text;
      pending = pending.filter((p) => p.id !== last.id);
    }
    try {
      await interrupt(sessionName, !!last);
    } catch (err) {
      console.error('interrupt error:', err);
    }
  }

  // 409 (mismatch de verificação) ou erro inesperado -> cai no espelho TUI p/ finalizar manual
  async function handleAnswer(answers: AnswerItem[]) {
    try {
      await answerQuestions(sessionName, answers);
      askOpen = false;
    } catch {
      askOpen = false;
      openMirror();
    }
  }
</script>

<svelte:window onkeydown={onGlobalKey} />

<div class="chat-screen" bind:this={screenEl} style:--nav-h={navH + 'px'}>
  <div class="sr-only" role="status">{stateAnnounce}</div>
  <div class="navbar-mount" bind:this={navEl}>
    <NavBar title={sessionName} showBack={!desktop} onBack={onBack} onTitleTap={desktop ? undefined : openSwitcher} {crumbs} stateLabel={desktop ? stateLabels[currentState] : undefined} stateColor={stateColors[currentState]} {status} onExpandUsage={() => (usageOpen = true)} onOpenActivity={hasActivity ? () => (activityOpen = true) : undefined} {activityBadge} {activityRunning} onOpenTerminal={openMirror} terminalAlert={tuiOverlay && !mirrorOpen} onOpenRun={() => (runOpen = true)} {runRunning} working={currentState === 'working'} />
  </div>

  {#if loading}
    <!-- Entrando na sessao: skeleton shimmer (familia Respiracao) enquanto o /history carrega. -->
    <div class="chat-skeleton" aria-label="Carregando histórico" aria-busy="true">
      <div class="sk-line sk-r" style="width:46%"></div>
      <div class="sk-line" style="width:82%"></div>
      <div class="sk-line" style="width:64%"></div>
      <div class="sk-line sk-r" style="width:38%"></div>
      <div class="sk-line" style="width:90%"></div>
      <div class="sk-line" style="width:55%"></div>
    </div>
  {:else if error}
    <div class="chat-error">
      {#if errorInfo.notFound}
        <p class="chat-error-title">Não encontrei o transcript desta sessão.</p>
        <p class="chat-error-hint">O transcript pode ter sido trocado (por exemplo com <code>/clear</code>) ou o backend reiniciou. Tentar de novo costuma resolver.</p>
      {:else}
        <p class="chat-error-title">Não deu pra carregar o histórico.</p>
        <p class="chat-error-hint">{error}</p>
      {/if}
      <div class="chat-error-actions">
        <button class="retry-btn" onclick={loadHistory}>Tentar novamente</button>
        <button class="back-btn-inline" onclick={onBack}>Voltar às sessões</button>
      </div>
    </div>
  {:else}
    <MessageList
      {events}
      {stateEvent}
      {pending}
      {sessionName}
      {dockH}
      {swapIds}
      preview={previewText}
      onSelectOption={handleSelect}
      onCancel={handleInterrupt}
      askOpen={isWide && askOpen}
      askPayload={askPayload}
      askActive={askOpen && askPayload != null}
      onAnswer={handleAnswer}
      onAskClose={() => (askOpen = false)}
    />
  {/if}

  {#if tuiOverlay && !mirrorOpen}
    <!-- Aviso DESTACADO: ha um painel que SO da pra interagir pela TUI. Pulsa pra chamar atencao;
         tocar abre o espelho. Nao toma a tela (so um banner acima do dock). -->
    <button class="tui-pill" style:bottom={`${dockH + 10}px`} onclick={openMirror} aria-label={needsLogin ? 'Abrir terminal para fazer login' : 'Abrir terminal para interagir'}>
      <span class="tui-pill-dot"></span>
      <span class="tui-pill-text">{needsLogin ? 'Sessão precisa de login — toque pra entrar' : 'Interação só pela TUI — toque pra abrir'}</span>
    </button>
  {/if}

  {#if !desktop && awaitingCount > 0}
    <!-- Triage mobile (feature #4): pula pra proxima sessao aguardando resposta (wrap-around).
         Canto inferior direito (alcance do polegar) pra nao brigar com o tui-pill (centralizado)
         nem cobrir o composer/navbar. Some sozinha quando o contador zera (derived, sem cache). -->
    <button class="awaiting-pill" style:bottom={`${dockH + 10}px`} onclick={goNextAwaiting} aria-label={`${awaitingCount} sessão${awaitingCount > 1 ? 'ões' : ''} aguardando — ir para a próxima`}>
      {awaitingCount} aguardando →
    </button>
  {/if}

  <div class="bottom-dock" bind:this={dockEl}>
    {#if currentState === 'dead'}
      <div class="dead-footer">
        <p class="dead-text">Esta sessão foi encerrada.</p>
        <button class="back-btn" onclick={onBack}>← Voltar</button>
      </div>
    {:else}
      <!-- Composer SEMPRE visivel (exceto sessao morta). Antes ele sumia em awaiting_input e,
           se as opcoes nao fossem parseadas, o usuario ficava sem input E sem botoes = preso.
           Os OptionButtons continuam aparecendo na lista; o composer fica como saida garantida. -->
      <Composer
        bind:this={composerRef}
        {sessionName}
        bind:inputText={composerText}
        sessionState={currentState}
        status={status}
        onSend={handleSend}
        onCommand={handleCommand}
        onInterrupt={handleInterrupt}
        onExpandUsage={() => (usageOpen = true)}
        onOpenGit={() => (gitOpen = true)}
        onOpenPreview={() => (previewOpen = true)}
      />
    {/if}
  </div>

  <SessionSwitcherSheet
    open={switcherOpen}
    sessions={allSessions}
    currentName={sessionName}
    onPick={pickSession}
    onNew={startNew}
    onClose={() => (switcherOpen = false)}
  />

  <CreateSessionSheet
    open={createOpen}
    servers={listServers()}
    onClose={() => (createOpen = false)}
    onCreate={handleCreate}
    onOpenSession={onNavigateToChat}
  />

  <UsageSheet open={usageOpen} {status} onClose={() => (usageOpen = false)} />

  <GitSheet open={gitOpen} {sessionName} onClose={() => (gitOpen = false)} />

  <RunSheet open={runOpen} {sessionName} onClose={() => (runOpen = false)} onRunningChange={(r) => (runRunning = r)} />

  <PreviewSheet open={previewOpen} onClose={() => (previewOpen = false)} />

  <ActivitySheet open={activityOpen} {activity} {sessionName} onClose={() => (activityOpen = false)} />

  <TerminalMirror open={mirrorOpen} {sessionName} onClose={closeMirror} />

  {#if !isWide}
    <AskQuestionSheet
      open={askOpen}
      payload={askPayload}
      onSubmit={handleAnswer}
      onClose={() => (askOpen = false)}
      onFallback={openMirror}
    />
  {/if}
</div>

<style>
  .chat-screen {
    display: flex;
    flex-direction: column;
    height: 100vh;          /* fallback; o JS (fit) sobrescreve com visualViewport.height no teclado */
    overflow: hidden;
    position: relative;
    top: 0;
    background: var(--bg-base);  /* backing solido: a layer nunca renderiza preto no glitch do iOS */
    /* Higiene de stacking; reforço de baixo risco. isolation NAO cria containing block pros sheets
       position:fixed (filhos da .chat-screen) — ao contrário de transform/contain/will-change/filter,
       que clipariam os sheets. NÃO reintroduzir transform aqui (top relativo = sem layer, sem preto). */
    isolation: isolate;
  }

  /* Navbar overlay colado no topo (nao descola): a lista rola POR BAIXO via --nav-h. pointer-events
     deixa o fade transparente passar o toque pro conteudo; a navbar reativa. */
  .navbar-mount {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    z-index: 20;
    pointer-events: none;
  }
  .navbar-mount > :global(.navbar) {
    pointer-events: auto;
  }

  .chat-error {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-4);
    padding-top: var(--nav-h, 56px);
  }

  /* Skeleton de boot (no lugar do splash): linhas shimmer ocupando a area do chat. */
  .chat-skeleton {
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: var(--space-3);
    padding: var(--space-6) var(--space-5);
    max-width: 600px;
    width: 100%;
    margin: 0 auto;
    overflow: hidden;
  }
  .sk-line {
    height: 16px;
    border-radius: 8px;
    align-self: flex-start;
    background: linear-gradient(90deg, var(--bg-elevated) 0%, var(--bg-hover) 40%, var(--accent-dim) 50%, var(--bg-hover) 60%, var(--bg-elevated) 100%);
    background-size: 220% 100%;
    animation: sk-shim 1.6s linear infinite;
  }
  .sk-line.sk-r { align-self: flex-end; }   /* algumas linhas "do usuario" a direita */
  @keyframes sk-shim {
    0%   { background-position: 140% 0; }
    100% { background-position: -140% 0; }
  }

  .chat-error {
    max-width: 380px;
    margin: 0 auto;
    padding-left: var(--space-5);
    padding-right: var(--space-5);
  }
  .chat-error-title {
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    text-align: center;
  }
  .chat-error-hint {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    text-align: center;
    line-height: 1.5;
  }
  .chat-error-hint code {
    font-family: var(--font-mono);
    font-size: 0.85em;
    background: var(--bg-elevated);
    padding: 1px 5px;
    border-radius: var(--radius-sm);
  }
  .chat-error-actions {
    display: flex;
    gap: var(--space-3);
    flex-wrap: wrap;
    justify-content: center;
  }

  .retry-btn {
    height: 44px;
    padding: 0 var(--space-5);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    background: var(--accent);
    color: #fff;
    font-size: var(--text-sm);
    font-weight: 500;
  }
  .back-btn-inline {
    height: 44px;
    padding: 0 var(--space-5);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-secondary);
    font-size: var(--text-sm);
  }

  /* Bottom dock: statusline bar + composer (or dead footer). Flex child normal. */
  .bottom-dock {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    z-index: 20;
  }

  /* Aviso flutuante "interação só pela TUI": acima do dock (bottom = altura do dock + gap, via JS).
     Pulsa pra chamar atenção; centralizado. z acima do dock. */
  .tui-pill {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    z-index: 21;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    max-width: calc(100% - var(--space-6));
    padding: var(--space-2) var(--space-4);
    border: 1px solid var(--accent);
    border-radius: var(--radius-full, 999px);
    background: var(--bg-elevated, var(--bg-base));
    color: var(--text-primary);
    font-size: var(--text-sm);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
    animation: tui-pulse 1.6s ease-in-out infinite;
    -webkit-tap-highlight-color: transparent;
  }
  .tui-pill:active { background: var(--bg-hover); }
  .tui-pill-dot {
    width: 8px; height: 8px; flex-shrink: 0;
    border-radius: 50%;
    background: var(--accent);
  }
  .tui-pill-text { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  @keyframes tui-pulse {
    0%, 100% { box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35), 0 0 0 0 var(--accent); }
    50%      { box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35), 0 0 0 4px transparent; }
  }
  @media (prefers-reduced-motion: reduce) {
    .tui-pill { animation: none; }
  }

  /* Pilula de triage "N aguardando" (mobile, feature #4): FAB no canto inferior direito, acima do
     dock, alcance de polegar. Sem pulso (nao e alerta de bloqueio como o tui-pill, e uma acao
     disponivel) — cor de destaque so pra chamar atencao sem ansiedade visual. */
  .awaiting-pill {
    position: absolute;
    right: var(--space-4);
    z-index: 21;
    padding: var(--space-2) var(--space-4);
    border: none;
    border-radius: var(--radius-full, 999px);
    background: var(--accent);
    color: #fff;
    font-size: var(--text-sm);
    font-weight: 600;
    white-space: nowrap;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
    -webkit-tap-highlight-color: transparent;
  }
  .awaiting-pill:active { opacity: 0.85; }

  /* Dead state footer */
  .dead-footer {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-5) var(--space-6);
    background: var(--bg-base);
  }

  .dead-text {
    font-size: var(--text-sm);
    color: var(--text-muted);
    text-align: center;
  }

  .back-btn {
    height: 44px;
    padding: 0 var(--space-6);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    transition: background 180ms ease-out;
  }

  .back-btn:active {
    background: var(--bg-hover);
  }
</style>
