<script lang="ts">
  // Navegador embutido — ABA da coluna da direita (DesktopSessionContext). No shell Electron o
  // conteúdo é um WebContentsView NATIVO (partição persist:nav, dirigível por CDP); fora do shell
  // cai no iframe.
  //
  // O view nativo flutua POR CIMA do DOM — não é elemento HTML. Este componente só mede o
  // retângulo do âncora e manda pro shell pintar lá. Trocar de aba/sessão DESMONTA este painel →
  // nav-hide: o view fica vivo escondido e o agente segue dirigindo via CDP; fechar de verdade é
  // o ×. E com overlay DOM aberto (sheet/modal/visor) o view se esconde (bounds zero) pra não
  // cobrir nada.
  import { onMount, untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import { navegadorNativo } from '../lib/navegadorNativo';
  import { navegadorPanel, atualizarNavUrl, fecharNav } from '../lib/navegadorPanel.svelte';
  import { ctxPanel } from '../lib/ctxPanel.svelte';
  import { sidebarPin } from '../lib/sidebarPin.svelte';

  // navKey = workspaceSessionKey da sessão dona deste painel.
  let { navKey }: { navKey: string } = $props();

  const nativo = navegadorNativo();
  // Overlays DOM (sheet/modal/visor de mídia) abrem POR BAIXO do view nativo — com um aberto, o
  // view se esconde (bounds zero). O seletor é o canônico do app (Composer/DesktopShell usam o
  // mesmo) + .bp-wrap do visor; os dois teleportam pro body.
  const SELETOR_OVERLAY = '[role="dialog"]:not(.board-overlay), .bp-wrap';
  let ancora = $state<HTMLDivElement | null>(null);
  // O painel é remontado por sessão (o Chat tem key por sessão), então o valor INICIAL do navKey
  // é o certo aqui — untrack declara isso sem warning.
  const urlInicial = untrack(() => navegadorPanel.abertos[navKey] || '');
  let endereco = $state(urlInicial);   // o que está no campo
  let aberta = $state(urlInicial);     // a URL efetivamente aberta
  let recarregos = $state(0);   // iframe: trocar a key recria o elemento (= reload)

  // O rect TEM que sair como objeto plano: getBoundingClientRect devolve DOMRect, cujas
  // propriedades são getters no prototype — o structuredClone do IPC vira {} e o view nasce
  // com bounds zerados (invisível, área preta). Medido: open chegava no main com tudo 0.
  function rectDaAncora() {
    if (!ancora) return { x: 0, y: 0, width: 0, height: 0 };
    const r = ancora.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  }

  // Front mais novo que o shell Electron: o preload expõe `hangar.nav`, mas o main não registrou o
  // handler IPC, e `invoke` rejeita com "No handler registered for 'hangar:nav-open'". Sem isto o
  // painel ficava em branco, calado (10 no diário de uma semana).
  let shellVelho = $state(false);
  function abrirNativo(url: string | undefined): Promise<{ ok: boolean }> {
    if (!nativo) return Promise.resolve({ ok: false });
    return nativo.open(navKey, url, rectDaAncora()).catch(() => {
      shellVelho = true;
      return { ok: false };
    });
  }

  function ir() {
    const t = endereco.trim();
    if (!t) return;
    const u = /^https?:\/\//i.test(t) ? t : `http://${t}`;
    endereco = u;
    aberta = u;
    atualizarNavUrl(navKey, u);
    void abrirNativo(u);
    void importarSeNovo(u);
  }

  // Login trazido do Chrome real (CDP). Automático por host, uma vez por carga da página; com o
  // Chrome fechado, para até o próximo clique manual — senão cada URL nova viraria um erro igual.
  let cookiesStatus = $state('');
  const hostsImportados = new Set<string>();
  let chromeFechado = false;
  const hostLocal = (h: string) =>
    /^(localhost|127\.|10\.|192\.168\.)/.test(h) || /\.(ts\.net|local)$/.test(h) || !h.includes('.');
  function hostDe(u: string): string {
    try { return new URL(u).hostname; } catch { return ''; }
  }
  async function importarCookies(u: string, manual: boolean) {
    const host = hostDe(u);
    if (!nativo?.importCookies || !host) return;
    if (!manual && (hostLocal(host) || hostsImportados.has(host) || chromeFechado)) return;
    hostsImportados.add(host);
    if (manual) cookiesStatus = m.nav_cookies_buscando();
    // O invoke rejeita quando o shell é mais velho que este front (sem o handler) — sem o catch
    // o status ficava em "Buscando…" pra sempre.
    // Reload do view só no clique: o automático roda em cima de uma navegação que pode ser do
    // agente, no meio de um formulário.
    const r = await nativo.importCookies(navKey, host, undefined, manual).catch((e: unknown) =>
      ({ ok: false as const, gravados: 0, falhos: 0, erro: 'shell', detalhe: e instanceof Error ? e.message : String(e) }));
    if (r.ok) {
      chromeFechado = false;
      if (manual || r.gravados > 0) {
        cookiesStatus = r.falhos > 0
          ? m.nav_cookies_ok_parcial({ n: r.gravados, f: r.falhos })
          : m.nav_cookies_ok({ n: r.gravados });
      }
      return;
    }
    hostsImportados.delete(host);   // qualquer falha volta a tentar depois (a causa pode ser passageira)
    if (r.erro === 'chrome_fechado') {
      chromeFechado = true;
      if (manual) { cookiesStatus = m.nav_cookies_chrome_fechado(); ofereceAtivar = !!nativo.ativarChrome; }
      return;
    }
    if (manual) cookiesStatus = m.nav_cookies_erro({ e: r.detalhe ?? r.erro ?? '' });
  }
  const importarSeNovo = (u: string) => importarCookies(u, false);

  // O Chrome atual não aceita porta por flag no perfil padrão: quem liga a depuração é a pessoa,
  // no toggle de chrome://inspect/#remote-debugging. O botão abre essa página no Chrome dela; ao
  // voltar e clicar no 🍪 de novo, a importação acha o `DevToolsActivePort` que o toggle gravou.
  let ofereceAtivar = $state(false);
  let abrindo = $state(false);
  async function ativarChrome() {
    if (!nativo?.ativarChrome) return;
    abrindo = true;
    try {
      const r = await nativo.ativarChrome();
      if (r.ok) { cookiesStatus = m.nav_cookies_ativar_instrucao(); ofereceAtivar = false; }
      else { cookiesStatus = m.nav_cookies_sem_chrome(); ofereceAtivar = false; }
    } catch (e) {
      cookiesStatus = m.nav_cookies_erro({ e: e instanceof Error ? e.message : String(e) });
    } finally {
      abrindo = false;
    }
  }

  // URL empurrada de fora (o agente, via `hangar-preview open`): o store é a via de entrada; se
  // ela difere da aberta, navega. O ir() do usuário escreve no store junto, então não há loop.
  $effect(() => {
    const externa = navegadorPanel.abertos[navKey];
    if (externa && externa !== aberta) {
      endereco = externa;
      aberta = externa;
      void abrirNativo(externa);
      void importarSeNovo(externa);
    }
  });

  // O ResizeObserver só dispara em mudança de TAMANHO — mas a coluna também se MOVE (a sidebar
  // anima `width 160ms` ao colapsar quando a aba abre, e o painel de contexto anima ao
  // recolher): o âncora desloca em x com o tamanho intacto e o view ficaria desalinhado. Quando
  // um dos dois muda, re-sincroniza DEPOIS da animação (200ms cobre os 160ms com folga).
  $effect(() => {
    void sidebarPin.collapsed;
    void ctxPanel.recolhido;
    if (!nativo) return;
    const t = setTimeout(() => {
      if (document.querySelector(SELETOR_OVERLAY)) nativo.bounds(navKey, { x: 0, y: 0, width: 0, height: 0 });
      else nativo.bounds(navKey, rectDaAncora());
    }, 200);
    return () => clearTimeout(t);
  });

  onMount(() => {
    if (!nativo) return;
    // Reenvia o retângulo a cada mudança de layout (resize da janela, drag da coluna, sidebar) —
    // o view nativo não acompanha o DOM sozinho.
    const sync = () => {
      if (document.querySelector(SELETOR_OVERLAY)) nativo.bounds(navKey, { x: 0, y: 0, width: 0, height: 0 });
      else nativo.bounds(navKey, rectDaAncora());
    };
    const ro = new ResizeObserver(sync);
    if (ancora) ro.observe(ancora);
    const mo = new MutationObserver(sync);
    mo.observe(document.body, { childList: true });
    // Reexibe o view DESTA sessão (sem url: não recarrega — o view pode ter navegado por cliques).
    // Se o main não o tem mais (shell reiniciou), ok:false e o front recria com a url salva.
    void abrirNativo(undefined).then((r) => {
      const salva = navegadorPanel.abertos[navKey];
      if (!r?.ok && salva && !shellVelho) void abrirNativo(salva);
    });
    return () => {
      ro.disconnect();
      mo.disconnect();
      nativo.hide(navKey);   // desmontou (troca de aba/sessão): ESCONDE, não fecha — o × é o close
    };
  });
</script>

<div class="nav-pane">
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
      onclick={() => (nativo ? nativo.reload(navKey) : recarregos++)}
      disabled={!aberta}
      aria-label={m.nav_recarregar()}
      title={m.nav_recarregar()}
    >↻</button>
    {#if nativo?.importCookies}
      <button
        class="nav-btn"
        onclick={() => void importarCookies(aberta, true)}
        disabled={!aberta}
        aria-label={m.nav_cookies_trazer()}
        title={m.nav_cookies_trazer()}
      >🍪</button>
    {/if}
    <!-- O × fecha o navegador DE VERDADE (mata o view) e volta pra aba Contexto — esconder sem
         fechar é trocar de aba. -->
    <button
      class="nav-btn"
      onclick={() => { fecharNav(navKey); nativo?.close(navKey); ctxPanel.aba = 'contexto'; }}
      aria-label={m.shell_fechar_painel()}
      title={m.shell_fechar_painel()}
    >×</button>
  </header>
  {#if cookiesStatus}
    <p class="nav-status" role="status">
      {cookiesStatus}
      {#if ofereceAtivar}
        <button type="button" class="nav-abrir-chrome" onclick={ativarChrome} disabled={abrindo}>{m.nav_cookies_ativar_chrome()}</button>
      {/if}
    </p>
  {/if}

  {#if nativo}
    <!-- O âncora precisa existir MESMO vazio: é ele que o ResizeObserver mede pro shell. -->
    <div class="nav-body" bind:this={ancora}>
      {#if shellVelho}<p class="nav-hint">{m.nav_shell_velho()}</p>
      {:else if !aberta}<p class="nav-hint">{m.nav_vazio()}</p>{/if}
    </div>
  {:else if aberta}
    {#key aberta + recarregos}
      <!-- Fallback fora do shell (PWA/navegador): sandbox corta popup/download, mantém o uso. -->
      <iframe class="nav-body nav-frame" src={aberta} title={m.ctx_navegador()}
              sandbox="allow-scripts allow-forms allow-same-origin"></iframe>
    {/key}
  {:else}
    <div class="nav-body"><p class="nav-hint">{m.nav_vazio()}</p></div>
  {/if}
</div>

<style>
  /* Conteúdo de coluna: quem manda na largura é o painel de contexto (a aba Navegador usa a
     largura do navegadorPanel). Nada de position aqui — o view nativo segue o âncora medido. */
  .nav-pane {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
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
  .nav-status { margin: 0; padding: 2px var(--space-3) 4px; font-size: var(--text-xs); color: var(--text-muted); display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .nav-abrir-chrome {
    font-size: var(--text-xs); font-weight: 600; padding: 2px 10px; border-radius: var(--radius-full);
    border: 1px solid var(--accent); background: var(--accent-dim); color: var(--accent); cursor: pointer;
  }
  .nav-abrir-chrome:disabled { opacity: 0.6; cursor: default; }
  .nav-body { flex: 1; min-height: 0; display: grid; place-items: center; }
  .nav-frame { border: 0; width: 100%; height: 100%; display: block; }
  .nav-hint { opacity: 0.55; font-size: var(--text-sm); padding: var(--space-4); text-align: center; }
</style>
