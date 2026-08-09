<script lang="ts">
  /**
   * A marca do Hangar: arcos concêntricos — a abóbada do hangar, e o aninhamento são as sessões
   * abrigadas juntas.
   *
   * `arcs`: 3 é o desenho normal; **2 abaixo de ~32px**, onde o terceiro arco empasta e a marca
   * vira mancha (medido reduzindo de verdade pra 16px, não no olho). Mesma geometria e mesmo grid
   * nos dois: só muda quanto detalhe cabe.
   *
   * Sem cor própria de propósito: `currentColor` faz a marca adotar o `--accent`, que vem da paleta
   * Material You do papel de parede (lib/desktopTheme.ts) e portanto muda por máquina. E sem fundo,
   * pra não virar retângulo opaco boiando sobre a foto quando o papel de parede está ligado.
   *
   * A fonte vetorial e o gerador de todos os tamanhos estão em `assets/brand/` (gen.py).
   */
  let { size = 24, arcs = 3 }: { size?: number; arcs?: 2 | 3 } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-linecap="round"
  aria-hidden="true"
>
  {#if arcs === 2}
    <g stroke-width="2.05">
      <path d="M 4.00 15.40 A 8.90 8.90 0 1 1 20.00 15.40" />
      <path d="M 6.82 12.60 A 5.30 5.30 0 1 1 17.18 12.60" />
    </g>
  {:else}
    <g stroke-width="1.55">
      <path d="M 4.12 16.05 A 9.10 9.10 0 1 1 19.88 16.05" />
      <path d="M 5.96 13.46 A 6.35 6.35 0 1 1 18.04 13.46" />
      <path d="M 8.42 11.88 A 3.60 3.60 0 1 1 15.58 11.88" />
    </g>
  {/if}
</svg>
