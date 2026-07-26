<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { listUploads, uploadUrl } from '../lib/api';
  import { fileKind, fmtBytes, relativeTime } from '../lib/format';
  import { zoomable } from '../lib/zoomable';
  import type { UploadFile } from '../lib/types';

  // Galeria dos anexos JÁ enviados pra esta sessão. Até aqui, rever uma foto mandada do celular
  // significava rolar o chat inteiro atrás dela — e agora os anexos EXPIRAM (retenção configurável),
  // então o usuário precisa de um lugar onde dê pra ver o que ainda existe e por quanto tempo.
  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  let files = $state<UploadFile[]>([]);
  let loading = $state(false);
  let erro = $state<string | null>(null);
  // Imagem aberta em tela cheia (null = fechada) e gesto de zoom em curso — mesmo par do ImageBubble:
  // sem o `gesto`, soltar um arrasto fecha o visualizador no clique que o browser dispara atrás.
  let lightbox = $state<UploadFile | null>(null);
  let gesto = $state(false);

  // Recarrega A CADA abertura em vez de uma vez só: entre uma abertura e outra o usuário mandou
  // anexos novos (e o backend pode ter podado os vencidos no meio) — uma lista em cache mostraria
  // miniatura de arquivo que não existe mais.
  $effect(() => {
    if (!open) return;
    const sess = sessionName;
    let vivo = true;
    loading = true;
    erro = null;
    listUploads(sess)
      .then((r) => { if (vivo) files = r.files; })
      .catch((e) => { if (vivo) erro = e instanceof Error ? e.message : String(e); })
      .finally(() => { if (vivo) loading = false; });
    return () => { vivo = false; };
  });

  const url = (f: UploadFile) => uploadUrl(sessionName, f.filename);

  function icone(f: UploadFile): string {
    const k = fileKind(f.filename);
    return k === 'pdf' ? '📄' : k === 'html' ? '🌐' : k === 'audio' ? '🎵' : '📎';
  }

  // Prazo em texto curto. O backend pode mandar negativo (o prune só roda no próximo upload), e aí
  // o honesto é avisar que o arquivo está vencido, não arredondar pra "expira em 0 d".
  function prazo(d: number | null): { txt: string; urgente: boolean } {
    if (d === null) return { txt: 'sem expiração', urgente: false };
    if (d <= 0) return { txt: 'vencido', urgente: true };
    if (d < 1) return { txt: `expira em ${Math.max(1, Math.round(d * 24))} h`, urgente: true };
    return { txt: `expira em ${Math.round(d)} d`, urgente: d <= 3 };
  }

  // Move o overlay pro <body>: o painel do sheet rola e ganha transform no swipe, o que vira bloco
  // de contenção e prenderia um position:fixed dentro dele. Mesmo truque do ImageBubble.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Anexos da sessão">
  <div class="atts">
    <h2 class="atts-title">
      Anexos
      {#if files.length}<span class="count">{files.length}</span>{/if}
    </h2>

    {#if loading}
      <p class="atts-msg">Carregando…</p>
    {:else if erro}
      <p class="atts-msg erro">Não deu pra listar os anexos: {erro}</p>
    {:else if !files.length}
      <p class="atts-msg">Nenhum anexo nesta sessão.</p>
    {:else}
      <ul class="grid">
        {#each files as f (f.filename)}
          {@const kind = fileKind(f.filename)}
          {@const p = prazo(f.expires_in_days)}
          <li class="item">
            {#if kind === 'image'}
              <button class="tile" onclick={() => (lightbox = f)} aria-label="Ver {f.filename}">
                <img class="media" src={url(f)} alt={f.filename} loading="lazy" />
              </button>
            {:else if kind === 'video'}
              <!-- #t=0.1: media fragment -> o browser (inclusive iOS) busca o 1o frame pro thumb -->
              <a class="tile" href={url(f)} target="_blank" rel="noopener noreferrer" aria-label="Abrir {f.filename}">
                <video class="media" src={url(f) + '#t=0.1'} preload="metadata" muted playsinline></video>
                <span class="play" aria-hidden="true">▶</span>
              </a>
            {:else}
              <a class="tile chip" href={url(f)} target="_blank" rel="noopener noreferrer" aria-label="Abrir {f.filename}">
                <span class="chip-ico" aria-hidden="true">{icone(f)}</span>
              </a>
            {/if}
            <span class="nome" title={f.filename}>{f.filename}</span>
            <span class="meta">{fmtBytes(f.size)} · {relativeTime(f.mtime)}</span>
            <span class="prazo" class:urgente={p.urgente}>{p.txt}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</BottomSheet>

<!-- Escape fecha o visualizador ANTES do sheet. Precisa ser na fase de CAPTURA: o BottomSheet
     tambem escuta Escape na window e chama stopImmediatePropagation; como ele monta primeiro, o
     listener dele rodava antes e fechava a sheet INTEIRA, deixando a foto em tela cheia presa por
     cima (com Esc ja sem efeito). Captura roda antes de qualquer listener de bolha. -->
<svelte:window
  onkeydowncapture={(e) => {
    if (lightbox && e.key === 'Escape') {
      e.stopImmediatePropagation();
      lightbox = null;
    }
  }}
/>

{#if lightbox}
  {@const cur = lightbox}
  <button use:portal class="lightbox" onclick={() => { if (!gesto) lightbox = null; }} aria-label="Fechar imagem">
    <!-- Pinch / duplo-toque / arrastar vêm da action compartilhada (a mesma do chat). -->
    <img class="lightbox-img" src={url(cur)} alt={cur.filename} use:zoomable={{ onGesture: (a) => (gesto = a) }} />
  </button>
{/if}

<style>
  .atts { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; }
  .atts-title {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin: 0;
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
  }
  .count {
    font-size: 11px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    color: var(--text-secondary);
    background: var(--bg-elevated);
  }
  .atts-msg {
    font-size: var(--text-sm);
    color: var(--text-muted);
    padding: var(--space-4) 0;
    text-align: center;
  }
  .atts-msg.erro { color: var(--error); }

  /* auto-fill: no celular cabem 3 colunas, no dock desktop (mais largo) enche sozinho — sem media
     query pra manter em duas versoes. */
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(96px, 1fr));
    gap: var(--space-3);
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .item { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .tile {
    position: relative;
    display: block;
    aspect-ratio: 1;
    width: 100%;
    padding: 0;
    border: none;
    background: var(--bg-elevated);
    border-radius: var(--radius-md);
    overflow: hidden;
    line-height: 0;
    -webkit-tap-highlight-color: transparent;
  }
  .media { width: 100%; height: 100%; object-fit: cover; display: block; }
  .play {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: var(--text-lg);
    line-height: 1;
    color: #fff;
    text-shadow: 0 1px 4px rgba(0, 0, 0, 0.6);
  }
  .chip { display: flex; align-items: center; justify-content: center; }
  .chip-ico { font-size: 28px; line-height: 1; }
  .nome {
    font-size: var(--text-xs);
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta { font-size: 11px; color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .prazo { font-size: 11px; color: var(--text-muted); }
  .prazo.urgente { color: var(--warning); }

  .lightbox {
    position: fixed;
    inset: 0;
    z-index: 1000;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-4);
    padding-top: calc(var(--space-4) + env(safe-area-inset-top));
    padding-bottom: calc(var(--space-4) + env(safe-area-inset-bottom));
    background: rgba(0, 0, 0, 0.92);
    border: none;
  }
  .lightbox-img { max-width: 100%; max-height: 100%; object-fit: contain; border-radius: var(--radius-md); }
</style>
