<script lang="ts">
  // Popover de espiada (desktop): última resposta do Claude da sessão sob o mouse. A posição já vem
  // resolvida pelo chamador (coordenadas de viewport) — aqui só desenha.
  interface Props { text: string; x: number; y: number }
  let { text, x, y }: Props = $props();
</script>

<div class="hover-preview" style="left: {x}px; top: {y}px" role="tooltip">
  <p>{text}</p>
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
  }
  .hover-preview p {
    margin: 0; font-size: var(--text-xs); font-weight: 400; color: var(--text-secondary);
    line-height: 1.45;
    white-space: pre-wrap; word-break: break-word;
    /* line-clamp padrão junto do -webkit- (sem ele o svelte-check acusa compatibilidade). */
    display: -webkit-box; -webkit-line-clamp: 10; line-clamp: 10; -webkit-box-orient: vertical; overflow: hidden;
  }
  /* Só opacidade: transform aqui promoveria camada (e o board já tem a única pulsação do sistema). */
  @keyframes hp-in { from { opacity: 0; } to { opacity: 1; } }
</style>
