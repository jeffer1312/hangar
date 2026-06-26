<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import NavBar from '../components/NavBar.svelte';
  import MessageList from '../components/MessageList.svelte';
  import Composer from '../components/Composer.svelte';
  import SessionSwitcherSheet from '../components/SessionSwitcherSheet.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
  import UsageSheet from '../components/UsageSheet.svelte';
  import {
    getHistory,
    sendInput,
    selectOption,
    interrupt,
    openEventStream,
    getSessions,
    createSession,
  } from '../lib/api';
  import { parseStatusLine } from '../lib/statusline';
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
  let screenEl: HTMLElement | undefined = $state();
  let pending = $state<{ id: string; text: string }[]>([]);

  // ── Switcher de sessoes (NavBar -> sheet) + criar nova sem voltar ──────────
  let switcherOpen = $state(false);
  let createOpen = $state(false);
  let usageOpen = $state(false);
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

  async function loadHistory() {
    try {
      events = await getHistory(sessionName);
    } catch (err) {
      error = err instanceof Error ? err.message : 'Erro ao carregar histórico';
    } finally {
      loading = false;
    }
  }

  function connectSSE() {
    if (es) { es.close(); es = null; }

    es = openEventStream(sessionName);

    es.addEventListener('message', (e: MessageEvent) => {
      try {
        const ev = JSON.parse(e.data) as ChatEvent;
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
      try {
        stateEvent = JSON.parse(e.data) as StateEvent;
      } catch {}
    });

    es.onerror = () => {
      // Reconnect after 3s if not dead
      if (currentState !== 'dead') {
        setTimeout(connectSSE, 3000);
      }
    };
  }

  onMount(async () => {
    await loadHistory();
    connectSSE();
  });

  onDestroy(() => {
    es?.close();
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
      screenEl.style.height = vv.height + 'px';
      screenEl.style.transform = `translateY(${vv.offsetTop}px)`;
    }
    function onFocusIn() {
      requestAnimationFrame(fit);
      setTimeout(fit, 300); // iOS as vezes so estabiliza apos a animacao do teclado
    }
    fit();
    vv.addEventListener('resize', fit);
    vv.addEventListener('scroll', fit);
    screenEl.addEventListener('focusin', onFocusIn);
    return () => {
      vv.removeEventListener('resize', fit);
      vv.removeEventListener('scroll', fit);
      screenEl?.removeEventListener('focusin', onFocusIn);
    };
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

  // Dedup: quando o transcript (SSE) traz o user_msg real, solta o pending de mesmo texto.
  // Idempotente -> nao entra em loop (apos filtrar, o length estabiliza e nao reatribui).
  $effect(() => {
    if (pending.length === 0) return;
    const committed = new Set(
      events.filter((e) => e.kind === 'user_msg' && e.text).map((e) => e.text)
    );
    const next = pending.filter((p) => !committed.has(p.text));
    if (next.length !== pending.length) pending = next;
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
  <NavBar title={sessionName} showBack={true} onBack={onBack} onTitleTap={openSwitcher} {status} onExpandUsage={() => (usageOpen = true)} />

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
      onSelectOption={handleSelect}
      onCancel={handleInterrupt}
    />
  {/if}

  <div class="bottom-dock">
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
    onClose={() => (createOpen = false)}
    onCreate={handleCreate}
    onOpenSession={onNavigateToChat}
  />

  <UsageSheet open={usageOpen} {status} onClose={() => (usageOpen = false)} />
</div>

<style>
  .chat-screen {
    display: flex;
    flex-direction: column;
    height: 100dvh;          /* fallback; o JS sobrescreve com visualViewport.height */
    overflow: hidden;
    transform-origin: top;
    position: relative;
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
