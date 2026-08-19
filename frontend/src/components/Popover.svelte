<script lang="ts">
  // Popover ancorado num elemento (a pill do composer). Extraido do Select.svelte, que ja resolvia
  // os dois problemas dificeis: portal pro <body> (backdrop-filter em ancestral vira bloco de
  // contencao e recorta ate position:fixed) e medicao do espaco real acima/abaixo, pra lista rolar
  // DENTRO de si em vez de vazar da viewport.
  import type { Snippet } from 'svelte';

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    onClose: () => void;
    /** Largura do popover; por padrao acompanha a ancora, com um minimo legivel. */
    width?: number;
    ariaLabel: string;
    children: Snippet;
  }
  let { open, anchor, onClose, width, ariaLabel, children }: Props = $props();

  let pos = $state<{ top: number | null; bottom: number | null; left: number; width: number; maxH: number; acima: boolean }>(
    { top: 0, bottom: null, left: 0, width: 280, maxH: 320, acima: false });
  let caixa = $state<HTMLElement | null>(null);

  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }

  function medir() {
    if (!anchor) return;
    const r = anchor.getBoundingClientRect();
    const margem = 8;
    const larg = Math.max(width ?? r.width, 260);
    const gap = 6;                                   // respiro entre a caixa e a pill
    const abaixo = window.innerHeight - r.bottom - margem - gap;
    const acima = r.top - margem - gap;              // espaco ATE O TOPO da pill, nao ate o fundo
    // A pill do composer vive no RODAPE da tela: abrir pra baixo quase nunca cabe. Por isso a
    // preferencia aqui e o INVERSO da do Select — pra cima, caindo pra baixo so quando la couber
    // mais. Mesma referencia do opencode, que abre a lista ACIMA da pill.
    const paraCima = acima >= Math.min(320, abaixo) || acima > abaixo;
    const maxH = Math.max(120, Math.min(paraCima ? acima : abaixo, 380));
    // Encosta a direita da ancora quando a caixa e mais larga que ela, sem sair da viewport.
    const left = Math.max(margem, Math.min(r.left, window.innerWidth - larg - margem));
    // Abrindo pra cima, quem ancora e a BASE da caixa (`bottom`), nao o topo: com `top` calculado a
    // partir do maxH, lista curta deixava a caixa flutuando longe da pill — e com `r.bottom` ela
    // descia por cima da propria pill, escondendo o que o usuario acabou de clicar.
    pos = {
      top: paraCima ? null : r.bottom + gap,
      bottom: paraCima ? window.innerHeight - r.top + gap : null,
      left,
      width: larg,
      maxH,
      acima: paraCima,
    };
  }

  // O foco NAO chega num frame so. O conteudo dos popovers vem de `await`: a busca do modelo do
  // Claude so aparece depois da lista carregar (ela nem existe com 8 itens ou menos), e os niveis do
  // Pi idem. Tentar uma vez e desistir deixava o foco na pill: o Tab seguinte ia pra pagina ATRAS do
  // portal, e quem navega por teclado nunca entrava na caixa. Entao observa ate o alvo nascer.
  $effect(() => {
    if (!open) return;
    // Ancora que sumiu com a caixa aberta (a pill de esforco some quando a sessao vira Haiku): sem
    // ela nao ha o que medir, e a caixa ficaria parada no ultimo lugar conhecido, solta na tela.
    if (!anchor) { onClose(); return; }
    medir();
    let vivo = true;
    let obs: MutationObserver | null = null;
    const focar = () => {
      if (!vivo || !caixa) return false;
      const alvo = caixa.querySelector<HTMLElement>('[data-foco]');
      if (!alvo) return false;
      // Nao rouba foco de quem ja mexeu em outra coisa nesse meio-tempo — so age enquanto o foco
      // segue onde estava quando a caixa abriu (a pill) ou em lugar nenhum.
      const at = document.activeElement;
      if (at && at !== document.body && at !== anchor && !caixa.contains(at)) { vivo = false; return true; }
      alvo.focus();
      vivo = false;
      return true;
    };
    requestAnimationFrame(() => {
      if (!vivo || focar() || !caixa) return;
      obs = new MutationObserver(() => { if (focar()) obs?.disconnect(); });
      obs.observe(caixa, { childList: true, subtree: true });
    });
    return () => { vivo = false; obs?.disconnect(); };
  });

  // Fechar devolve o foco a pill. Sem isto o elemento focado morre junto com o portal, o foco cai no
  // <body> e o Tab recomeca do topo da pagina — o usuario de teclado perde o lugar toda vez que
  // fecha a caixa. So devolve se o foco ainda estiver "solto": quem clicou noutro botao fica onde
  // clicou.
  let estavaAberto = false;
  $effect(() => {
    if (open) { estavaAberto = true; return; }
    if (!estavaAberto) return;
    estavaAberto = false;
    const at = document.activeElement;
    if (!at || at === document.body) anchor?.focus();
  });

  // Reposiciona em scroll/resize: o composer se move quando o teclado do celular abre ou a conversa
  // rola. `capture: true` pega o scroll de qualquer ancestral.
  $effect(() => {
    if (!open) return;
    const on = () => medir();
    window.addEventListener('scroll', on, true);
    window.addEventListener('resize', on);
    return () => {
      window.removeEventListener('scroll', on, true);
      window.removeEventListener('resize', on);
    };
  });

  function teclado(e: KeyboardEvent) {
    if (e.key === 'Escape') { e.preventDefault(); onClose(); }
  }
</script>

<svelte:window onkeydown={open ? teclado : undefined} />

{#if open}
  <!-- Camada de fora: fecha ao clicar em qualquer lugar. Sem foco proprio (role=presentation),
       porque quem recebe o foco e o campo de busca dentro da caixa. -->
  <div class="pop-fora" use:portal onclick={onClose} role="presentation"></div>
  <div
    class="pop"
    class:pop--acima={pos.acima}
    use:portal
    bind:this={caixa}
    role="dialog"
    aria-label={ariaLabel}
    style="{pos.top !== null ? `top:${pos.top}px;` : `bottom:${pos.bottom}px;`} left:{pos.left}px; width:{pos.width}px; max-height:{pos.maxH}px"
  >
    {@render children()}
  </div>
{/if}

<style>
  .pop-fora {
    position: fixed;
    inset: 0;
    z-index: 90;
  }

  .pop {
    position: fixed;
    z-index: 91;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    /* Mesma receita do AccountMenu, o outro menu flutuante ancorado do app: --surface-raised (que
       respeita a transparência sobre papel de parede, ao contrário do --bg-elevated cru) e sombra
       literal — o projeto não tem token de sombra.
       EXCETO o alfa: o slider Solidez foi pensado pro papel de parede, mas a caixa de escolha
       (modelo, estilo) flutua sobre TEXTO do chat — e texto sobre texto não se lê nem com 0.87
       (queixa medida no seletor de modelo, 19/08/2026). Aqui força quase opaco e borra o que
       sobrar atrás, sem tirar o veu do resto do app. */
    --cp-surface-alpha: 0.97;
    background: var(--surface-raised);
    backdrop-filter: blur(14px);
    -webkit-backdrop-filter: blur(14px);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
  }
</style>
