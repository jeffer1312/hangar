<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import ModalDialog from './ModalDialog.svelte';
  import { PAGE_ZOOM_MAX, PAGE_ZOOM_MIN, pageFileName, pageZoomKey, pageZoomLabel, parseHostMessage, stepPageZoom, wheelZoom,
    themedPageDocument, type PageZoomAction } from '@hangar/core';
  import { openPageLink, pageVariables, postTheme, readFullscreenMessage, withZoomBridge, type Scheme } from '../lib/pageHost';
  import ReadView from './ReadView.svelte';
  import { copyText } from '../lib/clipboard';
  import { saveFile } from '../lib/saveFile';

  let { title, doc, ownTheme, scheme, onClose, loadSource }:
    { title: string; doc: string; ownTheme: boolean; scheme: Scheme; onClose: () => void; loadSource: () => Promise<string> } = $props();

  // Fonte: a página como o agente escreveu (o servidor tira o tema e a ponte), com realce, para ler, copiar e salvar.
  let showingSource = $state(false);
  let source = $state<string | null>(null);
  let sourceFailed = $state(false);
  let copied = $state<'ok' | 'failed' | null>(null);
  let saving = $state<'busy' | 'again' | 'failed' | null>(null);
  let fileUrl: string | null = null;

  // Um pedido por vez: alternar Página e Fonte com o pedido no ar reaproveita o mesmo, e uma resposta velha não
  // sobrescreve a nova.
  let fetching: Promise<void> | null = null;
  function fetchSource(): Promise<void> {
    if (source != null) return Promise.resolve();
    fetching ??= (async () => {
      sourceFailed = false;
      try {
        source = await loadSource();
      } catch {
        sourceFailed = true;
      } finally {
        fetching = null;
      }
    })();
    return fetching;
  }

  async function copySource() {
    if (source == null) return;
    copied = (await copyText(source)) ? 'ok' : 'failed';
    setTimeout(() => (copied = null), 1500);
  }

  // Mesmo endereço a cada toque: no iOS o segundo toque abre a folha de compartilhar com o arquivo já pronto.
  // `text/plain`, não `text/html`: o blob é da origem do app, e um navegador que abrisse em vez de baixar rodaria a
  // página com o acesso dele. O nome mantém o `.html`.
  async function saveSource() {
    if (source == null) return;
    fileUrl ??= URL.createObjectURL(new Blob([source], { type: 'text/plain;charset=utf-8' }));
    saving = 'busy';
    try {
      saving = (await saveFile(fileUrl, pageFileName(title))) === 'tap-again' ? 'again' : null;
    } catch {
      saving = 'failed';
    }
  }

  onMount(() => () => { if (fileUrl) URL.revokeObjectURL(fileUrl); });

  let zoom = $state(1);
  let pending = 0;
  let frame: HTMLIFrameElement | undefined = $state();
  let stage: HTMLDivElement | undefined = $state();
  let closeButton: HTMLButtonElement | null = $state(null);
  // O documento do cartão leva o tema da carga dele: a cópia nasce no tema de agora, que pode ter mudado até abrir.
  const src = untrack(() => withZoomBridge(ownTheme ? doc : themedPageDocument(doc, scheme, pageVariables())));

  function apply(action: PageZoomAction) { zoom = stepPageZoom(zoom, action); }

  function wheel(deltaY: number) {
    const r = wheelZoom(pending, deltaY);
    pending = r.pending;
    if (r.action) apply(r.action);
  }

  function onStageWheel(e: WheelEvent) {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    wheel(e.deltaY);
  }

  function onKey(e: KeyboardEvent) {
    const action = pageZoomKey(e);
    if (!action) return;
    e.preventDefault();
    apply(action);
  }

  function onMessage(e: MessageEvent) {
    if (!frame || e.source !== frame.contentWindow) return;
    const z = readFullscreenMessage(e.data);
    if (z?.kind === 'wheel') wheel(z.deltaY);
    else if (z?.kind === 'key') apply(z.action);
    else if (z?.kind === 'escape') onClose();
    else {
      const msg = parseHostMessage(e.data);
      if (msg?.kind === 'link') openPageLink(msg.url);
    }
  }

  // Tema trocado entre abrir e o documento carregar: a mensagem do efeito chega antes da página ouvir.
  function onFrameLoad() {
    if (!ownTheme) postTheme(frame, scheme, pageVariables());
  }

  $effect(() => {
    // Tema do app mudou com o diálogo aberto: a cópia recebe as variáveis novas sem recarregar.
    const theme = scheme;
    if (!ownTheme) postTheme(frame, theme, pageVariables());
  });

  onMount(() => {
    // Ctrl+roda na sobra do palco (zoom abaixo de 100%): o ouvinte não pode ser passivo para segurar o zoom do navegador.
    stage?.addEventListener('wheel', onStageWheel, { passive: false });
    return () => stage?.removeEventListener('wheel', onStageWheel);
  });
</script>

<svelte:window onmessage={onMessage} onkeydown={onKey} />

<ModalDialog open={true} ariaLabel={m.page_fullscreen_title({ title })} {onClose} className="page-dialog" initialFocus={closeButton}>
  <div class="page-full">
    <div class="bar">
      <span class="title">{title}</span>
      <button type="button" class="btn" aria-pressed={showingSource}
        onclick={() => { showingSource = !showingSource; if (showingSource) void fetchSource(); }}>
        {showingSource ? m.page_view_page() : m.page_view_source()}</button>
      {#if source != null}
        <button type="button" class="btn" aria-label={m.page_copy_source()} title={m.page_copy_source()} onclick={copySource}>
          {copied === 'ok' ? m.toast_copiado() : copied === 'failed' ? m.toast_copiar_falhou() : m.page_copy_source()}</button>
        <button type="button" class="btn" aria-label={m.page_save_file()} title={m.page_save_file()} disabled={saving === 'busy'}
          onclick={saveSource}>
          {saving === 'busy' ? m.save_file_downloading() : saving === 'again' ? m.save_file_tap_again()
            : saving === 'failed' ? m.save_file_failed() : m.page_save_file()}</button>
      {/if}
      <div class="zoom" hidden={showingSource}>
        <button type="button" class="btn" aria-label={m.page_zoom_out()} title={m.page_zoom_out()}
          disabled={zoom <= PAGE_ZOOM_MIN} onclick={() => apply('out')}>−</button>
        <button type="button" class="btn pct" aria-label={m.page_zoom_reset()} title={m.page_zoom_reset()}
          onclick={() => apply('reset')}>{pageZoomLabel(zoom)}</button>
        <button type="button" class="btn" aria-label={m.page_zoom_in()} title={m.page_zoom_in()}
          disabled={zoom >= PAGE_ZOOM_MAX} onclick={() => apply('in')}>+</button>
      </div>
      <button type="button" class="btn" bind:this={closeButton} aria-label={m.page_close()} title={m.page_close()}
        onclick={onClose}>✕</button>
    </div>
    {#if showingSource}
      <div class="stage source">
        {#if sourceFailed}
          <p class="note" role="alert">{m.page_source_error()}
            <button type="button" class="btn" onclick={fetchSource}>{m.page_retry()}</button></p>
        {:else if source == null}
          <p class="note" role="status">{m.page_loading({ title })}</p>
        {:else}
          <div class="code"><ReadView path="page.html" text={source} /></div>
        {/if}
      </div>
    {/if}
    <!-- A página continua montada atrás da Fonte: voltar a ela não recarrega. -->
    <div class="stage" bind:this={stage} hidden={showingSource}>
      <div class="zoomed" style:zoom={zoom}>
        <!-- Mesmo isolamento do cartão: sem allow-same-origin e sem allow-popups. -->
        <iframe bind:this={frame} {title} srcdoc={src} sandbox="allow-scripts" referrerpolicy="no-referrer"
          onload={onFrameLoad} style:color-scheme={ownTheme ? 'normal' : scheme}></iframe>
      </div>
    </div>
  </div>
</ModalDialog>

<style>
  :global(.modal-dialog.page-dialog) { width: 92vw; max-width: 92vw; max-height: none; overflow: hidden; }
  .page-full { display: flex; flex-direction: column; min-width: 0; }
  .bar {
    display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border-subtle);
  }
  .title {
    flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    color: var(--text-secondary); font-size: var(--text-sm);
  }
  .zoom { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
  .btn {
    min-width: 32px; height: 32px; padding: 0 var(--space-2); display: inline-flex; align-items: center; justify-content: center;
    border: 0; border-radius: var(--radius-sm); background: transparent; color: var(--text-secondary); font: inherit;
    font-size: var(--text-sm); cursor: pointer;
  }
  .btn:hover:not(:disabled) { background: var(--bg-hover); }
  .btn:disabled { opacity: 0.4; cursor: default; }
  /* Largura fixa: o número mudando não empurra os botões. */
  .pct { width: 56px; font-variant-numeric: tabular-nums; }
  .stage {
    height: min(86dvh, calc(100dvh - 160px)); overflow: auto; overscroll-behavior: contain;
    container-type: size; background: var(--surface-inset);
  }
  /* Sobre a foto o inset é translúcido e a imagem atravessaria a página: fundo liso, como no cartão. */
  :global(html:is([data-bg="image"], [data-bg="desktop"])) .stage { background: var(--bg-base); }
  .zoomed { width: 100cqw; height: 100cqh; }
  .stage[hidden], .zoom[hidden] { display: none; }
  .source { container-type: normal; }
  .code { padding: var(--space-3); }
  .note { margin: 0; padding: var(--space-4); color: var(--text-muted); font-size: var(--text-sm); }
  iframe { display: block; width: 100%; height: 100%; border: 0; background: transparent; }
</style>
