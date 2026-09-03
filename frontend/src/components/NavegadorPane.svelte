<script lang="ts">
  // Navegador embutido na coluna da DIREITA: quando abre, o painel de contexto sai e ele ocupa o
  // lugar (mais largo, redimensionável pela divisória esquerda), e a sidebar colapsa (o Chat faz
  // as duas coisas). No shell Electron o conteúdo é um WebContentsView NATIVO (login Google
  // persistente via partição, dirigível por CDP); fora do shell cai no iframe.
  //
  // O view nativo flutua POR CIMA do DOM — não é elemento HTML. Este componente só reserva o
  // espaço no layout (o Chat recua com --recuo-dir = --cp-nav-w) e mede o retângulo pro shell
  // pintar lá. Consequência conhecida: modal/sheet que abrir na área dele fica "atrás".
  import { onMount } from 'svelte';
  import * as m from '../paraglide/messages';
  import { navegadorNativo } from '../lib/navegadorNativo';
  import { navegadorPanel, arrastarNav, salvarNav } from '../lib/navegadorPanel.svelte';

  let { onClose }: { onClose: () => void } = $props();

  const nativo = navegadorNativo();
  let ancora = $state<HTMLDivElement | null>(null);
  let endereco = $state('');    // o que está no campo
  let aberta = $state('');      // a URL efetivamente aberta ('' = nenhuma)
  let recarregos = $state(0);   // iframe: trocar a key recria o elemento (= reload)

  // O rect TEM que sair como objeto plano: getBoundingClientRect devolve DOMRect, cujas
  // propriedades são getters no prototype — o structuredClone do IPC vira {} e o view nasce
  // com bounds zerados (invisível, área preta). Medido: open chegava no main com tudo 0.
  function rectDaAncora() {
    if (!ancora) return { x: 0, y: 0, width: 0, height: 0 };
    const r = ancora.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  }

  function ir() {
    const t = endereco.trim();
    if (!t) return;
    const u = /^https?:\/\//i.test(t) ? t : `http://${t}`;
    endereco = u;
    aberta = u;
    if (nativo) nativo.open(u, rectDaAncora());
  }

  // Drag na divisória ESQUERDA (o painel cola na direita): pointer capture no handle, largura
  // clampada no store, salva no soltar. Mesma pegada do ctxPanel — sem transição de width, o
  // arrasto segue o ponteiro sem lag (a classe resizing é a trava contra transição futura).
  // O handle vive FORA do âncora, de propósito: o view nativo cobre o âncora e engoliria o
  // clique — era por isso que "depois de abrir uma página não redimensionava mais".
  function resizeStart(e: PointerEvent) {
    navegadorPanel.resizing = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function resizeMove(e: PointerEvent) {
    if (navegadorPanel.resizing) arrastarNav(e.clientX);
  }
  function resizeEnd() {
    if (!navegadorPanel.resizing) return;
    navegadorPanel.resizing = false;
    salvarNav();
  }
  // A alça saindo do DOM no meio do arrasto (trocar de sessão remonta o Chat) deixaria o flag
  // preso e a divisória redimensionaria só com o cursor por cima — mesmo motivo do ctxPanel.
  $effect(() => () => { navegadorPanel.resizing = false; });

  onMount(() => {
    if (!nativo) return;
    // Reenvia o retângulo a cada mudança de layout (resize da janela, arrasto da divisória,
    // sidebar) — o view nativo não acompanha o DOM sozinho.
    // Com overlay DOM aberto (sheet, modal, visor de mídia) o view se esconde: ele flutua POR
    // CIMA de tudo e cobriria o overlay — era o "não sai da frente da imagem". O seletor é o
    // canônico do app (Composer/DesktopShell detectam modal com o mesmo) + .bp-wrap do visor.
    // Os dois teleportam pro body, então um MutationObserver raso (childList) já cobre.
    const SELETOR_OVERLAY = '[role="dialog"]:not(.board-overlay), .bp-wrap';
    const sync = () => {
      if (document.querySelector(SELETOR_OVERLAY)) nativo.bounds({ x: 0, y: 0, width: 0, height: 0 });
      else nativo.bounds(rectDaAncora());
    };
    const ro = new ResizeObserver(sync);
    if (ancora) ro.observe(ancora);
    const mo = new MutationObserver(sync);
    mo.observe(document.body, { childList: true });
    return () => {
      ro.disconnect();
      mo.disconnect();
      nativo.close();
    };
  });
</script>

<section class="nav-panel" class:resizing={navegadorPanel.resizing} aria-label={m.ctx_navegador()}>
  <header class="nav-bar">
    <form class="nav-form" onsubmit={(e) => { e.preventDefault(); ir(); }}>
      <input
        class="nav-url"
        bind:value={endereco}
        placeholder={m.nav_url_dica()}
        aria-label={m.nav_url_dica()}
        spellcheck="false"
        autocapitalize="off"
        autocomplete="off"
      />
    </form>
    <button
      class="nav-btn"
      onclick={() => (nativo ? nativo.reload() : recarregos++)}
      disabled={!aberta}
      aria-label={m.nav_recarregar()}
      title={m.nav_recarregar()}
    >↻</button>
    <button class="nav-btn" onclick={onClose} aria-label={m.shell_fechar_painel()} title={m.shell_fechar_painel()}>×</button>
  </header>

  <div class="nav-main">
    <!-- Divisória esquerda FORA do âncora: o view nativo cobre o âncora, então um handle dentro
         da área dele morre sem clique assim que uma página abre. -->
    <div
      class="nav-resize-handle"
      role="separator"
      aria-orientation="vertical"
      aria-label={m.ctx_navegador()}
      onpointerdown={resizeStart}
      onpointermove={resizeMove}
      onpointerup={resizeEnd}
      onpointercancel={resizeEnd}
    ></div>
    {#if nativo}
      <!-- O âncora precisa existir MESMO vazio: é ele que o ResizeObserver mede pro shell. -->
      <div class="nav-body" bind:this={ancora}>
        {#if !aberta}<p class="nav-hint">{m.nav_vazio()}</p>{/if}
      </div>
    {:else if aberta}
      {#key aberta + recarregos}
        <iframe class="nav-body nav-frame" src={aberta} title={m.ctx_navegador()}></iframe>
      {/key}
    {:else}
      <div class="nav-body"><p class="nav-hint">{m.nav_vazio()}</p></div>
    {/if}
  </div>
</section>

<style>
  /* Colado na borda direita, no LUGAR do painel de contexto (que não monta com o navegador
     aberto). A largura vem do Chat (--cp-nav-w, do store navegadorPanel), que recua o conteúdo
     da mesma faixa — assim os bounds do view nativo e a área reservada nunca divergem. */
  .nav-panel {
    position: absolute;
    top: calc(var(--nav-h, 0px) + 8px);
    right: 6px;
    bottom: 8px;
    width: var(--cp-nav-w, 45vw);
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-lg);
    background: var(--glass-panel);
    overflow: hidden;
    z-index: 19;
  }
  .nav-main { flex: 1; min-height: 0; display: flex; }
  .nav-resize-handle {
    flex: 0 0 12px;
    cursor: col-resize;
    touch-action: none;
    border-radius: var(--radius-lg) 0 0 var(--radius-lg);
  }
  .nav-resize-handle:hover { background: var(--surface-raised); }
  .nav-bar {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-1) var(--space-2);
    border-bottom: 1px solid var(--border-subtle);
  }
  .nav-form { flex: 1; min-width: 0; }
  .nav-url {
    width: 100%;
    padding: 6px 10px;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: var(--surface-inset);
    color: inherit;
    font-size: var(--text-sm);
  }
  .nav-btn {
    padding: 4px 10px;
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: inherit;
    font-size: var(--text-base);
    cursor: pointer;
  }
  .nav-btn:hover:not(:disabled) { background: var(--surface-raised); }
  .nav-btn:disabled { opacity: 0.4; cursor: default; }
  .nav-body { flex: 1; min-height: 0; display: grid; place-items: center; }
  .nav-frame { border: 0; width: 100%; height: 100%; display: block; }
  .nav-hint { opacity: 0.55; font-size: var(--text-sm); padding: var(--space-4); text-align: center; }
</style>
