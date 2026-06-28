<script lang="ts">
  import { fileUrl } from '../lib/api';
  import type { FileRef } from '../lib/format';

  interface Props {
    sessionName: string;
    refs: FileRef[];
  }
  let { sessionName, refs }: Props = $props();

  // Item aberto em tela cheia (img/video/html/pdf). null = fechado.
  let open = $state<FileRef | null>(null);
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
  const visible = $derived(refs.filter((r) => !failed.has(r.path)));

  // Move o overlay pro <body> -> escapa do overflow/posicionamento do .chat-screen (mesmo truque
  // do ImageBubble), senao o fixed fica preso e some atras do composer/topbar.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }
</script>

{#if visible.length}
  <div class="atts">
    {#each visible as r (r.path)}
      {#if r.kind === 'image'}
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
  <div use:portal class="att-overlay" role="dialog" aria-modal="true" onclick={() => (open = null)}>
    {#if cur.kind === 'image'}
      <img class="full-media" src={url(cur)} alt={cur.name} />
    {:else if cur.kind === 'video'}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video class="full-media" src={url(cur)} controls autoplay playsinline onclick={(e) => e.stopPropagation()}></video>
    {:else}
      <div class="doc-modal" onclick={(e) => e.stopPropagation()}>
        <div class="doc-bar">
          <span class="doc-name">{cur.name}</span>
          <a class="doc-btn" href={url(cur)} target="_blank" rel="noopener noreferrer">↗ nova aba</a>
          <button class="doc-btn" onclick={() => (open = null)} aria-label="Fechar">✕</button>
        </div>
        <!-- html: sandbox SEM allow-same-origin -> roda isolado, nao toca no app. pdf: viewer do browser. -->
        <iframe class="doc-frame" src={url(cur)} title={cur.name}
          sandbox={cur.kind === 'html' ? 'allow-scripts allow-popups' : undefined}></iframe>
      </div>
    {/if}
  </div>
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

  /* Tela cheia (portal no body). */
  .att-overlay {
    position: fixed; inset: 0; z-index: 1000; display: flex; align-items: center; justify-content: center;
    padding: var(--space-3); background: rgba(0, 0, 0, 0.92);
    padding-top: calc(var(--space-3) + env(safe-area-inset-top));
    padding-bottom: calc(var(--space-3) + env(safe-area-inset-bottom));
  }
  .full-media { max-width: 100%; max-height: 100%; object-fit: contain; border-radius: var(--radius-md); }

  .doc-modal {
    width: 100%; max-width: 1100px; height: 100%; display: flex; flex-direction: column;
    background: var(--bg-base); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); overflow: hidden;
  }
  .doc-bar { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-3); border-bottom: 1px solid var(--border-subtle); }
  .doc-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-sm); color: var(--text-secondary); font-family: var(--font-mono); }
  .doc-btn { flex-shrink: 0; height: 32px; padding: 0 var(--space-2); display: inline-flex; align-items: center; border-radius: var(--radius-sm); color: var(--text-secondary); font-size: var(--text-sm); }
  .doc-btn:active { background: var(--bg-hover); }
  .doc-frame { flex: 1; width: 100%; border: 0; background: #fff; }
</style>
