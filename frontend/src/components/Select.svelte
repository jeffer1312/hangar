<script lang="ts">
  // Substituto do <select> nativo. Existe porque a lista aberta do nativo é desenhada pelo SO, FORA
  // do DOM: o Chromium escolhe sozinho para que lado abrir e, num formulário alto dentro de modal
  // com `max-height` + `overflow: hidden` (SettingsModal), abrir pra cima estourava o limite do
  // modal e a lista era CORTADA — com 17 modelos, os de cima ficavam inalcançáveis. CSS não alcança
  // aquele popup (nem escopado nem global posiciona o que o navegador desenha).
  //
  // Aqui a lista é HTML comum, em portal pro <body> com posição calculada: cabe na viewport, escolhe
  // o lado por espaço real, e segue o tema. Bônus que o nativo não dá: filtro por digitação, que num
  // combo de 20 modelos com nomes parecidos (deepseek-v4-flash / -pro) é o que evita erro de clique.
  import { tick } from 'svelte';
  import * as m from '../paraglide/messages';
  interface Opcao {
    value: string;
    label: string;
    // Segunda linha, opcional: contexto que não cabe no label (tamanho de janela, custo, caminho).
    hint?: string;
    title?: string;
  }
  interface Props {
    value: string;
    opcoes: Opcao[];
    onchange: (v: string) => void;
    ariaLabel?: string;
    disabled?: boolean;
    // Acima disto aparece o campo de filtro. 8 é onde a lista deixa de caber num relance.
    filtroAcimaDe?: number;
    id?: string;
    class?: string;
  }
  let {
    value, opcoes, onchange, ariaLabel = undefined, disabled = false,
    filtroAcimaDe = 8, id = undefined, class: klass = '',
  }: Props = $props();

  let aberto = $state(false);
  let filtro = $state('');
  let ativo = $state(0);            // índice destacado (teclado)
  let botao = $state<HTMLButtonElement | null>(null);
  let listaEl = $state<HTMLElement | null>(null);
  let campoFiltro = $state<HTMLInputElement | null>(null);
  let pos = $state<{ top: number | null; bottom: number | null; left: number; width: number; maxH: number }>(
    { top: 0, bottom: null, left: 0, width: 0, maxH: 320 });

  // Prefixo dos ids dos itens, pro aria-activedescendant apontar o item corrente. $props.id() dá um
  // valor único por instância — duas telas com Select aberto não colidem.
  const uid = $props.id();
  const idLista = `sel-${uid}`;

  const rotuloAtual = $derived(opcoes.find((o) => o.value === value)?.label ?? value ?? '');
  const comFiltro = $derived(opcoes.length > filtroAcimaDe);
  const visiveis = $derived(
    filtro.trim()
      ? opcoes.filter((o) => [o.label, o.value, o.hint, o.title].join(' ').toLowerCase().includes(filtro.trim().toLowerCase()))
      : opcoes,
  );

  // Portal pro <body>: mesma razão documentada no AccountMenu — `backdrop-filter` em ancestral vira
  // bloco de contenção e recorta até `position: fixed`. Sem isto a lista volta a ficar presa.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }

  function medir() {
    if (!botao) return;
    const r = botao.getBoundingClientRect();
    const margem = 8;
    const gap = 4;
    const viewport = window.visualViewport;
    const topo = viewport?.offsetTop ?? 0;
    const altura = viewport?.height ?? window.innerHeight;
    const abaixo = topo + altura - r.bottom - margem - gap;
    const acima = r.top - topo - margem - gap;
    // Abre pra baixo por padrão; pra cima só quando lá caberia mais. `maxH` é o espaço REAL do lado
    // escolhido, então a lista rola dentro de si em vez de vazar (que era o bug do nativo).
    const paraCima = abaixo < 180 && acima > abaixo;
    // Em tela estreita a largura do CAMPO não basta: num formulário de duas colunas (o trio de
    // modelo/esforço do CreateSessionSheet) cada campo tem ~170px e id de modelo fica ilegível.
    // A lista aberta então toma quase a viewport, ancorada no campo mas sem vazar pelas bordas.
    const estreito = window.innerWidth < 820;
    const width = Math.max(0, Math.min(window.innerWidth - margem * 2,
      estreito ? 480 : Math.max(320, r.width)));
    const left = Math.max(margem, Math.min(r.left, window.innerWidth - width - margem));
    pos = {
      top: paraCima ? null : r.bottom + gap,
      bottom: paraCima ? window.innerHeight - r.top + gap : null,
      left,
      width,
      maxH: Math.max(0, Math.min(paraCima ? acima : abaixo, 320)),
    };
  }

  async function abrir() {
    if (disabled) return;
    filtro = '';
    ativo = Math.max(0, opcoes.findIndex((o) => o.value === value));
    medir();
    aberto = true;
    await tick();
    if (!aberto) return;
    if (comFiltro) campoFiltro?.focus();
    listaEl?.querySelector<HTMLElement>('[data-ativo="true"]')?.scrollIntoView({ block: 'nearest' });
  }

  function fechar() {
    aberto = false;
    botao?.focus();
  }

  function escolher(v: string) {
    onchange(v);
    fechar();
  }

  function teclado(e: KeyboardEvent) {
    if (!aberto) {
      if (e.key === 'Enter' || e.key === ' ' || e.key === 'ArrowDown') { e.preventDefault(); abrir(); }
      return;
    }
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); fechar(); return; }
    // Tab FECHA (o <select> nativo fecha o popup do SO antes de mover o foco; aqui não fechava).
    // Sem isto a lista ficava pairando aberta e desconectada de onde o foco foi: os itens vivem no
    // fim do <body> (portal), então o Tab saía do widget e nada mais respondia a Escape — quem
    // navega só por teclado ficava sem como fechar, dependendo de um clique fora.
    if (e.key === 'Tab') { fechar(); return; } // O Tab nativo continua a partir do gatilho, não do portal.
    if (e.key === 'Enter') {
      e.preventDefault();
      const o = visiveis[ativo];
      if (o) escolher(o.value);
      return;
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (!visiveis.length) return;
      ativo = (ativo + (e.key === 'ArrowDown' ? 1 : -1) + visiveis.length) % visiveis.length;
      requestAnimationFrame(() =>
        listaEl?.querySelector<HTMLElement>('[data-ativo="true"]')?.scrollIntoView({ block: 'nearest' }));
    }
  }

  // Reposiciona em scroll/resize: o campo pode sair de lugar com a lista aberta (o modal rola), e a
  // lista em `fixed` não acompanha sozinha. `capture: true` pega o scroll de qualquer ancestral.
  $effect(() => {
    if (!aberto) return;
    const on = () => medir();
    window.addEventListener('scroll', on, true);
    window.addEventListener('resize', on);
    window.visualViewport?.addEventListener('resize', on);
    window.visualViewport?.addEventListener('scroll', on);
    return () => {
      window.removeEventListener('scroll', on, true);
      window.removeEventListener('resize', on);
      window.visualViewport?.removeEventListener('resize', on);
      window.visualViewport?.removeEventListener('scroll', on);
    };
  });
</script>

<button
  bind:this={botao}
  {id}
  type="button"
  class="sel-campo {klass}"
  class:aberto
  {disabled}
  aria-label={ariaLabel}
  role="combobox"
  aria-haspopup="listbox"
  aria-controls={idLista}
  aria-expanded={aberto}
  aria-activedescendant={aberto && visiveis.length ? `${idLista}-${ativo}` : undefined}
  onclick={() => (aberto ? fechar() : abrir())}
  onkeydown={teclado}
>
  <span class="sel-valor">{rotuloAtual}</span>
  <span class="sel-seta" aria-hidden="true">▾</span>
</button>

{#if aberto}
  <!-- Backdrop transparente: fecha ao clicar fora sem precisar de listener global no document, que
       nas outras telas já brigou com o click-fantasma do iOS (ver BottomSheet). -->
  <div class="sel-fora" use:portal onclick={fechar} role="presentation"></div>
  <div
    class="sel-lista"
    use:portal
    style="top:{pos.top === null ? 'auto' : `${pos.top}px`}; bottom:{pos.bottom === null ? 'auto' : `${pos.bottom}px`}; left:{pos.left}px; width:{pos.width}px; max-height:{pos.maxH}px"
  >
    {#if comFiltro}
      <input
        bind:this={campoFiltro}
        class="sel-filtro"
        type="text"
        placeholder={m.select_filtrar()}
        autocapitalize="off"
        spellcheck="false"
        role="combobox"
        aria-label={ariaLabel ? m.select_filtrar_campo({ campo: ariaLabel }) : m.select_filtrar()}
        aria-autocomplete="list"
        aria-controls={idLista}
        aria-expanded="true"
        aria-activedescendant={visiveis.length ? `${idLista}-${ativo}` : undefined}
        bind:value={filtro}
        oninput={() => (ativo = 0)}
        onkeydown={teclado}
      />
    {/if}
    <div class="sel-itens" id={idLista} role="listbox" aria-label={ariaLabel} bind:this={listaEl}>
      {#each visiveis as o, i (o.value)}
        <!-- tabindex="-1" + id: padrão ARIA de listbox é UM ponto de foco (o botão/filtro) e o item
             corrente apontado por aria-activedescendant. Sem o -1 cada item virava parada própria do
             Tab, e como eles vivem no fim do <body> (portal) o Tab entrava na lista pela ordem física
             do DOM, longe do campo — os itens não têm handler de teclado, então ali nem Escape valia. -->
        <button
          type="button"
          class="sel-item"
          id="{idLista}-{i}"
          tabindex="-1"
          class:atual={o.value === value}
          data-ativo={i === ativo}
          title={o.title}
          role="option"
          aria-selected={o.value === value}
          onclick={() => escolher(o.value)}
          onmouseenter={() => (ativo = i)}
        >
          <span class="sel-item-label">{o.label}</span>
          {#if o.hint}<span class="sel-item-hint">{o.hint}</span>{/if}
        </button>
      {:else}
        <p class="sel-vazio">{m.select_nada_encontrado()}</p>
      {/each}
    </div>
  </div>
{/if}

<style>
  /* O campo fechado imita o <select> das telas (altura 40, mono, 16px pra não dar zoom no iOS) pra
     a troca não mudar o visual de nenhum formulário. */
  .sel-campo {
    display: flex; align-items: center; gap: var(--space-2);
    width: 100%; height: 40px; min-width: 0;
    background: var(--bg-base);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 16px;
    padding: 0 var(--space-3);
    text-align: left; cursor: pointer;
  }
  .sel-campo:focus-visible, .sel-campo.aberto { border-color: var(--accent); outline: none; }
  .sel-campo:disabled { opacity: 0.6; cursor: default; }
  .sel-valor { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sel-seta { color: var(--text-secondary); font-size: 11px; flex: none; }

  .sel-fora { position: fixed; inset: 0; z-index: 900; }
  .sel-lista {
    position: fixed; z-index: 901;
    display: flex; flex-direction: column;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.45);
    overflow: hidden;
  }
  .sel-filtro {
    flex: none; height: 34px; margin: var(--space-2) var(--space-2) 0;
    background: var(--bg-base);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-mono); font-size: 16px;
    padding: 0 var(--space-2); outline: none;
  }
  .sel-filtro:focus { border-color: var(--accent); }
  .sel-itens { overflow-y: auto; padding: var(--space-1); min-height: 0; }
  .sel-item {
    display: flex; align-items: baseline; gap: var(--space-2);
    min-height: 44px;
    width: 100%; background: none; border: 0;
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-mono); font-size: 14px;
    padding: 7px var(--space-2);
    text-align: left; cursor: pointer;
  }
  .sel-item[data-ativo='true'] { background: var(--bg-hover); }
  .sel-item.atual { color: var(--accent); }
  /* Label quebra linha em vez de truncar: id de modelo (muse-spark-1.2-contributor-free) não cabe
     numa linha nem com a lista larga, e cortado o usuário não distingue um modelo do irmão. */
  .sel-item-label { flex: 1; min-width: 0; overflow-wrap: anywhere; }
  .sel-item-hint { flex: 0 1 auto; max-width: 45%; overflow-wrap: anywhere; color: var(--text-secondary); font-size: 12px; text-align: right; }
  /* Alvo de toque: 7px de padding vertical dá ~30px de item, abaixo dos ~44px de dedo. */
  @media (pointer: coarse) {
    .sel-item { padding: 10px var(--space-2); }
  }
  .sel-vazio { margin: 0; padding: var(--space-3); color: var(--text-secondary); font-size: 13px; text-align: center; }
</style>
