<script lang="ts">
  import IconSend from './icons/IconSend.svelte';
  import IconInterrupt from './icons/IconInterrupt.svelte';
  import type { State } from '../lib/types';

  interface Props {
    sessionState: State;
    onSend: (text: string) => void;
    onInterrupt: () => void;
  }
  let { sessionState, onSend, onInterrupt }: Props = $props();

  let inputText = $state('');
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  const isWorking = $derived(sessionState === 'working');
  const canSend = $derived(inputText.trim().length > 0 && !isWorking);

  // Auto-grow textarea
  function autoGrow() {
    if (!textareaEl) return;
    textareaEl.style.height = 'auto';
    textareaEl.style.height = Math.min(textareaEl.scrollHeight, 120) + 'px';
  }

  function handleInput() {
    autoGrow();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  function submit() {
    if (!canSend) return;
    const msg = inputText.trim();
    inputText = '';
    if (textareaEl) {
      textareaEl.style.height = 'auto';
    }
    onSend(msg);
  }

  // Auto-focus when idle
  $effect(() => {
    if (sessionState === 'idle' && textareaEl) {
      setTimeout(() => textareaEl?.focus(), 100);
    }
  });
</script>

<footer class="composer">
  <div class="composer-inner">
    <div class="input-row">
      <textarea
        bind:this={textareaEl}
        bind:value={inputText}
        class="composer-textarea"
        placeholder="Mensagem para Claude…"
        rows={1}
        disabled={isWorking}
        style={isWorking ? 'opacity: 0.4; pointer-events: none;' : ''}
        oninput={handleInput}
        onkeydown={handleKeydown}
        aria-label="Mensagem"
      ></textarea>
      <button
        class="send-btn"
        class:send-btn--disabled={!canSend}
        onclick={submit}
        disabled={!canSend}
        aria-label="Enviar mensagem"
      >
        <IconSend size={18} />
      </button>
    </div>

    {#if isWorking}
      <button class="interrupt-btn" onclick={onInterrupt} aria-label="Interromper Claude">
        <IconInterrupt size={16} />
        <span>Interromper</span>
      </button>
    {/if}
  </div>
</footer>

<style>
  .composer {
    background: var(--bg-base);
  }

  .composer-inner {
    padding: var(--space-3) var(--space-4);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 600px;
    width: 100%;
    margin: 0 auto;
  }

  .input-row {
    display: flex;
    align-items: flex-end;
    gap: var(--space-2);
  }

  .composer-textarea {
    flex: 1;
    min-height: 44px;
    max-height: 120px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* prevent iOS zoom */
    line-height: 1.55;
    padding: 10px var(--space-3);
    resize: none;
    outline: none;
    transition: border-color 180ms ease-out;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .composer-textarea::placeholder {
    color: var(--text-muted);
  }

  .composer-textarea:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .send-btn {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    transition: background 180ms ease-out;
  }

  .send-btn:active:not(:disabled) {
    background: var(--accent-press);
  }

  .send-btn--disabled {
    background: var(--bg-hover);
    color: var(--text-muted);
    cursor: default;
  }

  .interrupt-btn {
    width: 100%;
    height: 44px;
    background: transparent;
    border: 1px solid var(--error);
    border-radius: var(--radius-md);
    color: var(--error);
    font-size: var(--text-sm);
    font-weight: 500;
    gap: var(--space-2);
    transition: background 180ms ease-out;
  }

  .interrupt-btn:active {
    background: rgba(255, 69, 58, 0.08);
  }
</style>
