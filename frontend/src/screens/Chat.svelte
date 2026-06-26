<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import NavBar from '../components/NavBar.svelte';
  import MessageList from '../components/MessageList.svelte';
  import Composer from '../components/Composer.svelte';
  import SessionSwitcherSheet from '../components/SessionSwitcherSheet.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
  import UsageSheet from '../components/UsageSheet.svelte';
  import ActivitySheet from '../components/ActivitySheet.svelte';
  import {
    getHistory,
    sendInput,
    selectOption,
    interrupt,
    openEventStream,
    getSessions,
    createSession,
    getWorkflows,
  } from '../lib/api';
  import { parseStatusLine } from '../lib/statusline';
  import { listServers } from '../lib/auth';
  import { deriveActivity } from '../lib/activity';
  import type { ChatEvent, StateEvent, State, SessionInfo } from '../lib/types';

  interface Props {
    sessionName: string;
    onBack: () => void;
    onNavigateToChat: (name: string) => void;
  }
  let { sessionName, onBack, onNavigateToChat }: Props = $props();

  let events = $state<ChatEvent[]>([]);
  let stateEvent = $state<StateEvent | null>(null);
  let loading = $state(true);
  let error = $state('');
  let es: EventSource | null = null;
  let watchdog: ReturnType<typeof setTimeout> | undefined;     // liveness: reconecta se a conexao morrer calada
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let screenEl: HTMLElement | undefined = $state();
  let pending = $state<{ id: string; text: string; solid?: boolean }[]>([]);
  // Preview AO VIVO do bloco de assistente em voo (lido do pane via SSE 'preview'). Texto-completo,
  // full-replace; some quando o assistant_msg canonico (do .jsonl) cobre o texto, ou ao sair de working.
  let previewText = $state('');
  let dockEl: HTMLElement | undefined = $state();
  // Altura real do dock (composer) -> vira padding da lista pra ultima msg sempre limpar o glass.
  let dockH = $state(150);

  // Scroll ativo? -> o Composer desliga o backdrop-filter do glass durante o scroll (o backdrop
  // re-amostrando o conteudo em movimento dispara o bloco preto no iOS). Volta ao parar (timer).
  let scrolling = $state(false);
  let scrollTimer: ReturnType<typeof setTimeout> | undefined;
  function handleScrollActivity() {
    scrolling = true;
    clearTimeout(scrollTimer);
    scrollTimer = setTimeout(() => (scrolling = false), 160);
  }

  // ── Switcher de sessoes (NavBar -> sheet) + criar nova sem voltar ──────────
  let switcherOpen = $state(false);
  let createOpen = $state(false);
  let usageOpen = $state(false);
  let activityOpen = $state(false);
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

  const currentState = $derived<State>(stateEvent?.state ?? 'idle');
  // Statusline crua -> campos tipados (modelo, contexto, custo, tempo de sessao).
  const status = $derived(parseStatusLine(stateEvent?.status_line ?? null));
  // Painel de atividade: tarefas (TaskCreate/Update) + agentes rodando, derivado dos eventos.
  const activity = $derived(deriveActivity(events));
  const activityBadge = $derived(activity.inProgress + activity.runningAgents);
  const hasActivity = $derived(activity.tasks.length > 0 || activity.agents.length > 0);

  // Workflow roda em BACKGROUND -> nao da pra inferir "rodando" so pelos eventos (activity.ts marca
  // workflow running:false). Pergunta ao backend (le os arquivos do run) enquanto houver workflow no
  // painel; alimenta o "respira" do botao de atividade no topo (sinal de que nao travou).
  let workflowRunning = $state(false);
  const hasWorkflow = $derived(activity.agents.some((a) => a.kind === 'workflow'));
  const activityRunning = $derived(workflowRunning || activity.runningAgents > 0);
  $effect(() => {
    if (!hasWorkflow) { workflowRunning = false; return; }
    let alive = true;
    async function poll() {
      try {
        const ws = await getWorkflows(sessionName);
        if (alive) workflowRunning = ws.some((w) => w.running);
      } catch { /* offline / sem run -> ignora */ }
    }
    poll();
    const id = setInterval(poll, 4000);
    return () => { alive = false; clearInterval(id); };
  });

  async function loadHistory() {
    try {
      events = await getHistory(sessionName);
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
            return at === bt || at.split('\n').some((ln) => ln.trim() === bt);
          };
          if (ev.id.startsWith('queued-')) {
            if (events.some((x) => x.kind === 'user_msg' && !x.id.startsWith('queued-') && x.text && covers(x.text, ev.text!))) {
              return; // real ja cobre este texto -> ignora o sintetico
            }
          } else {
            const filtered = events.filter((x) => !(x.kind === 'user_msg' && x.id.startsWith('queued-') && x.text && covers(ev.text!, x.text)));
            if (filtered.length !== events.length) events = filtered;
          }
        }
        // Dedupe by id: the SSE replays the whole transcript on every (re)connect and
        // loadHistory() also seeds events — without this, messages double up and the
        // keyed {#each} chokes on duplicate ids.
        const i = events.findIndex((x) => x.id === ev.id);
        if (i >= 0) {
          const next = events.slice();
          next[i] = ev;
          events = next;
        } else {
          events = [...events, ev];
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

    // Preview ao vivo (best-effort) do bloco de assistente em voo. Full-replace; tambem e prova de
    // vida (mas NAO a unica — entre turnos nao ha preview, por isso o ping ancora o watchdog).
    es.addEventListener('preview', (e: MessageEvent) => {
      armWatchdog();
      try {
        previewText = (JSON.parse(e.data) as { text?: string }).text ?? '';
      } catch {}
    });

    es.onerror = () => {
      // Erro REAL (TCP RST): reconecta em 3s. Half-open nao cai aqui -> coberto pelo watchdog.
      if (currentState !== 'dead') {
        clearTimeout(reconnectTimer);
        reconnectTimer = setTimeout(connectSSE, 3000);
      }
    };
  }

  // App voltou pro foreground (mobile suspende a conexao no background): reconecta na hora pra
  // re-sincronizar (o SSE reenvia transcript + estado atual). Pega o caso de background sem esperar
  // os 25s do watchdog.
  function onVisible() {
    if (document.visibilityState === 'visible') connectSSE();
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
  // como UNICO scroller. Sem position:fixed + translateY (que deixava o iOS rolar a pagina e
  // sumir com a NavBar). offsetTop compensa o scroll residual do iOS.
  $effect(() => {
    const vv = window.visualViewport;
    if (!vv || !screenEl) return;
    function fit() {
      if (!screenEl || !vv) return;
      // Ignora valores transientes (a animacao do teclado reporta alturas minusculas por 1 frame).
      if (vv.height < 120) return;
      const h = vv.height + 'px';
      // offsetTop = quanto o iOS PANEIA a visual viewport ao abrir o teclado (body travado em app.css
      // -> e pan VISUAL, nao scroll de documento; scrollTo nao resolveria). Compensamos via `top` em
      // position:relative (mesmo resultado do translateY, SEM transform): (a) nao promove o scroller a
      // layer com tiled-backing -> SEM retangulo preto (WebKit 220892/226532); (b) nao cria
      // containing-block que prenda os sheets position:fixed (SessionSwitcher/Usage/Activity da
      // .chat-screen E ModelEffort/Command/Confirm do .bottom-dock). NAO voltar a usar transform aqui.
      const top = (vv.offsetTop || 0) + 'px';
      if (screenEl.style.height !== h) screenEl.style.height = h;
      if (screenEl.style.top !== top) screenEl.style.top = top;
      if (screenEl.style.transform) screenEl.style.transform = ''; // limpa qualquer residuo de transform
    }
    function onFocusIn() {
      requestAnimationFrame(fit);
      setTimeout(fit, 300); // iOS as vezes so estabiliza apos a animacao do teclado
    }
    // iOS 26: offsetTop/height as vezes NAO zeram ao fechar o teclado (~24px residual, WebKit #297779).
    // No blur sem outro campo focado, forca estado limpo (senao sobra um vao no rodape).
    function onFocusOut() {
      setTimeout(() => {
        if (!screenEl) return;
        const a = document.activeElement;
        if (a && (a.tagName === 'TEXTAREA' || a.tagName === 'INPUT')) return; // trocou de campo
        screenEl.style.top = '0px';
        screenEl.style.height = '';   // volta pro height:100dvh do CSS
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
      for (const line of t.split('\n')) committed.add(line.trim());
    }
    const next = pending.filter((p) => !committed.has(p.text.trim()));
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
  const _norm = (s: string) => s.replace(/`/g, '').replace(/\s+/g, ' ').trim();
  let _asstCount = 0;
  $effect(() => {
    // CRÍTICO: ler previewText AQUI no topo, SEMPRE -> em Svelte 5 a dep só é rastreada se LIDA na
    // execução. Se a gente retornasse antes de ler (caminho idle), o effect não re-rodaria quando o
    // broker REEMITISSE o preview no idle -> o tail ficava (a duplicata que não saía). Lendo aqui, o
    // effect re-roda a cada update do preview e limpa.
    const pv = previewText;
    let c = 0;
    let last = '';
    for (const e of events) if (e.kind === 'assistant_msg' && e.text) { c++; last = e.text; }
    const committed = c > _asstCount;
    _asstCount = c;
    if (!pv) return;
    // (a) bloco novo commitou OU (b) saiu de working -> dropa.
    if (committed || currentState !== 'working') { previewText = ''; return; }
    // (c) residual coberto pela ÚLTIMA msg commitada (re-emit pós-commit, janela antes do idle).
    const p = _norm(pv);
    if (p.length >= 16 && last && _norm(last).includes(p)) previewText = '';
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
    try {
      await interrupt(sessionName);
    } catch (err) {
      console.error('interrupt error:', err);
    }
  }
</script>

<div class="chat-screen" bind:this={screenEl}>
  <NavBar title={sessionName} showBack={true} onBack={onBack} onTitleTap={openSwitcher} {status} onExpandUsage={() => (usageOpen = true)} onOpenActivity={hasActivity ? () => (activityOpen = true) : undefined} {activityBadge} {activityRunning} />

  {#if loading}
    <div class="chat-loading">
      <div class="spinner-lg" aria-label="Carregando…">⟳</div>
    </div>
  {:else if error}
    <div class="chat-error">
      <p>{error}</p>
      <button class="retry-btn" onclick={loadHistory}>Tentar novamente</button>
    </div>
  {:else}
    <MessageList
      {events}
      {stateEvent}
      {pending}
      {sessionName}
      {dockH}
      preview={previewText}
      onSelectOption={handleSelect}
      onCancel={handleInterrupt}
      onScrollActivity={handleScrollActivity}
    />
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
        {sessionName}
        sessionState={currentState}
        status={status}
        {scrolling}
        onSend={handleSend}
        onCommand={handleCommand}
        onInterrupt={handleInterrupt}
        onExpandUsage={() => (usageOpen = true)}
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

  <ActivitySheet open={activityOpen} {activity} {sessionName} onClose={() => (activityOpen = false)} />
</div>

<style>
  .chat-screen {
    display: flex;
    flex-direction: column;
    height: 100dvh;          /* fallback; o JS sobrescreve com visualViewport.height */
    overflow: hidden;
    position: relative;
    top: 0;                      /* baseline; o JS seta top = vv.offsetTop com o teclado aberto (pin do dock) */
    background: var(--bg-base);  /* backing solido (INVARIANTE): layer nunca renderiza preto no glitch do iOS */
    /* Higiene de stacking; reforço de baixo risco. isolation NAO cria containing block pros sheets
       position:fixed (filhos da .chat-screen) — ao contrário de transform/contain/will-change/filter,
       que clipariam os sheets. NÃO reintroduzir transform aqui (top relativo = sem layer, sem preto). */
    isolation: isolate;
  }

  .chat-loading,
  .chat-error {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-4);
  }

  .spinner-lg {
    font-size: 36px;
    color: var(--accent);
    animation: spin 0.8s linear infinite;
  }

  .chat-error p {
    font-size: var(--text-sm);
    color: var(--error);
    text-align: center;
    padding: 0 var(--space-4);
  }

  .retry-btn {
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
