<script lang="ts">
  import { codeOverlay } from '../lib/codeActions.svelte';
  import { highlightCodeBlocks } from '../lib/highlight';

  // Overlay fullscreen de um bloco de codigo (botao ⤢ do header do code-block). O codigo chega CRU
  // e e highlightado aqui de novo (highlightCodeBlocks e idempotente), entao o overlay nao depende
  // de o bloco original ja ter sido colorido na conversa.
  // O pre fica dentro de .code-block de proposito: o copy-btn do header pega carona no handler
  // GLOBAL de code-actions (lib/codeActions.svelte.ts), sem handler proprio.

  let preEl = $state<HTMLElement | null>(null);

  $effect(() => {
    const atual = codeOverlay.atual;
    const el = preEl;
    if (!atual || !el) return;
    void highlightCodeBlocks(el);
  });

  function aoCliqueFora(e: MouseEvent) {
    if (e.target === e.currentTarget) codeOverlay.fechar();
  }
</script>

{#if codeOverlay.atual}
  {@const atual = codeOverlay.atual}
  <!-- svelte-ignore a11y_click_events_have_key_events (o Esc ja fecha, ver codeActions) -->
  <div class="code-overlay" onclick={aoCliqueFora} role="presentation">
    <div class="code-overlay-card code-block" role="dialog" aria-label="Código expandido">
      <div class="code-head">
        <span class="code-lang">{atual.lang || 'Código'}</span>
        <button class="copy-btn" type="button" aria-label="Copiar código"></button>
        <button class="code-overlay-close" type="button" aria-label="Fechar" onclick={() => codeOverlay.fechar()}>✕</button>
      </div>
      <pre bind:this={preEl}><code class={atual.lang ? `language-${atual.lang}` : ''}>{atual.code}</code></pre>
    </div>
  </div>
{/if}

<style>
  .code-overlay {
    position: fixed;
    inset: 0;
    z-index: 1200;  /* acima de TUDO: BottomSheet z-100, SettingsModal z-90, ModalDialog z-1000 —
                       expandir codigo vale de dentro de qualquer painel. z-80 (1a versao) abria o
                       overlay ATRAS de uma sheet aberta: invisivel, e o Esc em capture morria calado. */
    display: flex;
    align-items: stretch;
    justify-content: center;
    padding: calc(var(--space-6) + env(safe-area-inset-top)) var(--space-4) calc(var(--space-6) + env(safe-area-inset-bottom));
    background: rgba(0, 0, 0, 0.55);
    animation: overlay-in 160ms var(--ease-out) both;
  }
  @keyframes overlay-in {
    from { opacity: 0; }
    to   { opacity: 1; }
  }
  .code-overlay-card {
    display: flex;
    flex-direction: column;
    width: min(960px, 100%);
    min-height: 0;                 /* deixa o pre encolher e rolar DENTRO do card */
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    overflow: hidden;
    box-shadow: 0 18px 50px rgba(0, 0, 0, 0.45);
    animation: card-in 220ms var(--spring) both;
  }
  @keyframes card-in {
    from { opacity: 0; transform: translateY(10px) scale(0.98); }
    to   { opacity: 1; transform: none; }
  }
  /* O header fica colado no topo; o pre rola em DUAS direcoes (codigo comprido e linha longa). */
  .code-overlay-card .code-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border-subtle);
    flex-shrink: 0;
  }
  .code-overlay-card .code-lang {
    flex: 1;
    min-width: 0;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-secondary);
    text-transform: lowercase;
  }
  .code-overlay-card pre {
    flex: 1;
    min-height: 0;
    overflow: auto;
    margin: 0;
    padding: var(--space-4);
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    line-height: 1.45;
    color: var(--text-primary);
    -webkit-overflow-scrolling: touch;
  }
  .code-overlay-card pre code { background: none; padding: 0; }
  .code-overlay-card .copy-btn {
    width: 28px; height: 28px; padding: 0;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    background: transparent; color: var(--text-secondary);
    cursor: pointer; opacity: 0.75;
  }
  .code-overlay-card .copy-btn::before { content: '⧉'; font-size: 15px; line-height: 1; }
  .code-overlay-card .copy-btn.copied { color: var(--accent); opacity: 1; }
  .code-overlay-card .copy-btn.copied::before { content: '✓'; }
  .code-overlay-close {
    width: 28px; height: 28px; padding: 0;
    display: flex; align-items: center; justify-content: center;
    border: none; border-radius: var(--radius-sm);
    background: transparent; color: var(--text-secondary);
    font-size: 14px; cursor: pointer;
  }
  .code-overlay-close:hover { color: var(--text-primary); }
</style>
