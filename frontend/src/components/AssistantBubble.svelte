<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';
  import { parseFilePaths } from '../lib/format';
  import FileAttachment from './FileAttachment.svelte';

  interface Props {
    text: string;
    ts?: number | null;
    sessionName?: string;
  }
  let { text, ts, sessionName = '' }: Props = $props();

  const html = $derived(renderMarkdown(text));
  // Anexos por caminho citado na minha msg (img/video/html/pdf que eu "mandar").
  const fileRefs = $derived(sessionName ? parseFilePaths(text) : []);

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
  {#if fileRefs.length}<FileAttachment {sessionName} refs={fileRefs} />{/if}
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
    min-width: 0;        /* cadeia flex encolhe -> filhos (chip de arquivo) truncam, nao estouram */
    max-width: 100%;
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

  .prose :global(h1), .prose :global(h2), .prose :global(h3),
  .prose :global(h4), .prose :global(h5), .prose :global(h6) {
    font-weight: 600; color: var(--text-primary); line-height: 1.3;
    margin: var(--space-3) 0 var(--space-2);
  }
  .prose :global(h1) { font-size: 1.4em; }
  .prose :global(h2) { font-size: 1.25em; }
  .prose :global(h3) { font-size: 1.1em; }
  .prose :global(h4), .prose :global(h5), .prose :global(h6) { font-size: 1em; }

  .prose :global(ul) { list-style: disc; margin: var(--space-2) 0; padding-left: 1.4em; }
  .prose :global(ol) { list-style: decimal; margin: var(--space-2) 0; padding-left: 1.5em; }
  .prose :global(li) { line-height: 1.5; margin: 2px 0; }

  .prose :global(a) { color: var(--accent); text-decoration: underline; }

  .prose :global(blockquote) {
    border-left: 3px solid var(--border-default); padding-left: var(--space-3);
    margin: var(--space-2) 0; color: var(--text-secondary);
  }

  /* Tabela GFM: o WRAPPER rola na horizontal (box propria; a pagina nao mexe). hairlines discretas. */
  .prose :global(.md-table) {
    display: block; overflow-x: auto; -webkit-overflow-scrolling: touch;
    max-width: 100%; margin: var(--space-2) 0;
  }
  .prose :global(.md-table table) {
    /* largura NATURAL (nao espreme): rola no wrapper se passar da tela. */
    border-collapse: collapse; width: max-content; max-width: none; font-size: var(--text-sm);
  }
  .prose :global(th), .prose :global(td) {
    border: 1px solid var(--border-subtle); padding: 6px 10px; text-align: left; vertical-align: top;
    /* piso = nao colapsa pra quebra letra-a-letra; teto = nao vira uma mega-coluna (quebra por palavra). */
    min-width: 4.5em; max-width: 15em; overflow-wrap: break-word;
  }
  .prose :global(th) {
    background: var(--bg-elevated); font-weight: 600; color: var(--text-primary); white-space: nowrap;
  }

  .ts {
    font-size: var(--text-xs);
    color: var(--text-muted);
    margin-top: var(--space-1);
  }
</style>
