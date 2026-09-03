<script lang="ts">
  // Navegador embutido ao lado do chat. No shell Electron o conteúdo é um WebContentsView NATIVO
  // (login Google persistente via partição, dirigível por CDP); fora do shell cai no iframe.
  //
  // O view nativo flutua POR CIMA do DOM — não é elemento HTML. Este componente só reserva o
  // espaço no layout (o Chat recua com --recuo-dir) e mede o retângulo pro shell pintar lá.
  // Consequência conhecida: modal/sheet que abrir na área dele fica "atrás" — fechar o painel
  // é a saída.
  import { onMount } from 'svelte';
  import * as m from '../paraglide/messages';
  import { navegadorNativo } from '../lib/navegadorNativo';

  let { onClose }: { onClose: () => void } = $props();

  const nativo = navegadorNativo();
  let ancora = $state<HTMLDivElement | null>(null);
  let endereco = $state('');    // o que está no campo
  let aberta = $state('');      // a URL efetivamente aberta ('' = nenhuma)
  let recarregos = $state(0);   // iframe: trocar a key recria o elemento (= reload)

  function ir() {
    const t = endereco.trim();
    if (!t) return;
    const u = /^https?:\/\//i.test(t) ? t : `http://${t}`;
    endereco = u;
    aberta = u;
    if (nativo && ancora) nativo.open(u, ancora.getBoundingClientRect());
  }

  onMount(() => {
    if (!nativo) return;
    // Reenvia o retângulo a cada mudança de layout (resize da janela, sidebar, painel de
    // contexto) — o view nativo não acompanha o DOM sozinho.
    const sync = () => { if (ancora) nativo.bounds(ancora.getBoundingClientRect()); };
    const ro = new ResizeObserver(sync);
    if (ancora) ro.observe(ancora);
    return () => {
      ro.disconnect();
      nativo.close();
    };
  });
</script>

<section class="nav-panel" aria-label={m.ctx_navegador()}>
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
</section>

<style>
  /* À esquerda do painel de contexto (a faixa --ctx-w é dele), abaixo da navbar. A largura vem
     do Chat (--cp-nav-w), que também faz o conteúdo recuar a mesma faixa — assim os bounds do
     view nativo nunca cobrem outro painel. */
  .nav-panel {
    position: absolute;
    top: calc(var(--nav-h, 0px) + 8px);
    right: calc(var(--ctx-w, 0px) + 6px);
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
