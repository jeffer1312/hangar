<script lang="ts">
  // O chip de estado da sessao, em UM lugar. Antes existiam SETE desenhos da mesma palavra
  // (barra lateral 10px/2·7, card do celular 11px/3·9, painel de contexto 12px 650/2·8, quadro
  // 10,5px/1·8, barra do topo 12px/2·9, canvas sem pilula, e o ponto solto em 9 arquivos): cinco
  // tamanhos de letra, quatro espacamentos e tres pesos pra dizer "pronto".
  //
  // A cor vem dos tokens --pill-* do app.css, que ja existiam e ja trocam com o tema — e que
  // ate aqui SO o BoardCard usava. Os outros seis reescreviam a paleta na mao, entao mudar a cor
  // de um estado era editar seis arquivos, e foi assim que eles foram ficando diferentes.
  //
  // `stateColors` (lib/format.ts) continua existindo pra quem precisa da cor CRUA num style
  // inline (o anel do PlanRing, a bolinha de um grafico) — aqui nao se usa.
  import { rotuloEstado } from '@hangar/core';
  import type { State } from '@hangar/core';

  interface Props {
    state: State;
    // sm = listas, quadro, canvas, abas (o caso comum). md = cabecalho de painel e barra do topo.
    size?: 'sm' | 'md';
    // Ponto sem rotulo, pra onde nao cabe texto (aba, linha estreita). Mesma cor, mesma fonte
    // de verdade — antes era um `background: stateColors[...]` copiado em nove arquivos.
    dot?: boolean;
    // Rotulo proprio (o Canvas abrevia: "exec", "voce"). Sem isto ele tinha o proprio markup.
    label?: string;
    title?: string;
  }
  let { state, size = 'sm', dot = false, label, title }: Props = $props();

  const texto = $derived(label ?? rotuloEstado(state));
</script>

{#if dot}
  <!-- aria-hidden: o ponto e redundante — quem o usa sempre tem o nome da sessao e o estado
       em texto por perto (aria-label da linha). Anunciar "ponto" nao ajuda ninguem. -->
  <span class="chip ponto {state}" aria-hidden="true" title={title ?? texto}></span>
{:else}
  <span class="chip {size} {state}" {title}>{texto}</span>
{/if}

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
    border-radius: var(--radius-full);
    font-weight: var(--fw-semibold);
    line-height: 1.45;
    white-space: nowrap;
  }
  .chip.sm { padding: 2px var(--space-2); font-size: var(--text-3xs); }
  .chip.md { padding: 3px 10px; font-size: var(--text-2xs); }

  .working        { background: var(--pill-working-bg); color: var(--pill-working-fg); }
  .idle           { background: var(--pill-idle-bg);    color: var(--pill-idle-fg); }
  .awaiting_input { background: var(--pill-input-bg);   color: var(--pill-input-fg); }
  .dead           { background: var(--pill-dead-bg);    color: var(--pill-dead-fg); }

  /* O ponto usa a cor de TEXTO do estado (o -fg), nao a de fundo: o -bg e uma tinta de 12-16%
     de alfa, que numa bolinha de 8px sobre o vidro simplesmente nao aparece. */
  .ponto {
    width: 8px;
    height: 8px;
    padding: 0;
    border-radius: 50%;
  }
  .ponto.working        { background: var(--pill-working-fg); }
  .ponto.idle           { background: var(--pill-idle-fg); }
  .ponto.awaiting_input { background: var(--pill-input-fg); }
  .ponto.dead           { background: var(--pill-dead-fg); }
</style>
