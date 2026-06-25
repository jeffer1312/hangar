<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import NavBar from '../components/NavBar.svelte';
  import MessageList from '../components/MessageList.svelte';
  import Composer from '../components/Composer.svelte';
  import SessionSwitcherSheet from '../components/SessionSwitcherSheet.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
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
  let dockEl: HTMLElement | undefined = $state();

  // ── Switcher de sessoes (NavBar -> sheet) + criar nova sem voltar ──────────
  let switcherOpen = $state(false);
  let createOpen = $state(false);
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

  // Lift the bottom dock (composer) above the on-screen keyboard, cross-platform.
  // iOS: window.innerHeight nao encolhe; o teclado vive na diferenca p/ visualViewport,
  // e o Safari ainda ROLA a viewport (offsetTop > 0) ao focar perto do fundo — por isso
  // o inset desconta offsetTop e escutamos 'scroll' alem de 'resize'. Android/Chrome com
  // interactive-widget=resizes-content encolhe o layout -> inset ~0 -> no-op (sem duplo).
  $effect(() => {
    if (!dockEl) return;
    const vv = window.visualViewport;
    if (!vv) return;
    function update() {
      if (!dockEl || !vv) return;
      const inset = window.innerHeight - vv.height - vv.offsetTop;
      dockEl.style.transform = `translateY(-${Math.max(0, inset)}px)`;
    }
    vv.addEventListener('resize', update);
    vv.addEventListener('scroll', update);
    // Foco na textarea pode preceder o resize do teclado: forca um update no proximo frame.
    function onFocusIn() {
      requestAnimationFrame(update);
      // segundo tick: iOS as vezes so estabiliza apos a animacao do teclado
      setTimeout(update, 300);
    }
    dockEl.addEventListener('focusin', onFocusIn);
    update();
    return () => {
      vv.removeEventListener('resize', update);
      vv.removeEventListener('scroll', update);
      dockEl?.removeEventListener('focusin', onFocusIn);
    };
  });

  async function handleSend(text: string) {
    try {
      await sendInput(sessionName, text);
    } catch (err) {
      console.error('sendInput error:', err);
    }
  }

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

<div class="chat-screen">
  <NavBar title={sessionName} showBack={true} onBack={onBack} onTitleTap={openSwitcher} />

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
      onSelectOption={handleSelect}
      onCancel={handleInterrupt}
    />
  {/if}

  <div class="bottom-dock" bind:this={dockEl}>
    {#if currentState === 'dead'}
      <div class="dead-footer">
        <p class="dead-text">Esta sessão foi encerrada.</p>
        <button class="back-btn" onclick={onBack}>← Voltar</button>
      </div>
    {:else if currentState !== 'awaiting_input'}
      <Composer
        {sessionName}
        sessionState={currentState}
        status={status}
        label={stateEvent?.label}
        onSend={handleSend}
        onInterrupt={handleInterrupt}
        onCommand={handleCommand}
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
</div>

<style>
  .chat-screen {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
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

  /* Fixed bottom dock: statusline bar + composer (or dead footer) */
  .bottom-dock {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    z-index: 30;
    background: var(--bg-base);
    padding-bottom: env(safe-area-inset-bottom);
    will-change: transform;
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
