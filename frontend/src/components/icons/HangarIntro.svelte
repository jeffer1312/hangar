<script lang="ts">
  /**
   * Abertura: a marca se desenha, colapsa dentro de si mesma girando, e renasce no tamanho normal.
   * Toca UMA vez (2s) e termina parada na marca — não é spinner.
   *
   * A coreografia saiu de medir os dois Lottie que este app já carregava:
   * - `splash.json` (64x64, 60fps, 1,6s): peças que entram, **hold** parado no meio, micro-ajuste
   *   no fim. Daí vêm a duração e o assentamento (a ultrapassagem de 6% no retorno).
   * - `pensando.json`: escala indo de 156% a 77% — razão ~0,49. Daí vem o fundo do colapso, 0,44.
   *
   * Por que os arcos internos se RECOLHEM durante o colapso: `scale()` encolhe o traço junto, e com
   * os três arcos encolhendo ao mesmo tempo eles se grudavam num nó ilegível (medido em 44%).
   * Recolhendo os internos, o fundo do colapso é um arco só — e a leitura vira "some dentro de si e
   * renasce", que é o ponto.
   *
   * Por que o giro acontece com a marca PEQUENA: numa forma radialmente simétrica, girar grande
   * quase não se lê e ainda custa uma fase inteira; pequeno, ele sai de graça junto do colapso.
   */
  let { size = 64 }: { size?: number } = $props();

  const ARCOS = [
    { r: 9.1, a: 30, classe: 'externo' },
    { r: 6.35, a: 18, classe: 'meio' },
    { r: 3.6, a: 6, classe: 'interno' },
  ];

  const rad = (g: number) => (g * Math.PI) / 180;
  const ponto = (r: number, g: number) => [12 + r * Math.cos(rad(g)), 12 + r * Math.sin(rad(g))];

  const caminhos = ARCOS.map(({ r, a, classe }) => {
    const [x0, y0] = ponto(r, 180 - a);
    const [x1, y1] = ponto(r, 360 + a);
    return {
      d: `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${r} ${r} 0 1 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`,
      L: (r * rad(180 + 2 * a)).toFixed(2),
      classe,
    };
  });
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.55"
  stroke-linecap="round"
  aria-hidden="true"
>
  <g class="grupo">
    {#each caminhos as c}
      <path d={c.d} class="arco {c.classe}" style="--L: {c.L};" />
    {/each}
  </g>
</svg>

<style>
  .grupo {
    transform-box: fill-box;
    transform-origin: center;
    animation: hangar-grupo 2s var(--ease-out) forwards;
  }

  .arco {
    stroke-dasharray: var(--L);
    stroke-dashoffset: var(--L);
    animation-duration: 2s;
    animation-timing-function: var(--ease-out);
    animation-fill-mode: forwards;
  }

  .externo { animation-name: hangar-externo; }
  .meio    { animation-name: hangar-meio; }
  .interno { animation-name: hangar-interno; }

  /* colapsa a 44% girando 360, e volta passando 6% antes de assentar */
  @keyframes hangar-grupo {
    0%, 42%  { transform: scale(1) rotate(0deg); }
    62%      { transform: scale(0.44) rotate(216deg); }
    64%      { transform: scale(0.44) rotate(252deg); }
    72%      { transform: scale(0.62) rotate(360deg); }
    85%      { transform: scale(1.06) rotate(360deg); }
    100%     { transform: scale(1) rotate(360deg); }
  }

  /* o externo nunca some: é ele que encolhe e carrega o giro */
  @keyframes hangar-externo {
    0%   { stroke-dashoffset: var(--L); }
    33%, 100% { stroke-dashoffset: 0; }
  }

  /* os internos se recolhem no colapso e se redesenham na volta */
  @keyframes hangar-meio {
    0%   { stroke-dashoffset: var(--L); }
    38%  { stroke-dashoffset: 0; }
    42%  { stroke-dashoffset: 0; }
    60%, 70% { stroke-dashoffset: var(--L); }
    94%, 100% { stroke-dashoffset: 0; }
  }

  @keyframes hangar-interno {
    0%   { stroke-dashoffset: var(--L); }
    44%  { stroke-dashoffset: 0; }
    47%  { stroke-dashoffset: 0; }
    64%  { stroke-dashoffset: var(--L); }
    88%, 100% { stroke-dashoffset: 0; }
  }

  /* A regra global de prefers-reduced-motion (app.css) neutraliza animação em loop; esta toca uma
     vez só, então o corte tem que ser explícito: sem movimento, a marca aparece pronta. */
  @media (prefers-reduced-motion: reduce) {
    .grupo { animation: none; }
    .arco { animation: none; stroke-dashoffset: 0; }
  }
</style>
