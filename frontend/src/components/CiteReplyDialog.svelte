<script lang="ts">
  import { codeLabel, elementLabel, withContextRefComment } from '@hangar/core';
  import * as m from '../paraglide/messages';
  import ModalDialog from './ModalDialog.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import { citeDialog } from '../lib/citeDialog.svelte';
  import { contextRefsDraft } from '../lib/contextRefsDraft.svelte';

  // "Responder ao trecho": o trecho citado em cima (rolagem própria) e a resposta da pessoa embaixo.
  // "Comentar e continuar" deixa a conversa onde estava para citar o próximo trecho; "Finalizar" leva
  // ao field de mensagem. Quem mexe na conversa e no field é o Chat, por `onFinish`; aqui só se
  // adiciona à bandeja e fecha.
  interface Props {
    onFinish?: () => void;
  }
  let { onFinish }: Props = $props();

  const request = $derived(citeDialog.request);
  let comment = $state('');
  let field = $state<HTMLTextAreaElement | null>(null);

  $effect(() => {
    comment = request?.ref.comment ?? '';
  });

  // Enter adiciona só com teclado de verdade: no teclado da tela do celular ele quebra linha, e o
  // botão Adicionar está ali.
  const coarsePointer = typeof window !== 'undefined' && window.matchMedia('(pointer: coarse)').matches;

  function addToTray(finish: boolean) {
    const p = citeDialog.request;
    if (!p) return;
    contextRefsDraft.add(p.draftKey, withContextRefComment(p.ref, comment.trim()));
    document.getSelection()?.removeAllRanges();
    citeDialog.close();
    if (finish) onFinish?.();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key !== 'Enter' || e.isComposing) return;
    if (e.ctrlKey || e.metaKey) { e.preventDefault(); addToTray(true); return; }
    if (!e.shiftKey && !coarsePointer) { e.preventDefault(); addToTray(false); }
  }
</script>

<ModalDialog open={!!request} ariaLabel={m.cite_reply_title()} onClose={() => citeDialog.close()}
             className="cite-reply" initialFocus={field}>
  {#if request}
    {@const ref = request.ref}
    <h2 class="cr-titulo">{m.cite_reply_title()}</h2>
    <div class="cr-trecho">
      {#if ref.kind === 'quote'}
        <!-- Mesmo renderMarkdown das bolhas: escapa tudo, e `**` nunca aparece cru. -->
        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
        {@html renderMarkdown(ref.text)}
      {:else if ref.kind === 'code'}
        <p class="cr-el">{codeLabel(ref)}</p>
        <pre class="cr-code">{ref.text}</pre>
      {:else}
        <p class="cr-el">{elementLabel(ref)}</p>
        {#if ref.url}<p class="cr-url">{ref.url}</p>{/if}
      {/if}
    </div>
    <textarea bind:this={field} bind:value={comment} class="cr-campo" rows="3"
              placeholder={m.cite_reply_placeholder()} aria-label={m.cite_reply_placeholder()}
              onkeydown={onKey}></textarea>
    <div class="cr-acoes">
      <button type="button" class="cr-btn" onclick={() => citeDialog.close()}>{m.comum_cancelar()}</button>
      <button type="button" class="cr-btn" onclick={() => addToTray(true)}>{m.cite_finish()}</button>
      <button type="button" class="cr-btn cr-primario" onclick={() => addToTray(false)}>{m.cite_comment_continue()}</button>
    </div>
  {/if}
</ModalDialog>

<style>
  :global(.cite-reply) {
    width: min(560px, 100%);
    max-height: calc(100dvh - var(--space-8));
    padding: var(--space-4);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    box-sizing: border-box;
  }
  .cr-titulo { margin: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  /* Trecho longo rola dentro da própria caixa; o field e os botões ficam sempre à vista. */
  .cr-trecho {
    position: relative;
    flex: 0 1 auto;
    min-height: 0;
    max-height: 40dvh;
    overflow-y: auto;
    overscroll-behavior: contain;
    border-left: 3px solid var(--accent);
    padding: var(--space-1) var(--space-3);
    font-size: var(--text-sm);
    line-height: 1.5;
    color: var(--text-secondary);
    overflow-wrap: anywhere;
  }
  .cr-trecho :global(p) { margin: 0; }
  .cr-trecho :global(p + p) { margin-top: var(--space-1); }
  .cr-trecho :global(pre) { white-space: pre-wrap; font-size: 0.875em; margin: var(--space-1) 0; }
  .cr-trecho :global(code) { font-family: var(--font-mono); font-size: 0.875em; }
  .cr-el { font-weight: 600; color: var(--text-primary); }
  .cr-trecho .cr-code { margin: var(--space-1) 0 0; font-family: var(--font-mono); font-size: var(--text-xs); white-space: pre; overflow-x: auto; }
  .cr-url { font-size: var(--text-xs); color: var(--text-muted); }
  /* 16px: abaixo disso o Safari do iPhone dá zoom na página ao focar. */
  .cr-campo {
    flex-shrink: 0;
    resize: vertical;
    min-height: 4.5rem;
    max-height: 30dvh;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    border: 1px solid var(--border-default);
    background: var(--surface-inset);
    color: var(--text-primary);
    font: inherit;
    font-size: 16px;
    line-height: 1.45;
  }
  .cr-acoes { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: var(--space-2); flex-shrink: 0; }
  .cr-btn {
    min-height: 44px;
    padding: 0 var(--space-4);
    border-radius: var(--radius-full);
    border: 1px solid var(--border-default);
    background: transparent;
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-weight: 600;
    cursor: pointer;
  }
  .cr-primario { background: var(--accent); border-color: var(--accent); color: var(--bg-base); }
</style>
