<script lang="ts">
  /**
   * Indicador de "trabalhando". Coreografia em três camadas, cada uma resolvendo um problema:
   *
   * 1. ENTRADA (uma vez, 0,72s): a marca se desenha. É o aviso de que a sessão pegou o trabalho —
   *    sem ela o indicador surge pronto e não se distingue de um ícone parado.
   * 2. ONDA DE ROTAÇÃO (loop): cada arco dá a volta na SUA vez, defasado — o de fora começa, e o
   *    seguinte entra quando o anterior está em ~70% do giro. Girar o grupo inteiro junto era o que
   *    fazia a animação ler como spinner genérico: a marca é simétrica, então o giro sumia. Com as
   *    voltas desencontradas, em quase todo quadro as três aberturas apontam para lados diferentes.
   * 3. RESPIRO (loop): o conjunto colapsa a 44% e volta passando 6% antes de assentar — a escala do
   *    `pensando.json` (156%→77%, razão ~0,49) e o assentamento do `splash.json`.
   *
   * A marca fica COMPLETA o tempo todo depois da entrada: o que muda é rotação e tamanho, nunca o
   * traço. A primeira versão punha o desenhar/apagar em loop e, como o quadro cheio durava uma
   * fração do ciclo, num instante qualquer aparecia só um toco de arco.
   *
   * ESTRUTURA: um <svg> por arco, empilhados num grid, e toda animação de transform mora em
   * <span> — nunca no <svg> nem em <g>/<path>. Transform animado em SVG roda na thread principal
   * (style + layout + paint a cada quadro, na página inteira); em elemento HTML o compositor da
   * GPU faz sozinho. A propriedade `rotate` também não compõe, por isso espiral e giro ficam em
   * spans aninhados em vez de rotate+transform no mesmo elemento.
   */
  let { size = 20 }: { size?: number } = $props();

  const CICLO = 3.2;
  const ARCOS = [
    { r: 9.1, a: 30, atrasoEntrada: '0s', fase: 0 },
    { r: 6.35, a: 18, atrasoEntrada: '0.09s', fase: 0.48 },
    { r: 3.6, a: 6, atrasoEntrada: '0.18s', fase: 0.96 },
  ];

  const rad = (g: number) => (g * Math.PI) / 180;
  const ponto = (r: number, g: number) => [12 + r * Math.cos(rad(g)), 12 + r * Math.sin(rad(g))];

  const caminhos = ARCOS.map(({ r, a, atrasoEntrada, fase }) => {
    const [x0, y0] = ponto(r, 180 - a);
    const [x1, y1] = ponto(r, 360 + a);
    return {
      d: `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${r} ${r} 0 1 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`,
      L: (r * rad(180 + 2 * a)).toFixed(2),
      atrasoEntrada,
      // fase POSITIVA: cada arco espera a vez dele. O anterior está em ~65% do giro quando o
      // seguinte começa — sobreposição suficiente pra fluir, sem virar rotação contínua.
      atrasoGiro: `${(0.9 + fase).toFixed(2)}s`,
    };
  });
</script>

<span class="mark" style="--ciclo: {CICLO}s; --size: {size}px" aria-hidden="true">
  {#each caminhos as c}
    <span class="espiral">
      <span class="arco" style="--giro: {c.atrasoGiro}">
        <svg
          width={size}
          height={size}
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.55"
          stroke-linecap="round"
        >
          <path d={c.d} class="traco" style="--L: {c.L}; --entrada: {c.atrasoEntrada};" />
        </svg>
      </span>
    </span>
  {/each}
</span>

<style>
  /* Respiro: começa depois da entrada terminar, senão a marca encolhe enquanto ainda se desenha. */
  .mark {
    display: inline-grid;
    width: var(--size);
    height: var(--size);
    animation: hangar-respiro var(--ciclo, 3.2s) var(--ease-out) 0.9s infinite;
  }

  /* O nome do keyframes fica no CSS (nth-child), não em var() vindo do markup: o Svelte só
     reescreve o nome com escopo quando ele aparece na folha. */
  .espiral {
    grid-area: 1 / 1;
    display: inline-grid;
    animation: hangar-espiral-1 var(--ciclo, 3.2s) var(--ease-out) 0.9s infinite;
  }
  .espiral:nth-child(2) { animation-name: hangar-espiral-2; }
  .espiral:nth-child(3) { animation-name: hangar-espiral-3; }

  .arco {
    display: inline-grid;
    animation: hangar-gira var(--ciclo, 3.2s) var(--ease-out) var(--giro, 0.9s) infinite;
  }

  svg { display: block; }

  /* O `forwards` é o que segura o traço desenhado depois da entrada. */
  .traco {
    stroke-dasharray: var(--L);
    stroke-dashoffset: var(--L);
    animation: hangar-entra 0.72s var(--ease-out) var(--entrada, 0s) forwards;
  }

  @keyframes hangar-entra {
    to { stroke-dashoffset: 0; }
  }

  /* 1,06s de volta (33% de 3,2s) e segura o resto. A DEFASAGEM entre arcos é 0,48s — menos que
     metade do giro —, então quando um está terminando o seguinte já está na metade do dele: os três
     se encavalam e a leitura vira onda contínua. */
  @keyframes hangar-gira {
    0%        { transform: rotate(0deg); }
    33%, 100% { transform: rotate(360deg); }
  }

  /* O respiro começa em 63% (2,02s), que é EXATAMENTE quando o terceiro arco fecha a volta — sem
     batida de espera. Termina EM 100%, sem cauda parada: como a escala volta a 1 e as espirais
     terminam em múltiplos de 360°, o último quadro é idêntico ao primeiro e o ciclo emenda. */
  @keyframes hangar-respiro {
    0%, 63% { transform: scale(1); }
    78%     { transform: scale(0.44); }
    90%     { transform: scale(1.06); }
    100%    { transform: scale(1); }
  }

  /* Espiral do final: os arcos giram JUNTOS enquanto o conjunto respira, cada um o seu tanto.
     Um keyframes por arco (valor literal) em vez de var() dentro do keyframe. */
  @keyframes hangar-espiral-1 {
    0%, 63% { transform: rotate(0deg); }
    100%    { transform: rotate(360deg); }
  }
  @keyframes hangar-espiral-2 {
    0%, 63% { transform: rotate(0deg); }
    100%    { transform: rotate(720deg); }
  }
  @keyframes hangar-espiral-3 {
    0%, 63% { transform: rotate(0deg); }
    100%    { transform: rotate(1080deg); }
  }

  @media (prefers-reduced-motion: reduce) {
    .mark, .espiral, .arco { animation: none; }
    .traco { animation: none; stroke-dashoffset: 0; }
  }
</style>
