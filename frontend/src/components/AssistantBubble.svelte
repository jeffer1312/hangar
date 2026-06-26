<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';

  interface Props {
    text: string;
    ts?: number | null;
  }
  let { text, ts }: Props = $props();

  const html = $derived(renderMarkdown(text));

  function formatTime(ts: number | null | undefined): string {
    if (!ts) return '';
    return new Date(ts * 1000).toLocaleTimeString('pt-BR', {
      hour: '2-digit',
      minute: '2-digit',
    });
  }
</script>

<div class="assistant-msg">
  <!-- eslint-disable-next-line svelte/no-at-html-tags -->
  <div class="prose">{@html html}</div>
  {#if ts}
    <span class="ts">{formatTime(ts)}</span>
  {/if}
</div>

<style>
  /* Mensagem do assistente SEM bubble: texto full-width (estilo Claude iOS), mais legivel. */
  .assistant-msg {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    animation: bubble-in 220ms var(--ease-out) both;
    margin-bottom: var(--space-4);
  }

  .prose {
    color: var(--text-primary);
    max-width: 100%;
    word-break: break-word;
    font-size: var(--text-base);
    line-height: 1.6;
  }

  .prose :global(p) { margin: 0; }
  .prose :global(p + p) { margin-top: var(--space-3); }
  .prose :global(br) { display: block; content: ''; margin-top: var(--space-2); }
  .prose :global(strong) { font-weight: 600; color: var(--text-primary); }
  .prose :global(em) { font-style: italic; color: var(--text-secondary); }

  .prose :global(code) {
    font-family: var(--font-mono);
    font-size: 0.875em;
    background: var(--bg-elevated);
    padding: 2px 5px;
    border-radius: 4px;
    color: var(--text-primary);
  }

  .prose :global(pre) {
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    padding: var(--space-3);
    overflow-x: auto;
    margin: var(--space-2) 0;
    -webkit-overflow-scrolling: touch;
  }

  .prose :global(pre code) {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    line-height: 1.3;
    background: none;
    padding: 0;
    border-radius: 0;
  }

  .ts {
    font-size: var(--text-xs);
    color: var(--text-muted);
    margin-top: var(--space-1);
  }
</style>
