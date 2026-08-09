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
   */
  let { size = 20 }: { size?: number } = $props();

  // Alimenta o CSS via --ciclo: era constante MORTA (nada a lia) e as durações viviam escritas à
  // mão em três lugares do CSS — mexer aqui não mudava nada na tela, e quem confiasse nela erraria.
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

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.55"
  stroke-linecap="round"
  aria-hidden="true"
  style="--ciclo: {CICLO}s"
>
  <g class="respiro">
    {#each caminhos as c}
      <path
        d={c.d}
        class="arco"
        style="--L: {c.L}; --entrada: {c.atrasoEntrada}; --giro: {c.atrasoGiro};"
      />
    {/each}
  </g>
</svg>

<style>
  /* Respiro: começa depois da entrada terminar, senão a marca encolhe enquanto ainda se desenha. */
  .respiro {
    transform-box: view-box;
    transform-origin: 50% 50%;
    animation: hangar-respiro var(--ciclo) var(--ease-out) 0.9s infinite;
  }

  .arco {
    transform-box: view-box;
    transform-origin: 50% 50%;
    stroke-dasharray: var(--L);
    stroke-dashoffset: var(--L);
    /* entrada toca uma vez e FICA; o giro entra depois, com a fase própria de cada arco */
    animation:
      hangar-entra 0.72s var(--ease-out) var(--entrada) forwards,
      hangar-gira var(--ciclo) var(--ease-out) var(--giro) infinite;
  }

  @keyframes hangar-entra {
    to { stroke-dashoffset: 0; }
  }

  /* 1,06s de volta (33% de 3,2s) e segura o resto. A DEFASAGEM entre arcos é 0,48s — menos que
     metade do giro —, então quando um está terminando o seguinte já está na metade do dele: os três
     se encavalam e a leitura vira onda contínua. Com 0,65s de defasagem a sobreposição caía onde os
     dois estão lentos (ease desacelera no fim e acelera devagar no começo) e lia como "um de cada
     vez", que foi a queixa em uso. */
  @keyframes hangar-gira {
    0%        { transform: rotate(0deg); }
    33%, 100% { transform: rotate(360deg); }
  }

  /* O respiro começa em 63% (2,02s), que é EXATAMENTE quando o terceiro arco fecha a volta — sem
     batida de espera. A folga de 0,16s que havia antes parecia bem maior em uso: com a curva
     desacelerando no fim, o último arco já lê como parado antes de tecnicamente terminar, então o
     tempo morto percebido era o dobro do medido. */
  /* FINAL: depois que a onda passa por todos, os três giram JUNTOS enquanto encolhem e voltam.
     É o contraste que fecha o ciclo — a onda é sequencial, o final é uníssono. O giro do grupo se
     compõe com o dos arcos (que já estão parados em 360°), então o efeito é o conjunto inteiro
     rodando uma volta enquanto respira. */
  @keyframes hangar-respiro {
    0%, 63%   { transform: scale(1) rotate(0deg); }
    77%       { transform: scale(0.44) rotate(170deg); }
    88%       { transform: scale(1.06) rotate(300deg); }
    96%, 100% { transform: scale(1) rotate(360deg); }
  }

  @media (prefers-reduced-motion: reduce) {
    .respiro { animation: none; }
    .arco { animation: none; stroke-dashoffset: 0; }
  }
</style>
