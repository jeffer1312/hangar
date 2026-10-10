<script lang="ts">
  import { useSessionServer } from '../lib/sessionServer';
  import * as m from '../paraglide/messages';
  import ModalDialog from './ModalDialog.svelte';
  import { fileUrl } from '@hangar/core';
  import { abrirVisor } from '../lib/visor';
  import { saveFile } from '../lib/saveFile';
  import type { FileRef } from '@hangar/core';

  interface Props {
    sessionName: string;
    refs: FileRef[];
  }
  let { sessionName, refs }: Props = $props();
  const sessionServer = useSessionServer();

  // Documento aberto em tela cheia (html/pdf). Imagem e video vao pro visor compartilhado
  // (lib/visor.ts) — iframe com sandbox nao e midia e continua aqui, no ModalDialog.
  let open = $state<FileRef | null>(null);
  // Paths que falharam ao carregar -> some o anexo.
  let failed = $state<Set<string>>(new Set());

  function url(r: FileRef, download = false): string {
    // url absoluta (midia remota) usa direto; senao monta a do backend pelo path local.
    return r.url ?? fileUrl(sessionName, r.path, download, sessionServer());
  }
  // path -> estado do salvar daquele arquivo; ausente = parado.
  let saving = $state<Record<string, 'busy' | 'again' | 'failed'>>({});

  async function save(r: FileRef) {
    saving[r.path] = 'busy';
    try {
      const result = await saveFile(url(r, true), r.name);
      if (result === 'tap-again') saving[r.path] = 'again';
      else delete saving[r.path];
    } catch (e) {
      console.error('anexo: falhou ao salvar', e);
      saving[r.path] = 'failed';
    }
  }

  function saveLabel(r: FileRef): string {
    const s = saving[r.path];
    return s === 'busy'
      ? m.save_file_downloading()
      : s === 'again'
        ? m.save_file_tap_again()
        : s === 'failed'
          ? m.save_file_failed()
          : `↓ ${m.visor_baixar()}`;
  }

  function fail(r: FileRef) {
    failed = new Set(failed).add(r.path);
  }
  function icon(kind: string): string {
    return kind === 'html' ? '🌐' : kind === 'pdf' ? '📄' : '📎';
  }
  // Falha NAO some mais (sumir calado escondia o que quebrou) nem fica quadrado preto: vira um chip
  // "nao carregou" com o nome -> visivel + debugavel. So filtramos no render (failed.has).

  // Abre no visor com TODAS as midias desta mensagem (menos as que falharam): a mensagem costuma
  // trazer varias, e antes cada uma exigia fechar e abrir de novo.
  // path -> botao da miniatura: e dele que o visor tira o tamanho natural da midia (sem isso ele
  // abre vazio) e a origem da animacao.
  const botoes: Record<string, HTMLElement | undefined> = {};

  function abrir(r: FileRef) {
    const midias = refs.filter(
      (x) => !failed.has(x.path) && (x.kind === 'image' || x.kind === 'video'),
    );
    void abrirVisor(
      midias.map((x) => ({
        url: url(x),
        nome: x.name,
        tipo: x.kind as 'image' | 'video',
        element: botoes[x.path],
      })),
      Math.max(0, midias.findIndex((x) => x.path === r.path)),
    );
  }
</script>

{#if refs.length}
  <div class="atts">
    {#each refs as r (r.path)}
      {#if failed.has(r.path)}
        <span class="att-broken" title={r.path}>⚠ {m.anexos_nao_carregou({ nome: r.name })}</span>
      {:else if r.kind === 'image'}
        <button class="thumb-btn" bind:this={botoes[r.path]} onclick={() => abrir(r)} aria-label={m.anexos_ver({ n: r.name })}>
          <img class="thumb" src={url(r)} alt={r.name} loading="lazy" onerror={() => fail(r)} />
        </button>
      {:else if r.kind === 'video'}
        <button class="thumb-btn" bind:this={botoes[r.path]} onclick={() => abrir(r)} aria-label={m.anexos_tocar({ nome: r.name })}>
          <!-- svelte-ignore a11y_media_has_caption -->
          <!-- #t=0.1: media fragment -> faz o browser (incl. iOS) buscar e mostrar o 1o frame no thumb -->
          <video class="thumb" src={url(r) + '#t=0.1'} preload="metadata" muted playsinline onerror={() => fail(r)}></video>
          <span class="play" aria-hidden="true">▶</span>
        </button>
      {:else if r.kind === 'audio'}
        <audio class="att-audio" src={url(r)} controls onerror={() => fail(r)}></audio>
      {:else if r.kind === 'document'}
        <button class="att-chip" type="button" disabled={saving[r.path] === 'busy'} onclick={() => save(r)} aria-label={m.anexos_baixar({ nome: r.name })}>
          <span class="att-ico" aria-hidden="true">📄</span>
          <span class="att-name">{r.name}</span>
          <span class="att-open" aria-live="polite">{saveLabel(r)}</span>
        </button>
      {:else}
        <div class="att-document">
          <button class="att-chip" onclick={() => (open = r)}>
            <span class="att-ico" aria-hidden="true">{icon(r.kind)}</span>
            <span class="att-name">{r.name}</span>
            <span class="att-open" aria-hidden="true">{m.paleta_abrir()} ›</span>
          </button>
          {#if r.kind === 'pdf'}
            <button class="att-download" type="button" disabled={saving[r.path] === 'busy'} onclick={() => save(r)} aria-label={m.anexos_baixar({ nome: r.name })}>{saveLabel(r)}</button>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
{/if}

{#if open}
  {@const cur = open}
  <ModalDialog
    open={true}
    ariaLabel={m.anexos_visualizar({ nome: cur.name })}
    onClose={() => (open = null)}
    className="attachment-dialog"
  >
      <div class="doc-modal">
        <div class="doc-bar">
          <span class="doc-name">{cur.name}</span>
          {#if cur.kind === 'pdf'}
            <button class="doc-btn" type="button" disabled={saving[cur.path] === 'busy'} onclick={() => save(cur)} aria-label={m.anexos_baixar({ nome: cur.name })}>{saveLabel(cur)}</button>
          {/if}
          <a class="doc-btn" href={url(cur)} target="_blank" rel="noopener noreferrer" aria-label={m.anexos_abrir_nova_aba({ nome: cur.name })}>↗ {m.anexos_nova_aba()}</a>
          <button class="doc-btn" type="button" onclick={() => (open = null)} aria-label={m.anexos_fechar_visualizacao()}>✕</button>
        </div>
        <!-- html: sandbox SEM allow-same-origin -> roda isolado, nao toca no app. pdf: viewer do browser. -->
        <iframe class="doc-frame" src={url(cur)} title={cur.name}
          sandbox={cur.kind === 'html' ? 'allow-scripts allow-popups' : undefined}></iframe>
      </div>
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
    padding: 0 var(--space-3); background: var(--surface-raised); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); color: var(--text-primary); font-size: var(--text-sm); text-decoration: none;
  }
  .att-document { display: inline-flex; align-items: center; gap: var(--space-1); max-width: 100%; min-width: 0; }
  .att-download { display: inline-flex; align-items: center; flex-shrink: 0; min-height: 38px; padding: 0 var(--space-2); color: var(--accent); font-size: var(--text-sm); }
  .att-chip:active { background: var(--bg-hover); }
  .att-ico { flex-shrink: 0; }
  .att-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); }
  .att-open { flex-shrink: 0; color: var(--text-muted); font-size: var(--text-xs); }

  /* Falha de carga (404/403/path errado): chip discreto no lugar do thumbnail. Visivel que quebrou
     (mostra o nome pra debug), sem virar quadrado preto nem sumir calado. */
  .att-broken {
    display: inline-flex; align-items: center; gap: var(--space-1); max-width: 100%; min-width: 0;
    height: 30px; padding: 0 var(--space-2); background: var(--surface-raised);
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
  .doc-modal {
    width: 100%; height: 100%; display: flex; flex-direction: column;
    background: var(--surface-inset); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); overflow: hidden;
  }
  .doc-bar { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-3); border-bottom: 1px solid var(--border-subtle); }
  .doc-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-sm); color: var(--text-secondary); font-family: var(--font-mono); }
  .doc-btn { flex-shrink: 0; height: 32px; padding: 0 var(--space-2); display: inline-flex; align-items: center; border-radius: var(--radius-sm); color: var(--text-secondary); font-size: var(--text-sm); }
  .doc-btn:active { background: var(--bg-hover); }
  .doc-frame { flex: 1; width: 100%; border: 0; background: #fff; }
</style>
