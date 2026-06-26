<script lang="ts">
  interface Props {
    caption: string;
    srcs: string[];
  }
  let { caption, srcs }: Props = $props();

  // Imagem aberta no lightbox (null = fechado). Clique na miniatura abre o original em tela cheia.
  let lightbox = $state<string | null>(null);

  // Move o no pro <body> pra escapar do transform + overflow do .chat-screen — senao o overlay
  // position:fixed fica relativo ao container transformado e some atras do composer/topbar.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }
</script>

<div class="image-bubble">
  <!-- Anexos primeiro (miniaturas), legenda embaixo — disposicao estilo Claude. -->
  <div class="thumb-row" class:thumb-row--multi={srcs.length > 1}>
    {#each srcs as src}
      <button class="thumb-btn" onclick={() => (lightbox = src)} aria-label="Ver imagem original">
        <img class="thumb" {src} alt="imagem enviada" loading="lazy" />
      </button>
    {/each}
  </div>
  {#if caption}<p class="image-caption">{caption}</p>{/if}
</div>

{#if lightbox}
  <!-- Overlay tela cheia com o original; toque em qualquer lugar fecha. Portal pro body. -->
  <button use:portal class="lightbox" onclick={() => (lightbox = null)} aria-label="Fechar imagem">
    <img class="lightbox-img" src={lightbox} alt="imagem original" />
  </button>
{/if}

<style>
  .image-bubble {
    align-self: flex-end;
    max-width: 80%;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: var(--space-1) 0 var(--space-3);
    padding: var(--space-2);
    background: var(--accent-dim);
    border-radius: 18px 18px 4px 18px;
    animation: bubble-in 200ms var(--ease-out);
  }

  /* Miniaturas: pequenas pra nao inflar o bubble. Uma imagem -> 1 thumb; varias -> grade. */
  .thumb-row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    justify-content: flex-end;
  }
  .thumb-btn {
    padding: 0;
    border: none;
    background: none;
    line-height: 0;
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .thumb {
    width: 96px;
    height: 96px;
    object-fit: cover;
    display: block;
  }
  /* Com varias, encolhe um pouco pra caber 2-3 por linha sem estourar. */
  .thumb-row--multi .thumb {
    width: 80px;
    height: 80px;
  }

  .image-caption {
    font-size: var(--text-sm);
    color: var(--text-primary);
    white-space: pre-wrap;
    word-break: break-word;
    text-align: left;
  }

  .lightbox {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-4);
    background: rgba(0, 0, 0, 0.92);
    border: none;
    /* respeita as safe-areas do iOS */
    padding-top: calc(var(--space-4) + env(safe-area-inset-top));
    padding-bottom: calc(var(--space-4) + env(safe-area-inset-bottom));
  }
  .lightbox-img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border-radius: var(--radius-md);
  }
</style>
