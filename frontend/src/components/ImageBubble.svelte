<script lang="ts">
  import * as m from '../paraglide/messages';
  import { abrirVisor } from '../lib/visor';

  interface Props {
    caption: string;
    srcs: string[];
  }
  let { caption, srcs }: Props = $props();

  // Abre no visor compartilhado (lib/visor.ts) com TODAS as imagens desta bolha: quem manda 4 fotos
  // de uma vez passa entre elas com as setas em vez de fechar e abrir uma a uma.
  // Os botoes das miniaturas: o visor le deles o tamanho natural da imagem (sem isso ele abre vazio)
  // e a origem da animacao.
  const botoes: (HTMLElement | undefined)[] = [];

  function abrir(i: number) {
    void abrirVisor(
      srcs.map((src, j) => ({
        url: src,
        nome: (src.split('/').pop() ?? src).split('?')[0],
        tipo: 'image' as const,
        element: botoes[j],
      })),
      i,
    );
  }
</script>

<div class="image-bubble">
  <!-- Anexos primeiro (miniaturas), legenda embaixo — disposicao estilo Claude. -->
  <div class="thumb-row" class:thumb-row--multi={srcs.length > 1}>
    {#each srcs as src, i}
      <button class="thumb-btn" bind:this={botoes[i]} onclick={() => abrir(i)} aria-label={m.anexos_ver_original()}>
        <img class="thumb" {src} alt={m.anexos_imagem_enviada()} loading="lazy" />
      </button>
    {/each}
  </div>
  {#if caption}<p class="image-caption">{caption}</p>{/if}
</div>

<style>
  .image-bubble {
    align-self: flex-start;     /* mesma margem da resposta, igual ao UserBubble */
    max-width: 80%;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: var(--space-1) 0 var(--space-3);
    padding: var(--space-2);
    background: var(--accent-dim);
    border-radius: 18px 18px 18px 4px;
    animation: bubble-in 200ms var(--ease-out);
  }

  /* Miniaturas: pequenas pra nao inflar o bubble. Uma imagem -> 1 thumb; varias -> grade. */
  .thumb-row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    justify-content: flex-start;
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
    /* Filete inset neutro: sem ele a borda da foto "vaza" pra superficie quando os tons coincidem
       (regra do make-interfaces-feel-better, com o token de borda do tema em vez de rgba fixo). */
    outline: 1px solid var(--border-default);
    outline-offset: -1px;
    border-radius: var(--radius-md);
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

</style>
