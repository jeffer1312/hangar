<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import NavBar from '../components/NavBar.svelte';
  import StatusPill from '../components/StatusPill.svelte';
  import MessageList from '../components/MessageList.svelte';
  import Composer from '../components/Composer.svelte';
  import { getHistory, sendInput, selectOption, interrupt, openEventStream } from '../lib/api';
  import type { ChatEvent, StateEvent, State } from '../lib/types';

  interface Props {
    sessionName: string;
    onBack: () => void;
  }
  let { sessionName, onBack }: Props = $props();

  let events = $state<ChatEvent[]>([]);
  let stateEvent = $state<StateEvent | null>(null);
  let loading = $state(true);
  let error = $state('');
  let es: EventSource | null = null;

  const currentState = $derived<State>(stateEvent?.state ?? 'idle');

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
        events = [...events, ev];
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

  async function handleSend(text: string) {
    try {
      await sendInput(sessionName, text);
    } catch (err) {
      console.error('sendInput error:', err);
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
  <NavBar title={sessionName} showBack={true} onBack={onBack} />

  <StatusPill state={currentState} label={stateEvent?.label} />

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

  {#if currentState === 'dead'}
    <div class="dead-footer">
      <p class="dead-text">Esta sessão foi encerrada.</p>
      <button class="back-btn" onclick={onBack}>← Voltar</button>
    </div>
  {:else if currentState !== 'awaiting_input'}
    <Composer
      sessionState={currentState}
      onSend={handleSend}
      onInterrupt={handleInterrupt}
    />
  {/if}
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

  /* Dead state footer */
  .dead-footer {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-5) var(--space-6);
    padding-bottom: calc(env(safe-area-inset-bottom) + var(--space-5));
    border-top: 1px solid var(--border-subtle);
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
