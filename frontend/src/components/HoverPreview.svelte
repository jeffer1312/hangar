<script lang="ts">
  // Popover de espiada (desktop): última resposta do Claude da sessão sob o mouse. A posição já vem
  // resolvida pelo chamador (coordenadas de viewport) — aqui só desenha. Markdown de verdade
  // (mesmo renderer das bolhas): a resposta é markdown, texto cru lia mal (** soltos, listas
  // viravam parágrafo).
  import { renderMarkdown } from '../lib/markdown';
  interface Props { text: string; x: number; y: number }
  let { text, x, y }: Props = $props();
  const html = $derived(renderMarkdown(text));
</script>

<div class="hover-preview" style="left: {x}px; top: {y}px" role="tooltip">
  {@html html}
</div>

<style>
  .hover-preview {
    position: fixed;
    /* Abaixo do backdrop dos menus (40) e dos modais (50/51): uma espiada nunca cobre menu/diálogo. */
    z-index: 39;
    /* Decoração pura: não pode receber o ponteiro, senão roubaria o hover da própria linha que a
       abriu (mouseleave -> fecha -> mouseenter -> abre = piscada infinita). */
    pointer-events: none;
    max-width: 380px; max-height: 220px; overflow: hidden;
    /* Mesma receita de elevação dos popovers da sidebar (.ctx-menu): 1 borda hairline + 1 sombra. */
    background: var(--bg-elevated); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); padding: var(--space-2) var(--space-3);
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.4);
    animation: hp-in 120ms var(--ease-out);
    font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.45;
  }
  /* Markdown vem do {@html} -> :global. Escala compacta de popover: sem margens generosas de chat. */
  .hover-preview :global(p), .hover-preview :global(ul), .hover-preview :global(ol),
  .hover-preview :global(pre), .hover-preview :global(blockquote) { margin: 0 0 6px; }
  .hover-preview :global(ul), .hover-preview :global(ol) { padding-left: 18px; }
  .hover-preview :global(h1), .hover-preview :global(h2), .hover-preview :global(h3),
  .hover-preview :global(h4) { margin: 0 0 4px; font-size: var(--text-xs); font-weight: 600; color: var(--text-primary); }
  .hover-preview :global(code) {
    font-family: var(--font-mono); font-size: 0.7rem;
    background: var(--bg-hover); padding: 1px 4px; border-radius: 4px;
  }
  .hover-preview :global(pre) { overflow: hidden; background: var(--bg-hover); padding: 6px 8px; border-radius: var(--radius-sm); }
  .hover-preview :global(pre code) { background: none; padding: 0; }
  /* Só opacidade: transform aqui promoveria camada (e o board já tem a única pulsação do sistema). */
  @keyframes hp-in { from { opacity: 0; } to { opacity: 1; } }
</style>
