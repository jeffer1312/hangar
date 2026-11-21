<script lang="ts">
  import { codeLabel, elementLabel, parseContextRefs, type QuoteRef } from '@hangar/core';
  import { renderMarkdown } from '../lib/markdown';
  import * as m from '../paraglide/messages';

  // Mensagem do usuário com blocos <hangar-ref>: a citação em destaque com o comentário fora dela, o
  // elemento como cartão compacto, as linhas de arquivo com o nome, as linhas e o código em fonte
  // monoespaçada, e o texto digitado como sempre (texto puro, sem markdown).
  interface Props {
    text: string;
    /** Toque na citação: rola até a resposta de origem. Ausente = citação estática. */
    onQuoteClick?: ((ref: QuoteRef) => void) | null;
  }
  let { text, onQuoteClick = null }: Props = $props();

  const segments = $derived(parseContextRefs(text));

  // Citação com origem vira botão: o mesmo corpo, com papel, foco e os dois gatilhos.
  function quoteButton(ref: QuoteRef) {
    return {
      role: 'button', tabindex: 0, 'aria-label': m.cite_go_to_source(), title: m.cite_go_to_source(),
      onclick: () => onQuoteClick?.(ref),
      onkeydown: (e: KeyboardEvent) => {
        if (e.key !== 'Enter' && e.key !== ' ') return;
        e.preventDefault();
        onQuoteClick?.(ref);
      },
    };
  }
</script>

<div class="crefs">
  {#each segments as seg, i (i)}
    {#if 'ref' in seg}
      {@const ref = seg.ref}
      {@const comment = ref.comment.trim()}
      {#if ref.kind === 'quote'}
        {@const clickable = !!onQuoteClick && !!ref.source}
        <!-- O trecho da web é texto selecionado; o do nativo é a fonte Markdown. Os dois passam pelo
             renderMarkdown, que escapa tudo: assim `**` nunca aparece cru. -->
        <blockquote class="cref-quote" class:clickable>
          <div class="cref-body" {...(clickable ? quoteButton(ref) : {})}>
            <span class="cref-head">{m.cite_quoted()}</span>
            <!-- eslint-disable-next-line svelte/no-at-html-tags -->
            <div class="cref-text">{@html renderMarkdown(ref.text)}</div>
          </div>
        </blockquote>
      {:else if ref.kind === 'code'}
        {@const label = codeLabel(ref)}
        <div class="cref-quote cref-code">
          <span class="cref-head">{m.cite_code()}</span>
          <span class="cref-code-name" title={ref.path}>{label}</span>
          {#if !label.startsWith(ref.path)}<span class="cref-el-url">{ref.path}</span>{/if}
          <pre class="cref-code-text" data-lang={ref.lang || undefined}>{ref.text}</pre>
        </div>
      {:else}
        <div class="cref-el">
          <span class="cref-head">{m.cite_element()}</span>
          <span class="cref-el-name">{elementLabel(ref)}</span>
          {#if ref.url}<span class="cref-el-url">{ref.url}</span>{/if}
          {#if ref.html}
            <details class="cref-el-html">
              <summary>{m.cite_element_html()}</summary>
              <pre>{ref.html}</pre>
            </details>
          {/if}
        </div>
      {/if}
      {#if comment}<p class="cref-comment">{comment}</p>{/if}
    {:else if seg.text.trim()}
      <p class="cref-typed">{seg.text.trim()}</p>
    {/if}
  {/each}
</div>

<style>
  .crefs { display: flex; flex-direction: column; gap: var(--space-2); min-width: 0; }
  .crefs p { margin: 0; }
  .cref-typed, .cref-comment {
    font-size: calc(var(--text-base) * var(--cp-text-scale, 1));
    line-height: calc(1.55 * var(--cp-lh-scale, 1));
    white-space: pre-wrap;
  }
  .cref-quote {
    margin: 0;
    border-left: 3px solid var(--accent);
    padding-left: var(--space-3);
    color: var(--text-secondary);
  }
  .cref-body { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .cref-quote.clickable .cref-body { cursor: pointer; -webkit-tap-highlight-color: transparent; }
  .cref-quote.clickable .cref-body:active { opacity: 0.7; }
  .cref-quote.clickable .cref-body:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: 2px; }
  .cref-head { font-size: var(--text-xs); font-weight: 600; color: var(--accent); }
  .cref-text {
    position: relative;
    font-size: var(--text-sm);
    line-height: 1.5;
    max-height: 12lh;
    overflow: hidden;
    overflow-wrap: anywhere;
  }
  .cref-text :global(p) { margin: 0; }
  .cref-text :global(p + p) { margin-top: var(--space-1); }
  .cref-text :global(code) { font-family: var(--font-mono); font-size: 0.875em; }
  .cref-text :global(pre) { margin: var(--space-1) 0; white-space: pre-wrap; font-size: 0.875em; }
  .cref-code { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .cref-code-name { font-family: var(--font-mono); font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); overflow-wrap: anywhere; }
  .cref-code-text {
    position: relative;
    margin: var(--space-1) 0 0;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.5;
    white-space: pre;
    max-height: 12lh;
    overflow: auto;
  }
  .cref-el {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: transparent;
    min-width: 0;
  }
  .cref-el-name { font-size: var(--text-sm); font-weight: 600; overflow-wrap: anywhere; }
  .cref-el-url {
    font-size: var(--text-xs);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cref-el-html summary { cursor: pointer; font-size: var(--text-xs); color: var(--text-secondary); padding-block: 4px; }
  .cref-el-html pre {
    position: relative;
    margin: var(--space-1) 0 0;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 12lh;
    overflow: auto;
  }
</style>
