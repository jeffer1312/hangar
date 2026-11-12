<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import { baseOf, fetchPageSource, fileAuthHeader, frameHeight, pageExpandable, pageFetchState, pageUrls, parseHostMessage, reservedHeight,
    themedPageDocument, type HtmlPageRef, type PageFetchState } from '@hangar/core';
  import { useSessionServer } from '../lib/sessionServer';
  import { desktop } from '../lib/desktop.svelte';
  import { openPageLink, pageVariables, postTheme, readScheme } from '../lib/pageHost';
  import HtmlPageDialog from './HtmlPageDialog.svelte';
  import IconMaximize from './icons/IconMaximize.svelte';
  import { getRouteBaseUrl } from '../lib/auth';
  import { useFollowTail } from '../lib/followTail';

  let { page, sessionName }: { page: HtmlPageRef; sessionName: string } = $props();
  const sessionServer = useSessionServer();
  const followTail = useFollowTail();

  let status = $state<PageFetchState>('loading');
  let doc = $state('');
  let reported = $state<number | null>(null);
  let width = $state(0);
  let scheme = $state(readScheme());
  let frame: HTMLIFrameElement | undefined = $state();
  let box: HTMLDivElement | undefined = $state();
  let expanded = $state(false);

  const height = $derived(width ? frameHeight(page, width, reported) : reservedHeight(page, 728));
  // A web desktop está congelada: a tela cheia é do celular (PWA); no desktop ela mora no app nativo.
  const fullscreen = $derived(!desktop.atual && pageExpandable(page, status));

  $effect(() => {
    // Janela que virou desktop ou página que caiu fecham o diálogo de vez: voltar ao celular não o reabre.
    if (!fullscreen) expanded = false;
  });

  let seq = 0;

  function loadSource(): Promise<string> {
    const server = sessionServer();
    return fetchPageSource(pageUrls(server ? baseOf(server) : getRouteBaseUrl(), sessionName, page.id).source, fileAuthHeader(server));
  }

  async function load() {
    // Resposta de um pedido antigo (nova tentativa no meio) não sobrescreve a do mais novo.
    const mine = ++seq;
    status = 'loading';
    const server = sessionServer();
    try {
      const r = await fetch(pageUrls(server ? baseOf(server) : getRouteBaseUrl(), sessionName, page.id).raw,
        { headers: fileAuthHeader(server) });
      // O 404 da página vencida traz o código; sem ele (convidado, rota ausente) é erro.
      const code = r.status === 404 ? await r.json().then((b) => b?.detail?.code, () => null) : null;
      const next = pageFetchState(r.status, code);
      // Tema próprio: a página fica como foi desenhada, sem as cores do app.
      const html = next === 'ready' ? await r.text() : null;
      if (mine !== seq) return;
      if (html != null) doc = page.ownTheme ? html : themedPageDocument(html, scheme, pageVariables());
      status = next;
    } catch {
      if (mine === seq) status = pageFetchState('network');
    }
  }

  function onMessage(e: MessageEvent) {
    if (!frame || e.source !== frame.contentWindow) return;
    const msg = parseHostMessage(e.data);
    if (msg?.kind === 'height') reported = msg.height;
    else if (msg?.kind === 'link') openPageLink(msg.url);
  }

  $effect(() => {
    void height;
    // Só a altura dispara; o estado de rolagem da lista que a função lê não entra como dependência.
    untrack(followTail);
  });

  $effect(() => {
    // Tema do app mudou: a página recebe as variáveis novas sem recarregar.
    const theme = scheme;
    if (page.ownTheme) return;
    postTheme(frame, theme, pageVariables());
  });

  onMount(() => {
    // Site de verdade só vive no app desktop; aqui o cartão só abre o endereço.
    if (!page.url) void load();
    const ro = new ResizeObserver(([e]) => (width = e.contentRect.width));
    if (box) ro.observe(box);
    const mo = new MutationObserver(() => (scheme = readScheme()));
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    return () => { ro.disconnect(); mo.disconnect(); };
  });
</script>

<svelte:window onmessage={onMessage} />

{#if page.url}
  <div class="site">
    <span class="site-title">{page.title}</span>
    <span class="site-url">{page.url}</span>
    <button type="button" class="link" onclick={() => window.open(page.url, '_blank', 'noopener,noreferrer')}>
      {m.page_open_browser()}</button>
  </div>
{:else}
<!-- A altura reservada só vale para a página (e enquanto ela carrega); aviso de erro fica do tamanho do texto. -->
<div class="page" bind:this={box} style:height={status === 'ready' || status === 'loading' ? `${height}px` : undefined}
  aria-busy={status === 'loading'}>
  {#if status === 'ready'}
    <!-- Sem allow-same-origin: a página roda em origem opaca, longe do token e do localStorage do app.
         Sem allow-popups: link só abre pela ponte ui/open-link, com gesto do leitor. -->
    <iframe bind:this={frame} title={page.title} srcdoc={doc} sandbox="allow-scripts"
      referrerpolicy="no-referrer" style:color-scheme={page.ownTheme ? 'normal' : scheme}></iframe>
    {#if fullscreen}
      <button type="button" class="expand" aria-label={m.page_fullscreen()} title={m.page_fullscreen()}
        onclick={() => (expanded = true)}><IconMaximize size={14} /></button>
    {/if}
  {:else if status === 'loading'}
    <p class="note">{m.page_loading({ title: page.title })}</p>
  {:else if status === 'expired'}
    <p class="note">{m.page_expired()}</p>
  {:else}
    <p class="note" role="alert">{m.page_error({ title: page.title })}
      <button type="button" class="link" onclick={load}>{m.page_retry()}</button></p>
  {/if}
</div>
{#if expanded && fullscreen}
  <HtmlPageDialog title={page.title} {doc} ownTheme={page.ownTheme} {scheme} onClose={() => (expanded = false)} {loadSource} />
{/if}
{/if}

<style>
  .page { position: relative; width: 100%; margin-bottom: var(--space-1); background: transparent; }
  /* Sobre foto ou desktop a página transparente perde a letra na imagem: fundo liso, como no nativo. A altura
     reservada é a da página, então a margem soma por fora (content-box) e a largura medida já vem sem ela. */
  :global(html:is([data-bg="image"], [data-bg="desktop"])) .page {
    box-sizing: content-box; width: auto; padding: 12px; border-radius: 10px; background: var(--bg-base);
  }
  iframe { display: block; width: 100%; height: 100%; border: 0; background: transparent; }
  .site { display: flex; flex-direction: column; align-items: flex-start; gap: 2px; padding: 8px 0; margin-bottom: var(--space-1); }
  .site-title { color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .site-url { color: var(--text-muted); font-size: var(--text-xs); overflow-wrap: anywhere; }
  .site .link { margin-left: 0; }
  .note { margin: 0; padding: 12px 0; color: var(--text-muted); font-size: var(--text-sm); }
  /* Como o T3: aparece ao passar o mouse ou com foco; no toque fica sempre à vista. */
  .expand {
    position: absolute; top: 6px; right: 6px; width: 28px; height: 28px; display: inline-flex;
    align-items: center; justify-content: center; padding: 0; border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm); background: var(--surface-raised); color: var(--text-secondary);
    cursor: pointer; opacity: 0; transition: opacity 150ms var(--ease-out);
  }
  .page:hover .expand, .expand:focus-visible { opacity: 1; }
  @media (pointer: coarse) { .expand { opacity: 1; } }
  .link {
    min-height: 0; min-width: 0; margin-left: 8px; padding: 0; border: 0; background: none;
    font: inherit; color: var(--accent); cursor: pointer;
  }
</style>
