<script lang="ts">
  import ModalDialog from './ModalDialog.svelte';
  import { fileUrl } from '../lib/api';
  import { zoomable } from '../lib/zoomable';
  import type { FileRef } from '../lib/format';

  interface Props {
    sessionName: string;
    refs: FileRef[];
  }
  let { sessionName, refs }: Props = $props();

  // Item aberto em tela cheia (img/video/html/pdf). null = fechado.
  let open = $state<FileRef | null>(null);
  let imageClose = $state<HTMLButtonElement | null>(null);
  let videoClose = $state<HTMLButtonElement | null>(null);
  // Paths que falharam ao carregar -> some o anexo.
  let failed = $state<Set<string>>(new Set());

  function url(r: FileRef): string {
    // url absoluta (midia remota) usa direto; senao monta a do backend pelo path local.
    return r.url ?? fileUrl(sessionName, r.path);
  }
  function fail(r: FileRef) {
    failed = new Set(failed).add(r.path);
  }
  function icon(kind: string): string {
    return kind === 'html' ? '🌐' : kind === 'pdf' ? '📄' : '📎';
  }
  // Falha NAO some mais (sumir calado escondia o que quebrou) nem fica quadrado preto: vira um chip
  // "nao carregou" com o nome -> visivel + debugavel. So filtramos no render (failed.has).

</script>

{#if refs.length}
  <div class="atts">
    {#each refs as r (r.path)}
      {#if failed.has(r.path)}
        <span class="att-broken" title={r.path}>⚠ {r.name} — não carregou</span>
      {:else if r.kind === 'image'}
        <button class="thumb-btn" onclick={() => (open = r)} aria-label="Ver {r.name}">
          <img class="thumb" src={url(r)} alt={r.name} loading="lazy" onerror={() => fail(r)} />
        </button>
      {:else if r.kind === 'video'}
        <button class="thumb-btn" onclick={() => (open = r)} aria-label="Tocar {r.name}">
          <!-- svelte-ignore a11y_media_has_caption -->
          <!-- #t=0.1: media fragment -> faz o browser (incl. iOS) buscar e mostrar o 1o frame no thumb -->
          <video class="thumb" src={url(r) + '#t=0.1'} preload="metadata" muted playsinline onerror={() => fail(r)}></video>
          <span class="play" aria-hidden="true">▶</span>
        </button>
      {:else if r.kind === 'audio'}
        <audio class="att-audio" src={url(r)} controls onerror={() => fail(r)}></audio>
      {:else}
        <!-- html / pdf -> chip que abre o modal -->
        <button class="att-chip" onclick={() => (open = r)}>
          <span class="att-ico" aria-hidden="true">{icon(r.kind)}</span>
          <span class="att-name">{r.name}</span>
          <span class="att-open" aria-hidden="true">abrir ›</span>
        </button>
      {/if}
    {/each}
  </div>
{/if}

{#if open}
  {@const cur = open}
  <ModalDialog
    open={true}
    ariaLabel={`Visualizar ${cur.name}`}
    onClose={() => (open = null)}
    initialFocus={cur.kind === 'video' ? videoClose : null}
    className="attachment-dialog"
  >
    {#if cur.kind === 'image'}
      <div class="media-modal">
        <button bind:this={imageClose} class="media-close" type="button" onclick={() => (open = null)} aria-label="Fechar visualização">✕</button>
        <!-- Mesmo gesto do ImageBubble: pinch / duplo-toque / arrastar. -->
        <img class="full-media" src={url(cur)} alt={cur.name} use:zoomable />
      </div>
    {:else if cur.kind === 'video'}
      <div class="media-modal">
        <button bind:this={videoClose} class="media-close" type="button" onclick={() => (open = null)} aria-label="Fechar visualização">✕</button>
        <!-- svelte-ignore a11y_media_has_caption -->
        <video class="full-media" src={url(cur)} controls autoplay playsinline></video>
      </div>
    {:else}
      <div class="doc-modal">
        <div class="doc-bar">
          <span class="doc-name">{cur.name}</span>
          <a class="doc-btn" href={url(cur)} target="_blank" rel="noopener noreferrer" aria-label={`Abrir ${cur.name} em nova aba`}>↗ nova aba</a>
          <button class="doc-btn" type="button" onclick={() => (open = null)} aria-label="Fechar visualização">✕</button>
        </div>
        <!-- html: sandbox SEM allow-same-origin -> roda isolado, nao toca no app. pdf: viewer do browser. -->
        <iframe class="doc-frame" src={url(cur)} title={cur.name}
          sandbox={cur.kind === 'html' ? 'allow-scripts allow-popups' : undefined}></iframe>
      </div>
    {/if}
  </ModalDialog>
{/if}

<style>
  /* max-width/min-width: a cadeia flex precisa poder encolher (min-width:auto default trava no
     conteudo) -> sem isto o nome longo do chip estoura a largura e gera scroll horizontal no mobile. */
  .atts { display: flex; flex-wrap: wrap; gap: var(--space-1); margin-top: var(--space-2); max-width: 100%; min-width: 0; }

  /* Miniatura pequena (igual ImageBubble): 96x96, tap abre em tela cheia. */
  .thumb-btn {
    position: relative; padding: 0; border: none; background: none; line-height: 0;
    border-radius: var(--radius-md); overflow: hidden; flex-shrink: 0;
  }
  .thumb { width: 96px; height: 96px; object-fit: cover; display: block; background: #000; }
  .play {
    position: absolute; inset: 0; display: flex; align-items: center; justify-content: center;
    color: #fff; font-size: 22px; text-shadow: 0 1px 4px rgba(0,0,0,0.6); pointer-events: none;
    background: rgba(0,0,0,0.18);
  }
  .att-audio { width: 100%; max-width: 320px; }

  .att-chip {
    display: inline-flex; align-items: center; gap: var(--space-2); max-width: 100%; min-width: 0; height: 38px;
    padding: 0 var(--space-3); background: var(--bg-elevated); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); color: var(--text-primary); font-size: var(--text-sm);
  }
  .att-chip:active { background: var(--bg-hover); }
  .att-ico { flex-shrink: 0; }
  .att-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); }
  .att-open { flex-shrink: 0; color: var(--text-muted); font-size: var(--text-xs); }

  /* Falha de carga (404/403/path errado): chip discreto no lugar do thumbnail. Visivel que quebrou
     (mostra o nome pra debug), sem virar quadrado preto nem sumir calado. */
  .att-broken {
    display: inline-flex; align-items: center; gap: var(--space-1); max-width: 100%; min-width: 0;
    height: 30px; padding: 0 var(--space-2); background: var(--bg-elevated);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    color: var(--text-muted); font-size: var(--text-xs); font-family: var(--font-mono);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }

  :global(.modal-backdrop:has(.attachment-dialog)) {
    background: rgba(0, 0, 0, 0.92);
    padding: var(--space-3);
    padding-top: calc(var(--space-3) + env(safe-area-inset-top));
    padding-bottom: calc(var(--space-3) + env(safe-area-inset-bottom));
  }
  :global(.modal-dialog.attachment-dialog) {
    width: 100%; max-width: 1100px; height: min(100%, 900px); max-height: 100%;
    overflow: hidden; background: transparent; border: 0; border-radius: 0; box-shadow: none;
  }
  .media-modal {
    position: relative; width: 100%; height: 100%; display: flex; align-items: center; justify-content: center;
  }
  .media-close {
    position: absolute; z-index: 1; top: var(--space-2); right: var(--space-2);
    width: 40px; height: 40px; display: inline-flex; align-items: center; justify-content: center;
    border: 1px solid rgba(255,255,255,0.25); border-radius: 999px; background: rgba(0,0,0,0.65);
    color: #fff; font-size: var(--text-base); cursor: pointer;
  }
  .media-close:active, .media-close:focus-visible { background: rgba(255,255,255,0.18); }
  .full-media { max-width: 100%; max-height: 100%; object-fit: contain; border-radius: var(--radius-md); }

  .doc-modal {
    width: 100%; height: 100%; display: flex; flex-direction: column;
    background: var(--bg-base); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); overflow: hidden;
  }
  .doc-bar { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-3); border-bottom: 1px solid var(--border-subtle); }
  .doc-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-sm); color: var(--text-secondary); font-family: var(--font-mono); }
  .doc-btn { flex-shrink: 0; height: 32px; padding: 0 var(--space-2); display: inline-flex; align-items: center; border-radius: var(--radius-sm); color: var(--text-secondary); font-size: var(--text-sm); }
  .doc-btn:active { background: var(--bg-hover); }
  .doc-frame { flex: 1; width: 100%; border: 0; background: #fff; }
</style>
