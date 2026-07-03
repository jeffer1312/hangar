<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { getPreview, startPreview, stopPreview } from '../lib/api';

  interface Props {
    open: boolean;
    onClose: () => void;
  }
  let { open, onClose }: Props = $props();

  let port = $state('3000');    // porta local digitada
  let url = $state('');         // url do túnel ativo ('' = nenhum -> mostra o formulário)
  let loading = $state(false);  // consultando o estado ao abrir
  let busy = $state(false);     // start/stop em andamento
  let error = $state('');

  function cleanErr(e: unknown): string {
    const m = e instanceof Error ? e.message : 'falhou';
    return m.replace(/^\d+:\s*/, '');   // tira o prefixo "403: " do status HTTP
  }

  // Ao abrir: consulta o túnel atual (pode já haver um preview no ar de antes). Fechar não mexe.
  $effect(() => {
    if (open) { error = ''; refresh(); }
  });

  async function refresh() {
    loading = true;
    try {
      const s = await getPreview();
      url = s.url ?? '';
      if (s.port) port = String(s.port);
    } catch (e) {
      error = cleanErr(e);
    } finally {
      loading = false;
    }
  }

  async function openTunnel() {
    const p = parseInt(port, 10);
    if (!(p >= 1 && p <= 65535)) { error = 'porta inválida (1–65535)'; return; }
    busy = true;
    error = '';
    try {
      url = (await startPreview(p)).url;
    } catch (e) {
      error = cleanErr(e);
    } finally {
      busy = false;
    }
  }

  async function stopTunnel() {
    busy = true;
    error = '';
    try {
      await stopPreview();
      url = '';
    } catch (e) {
      error = cleanErr(e);
    } finally {
      busy = false;
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Preview">
  <div class="pv">
    <div class="pv-head">
      <h2 class="pv-title">Preview</h2>
      <p class="pv-sub">Vê um projeto rodando nesta máquina (porta local) via túnel HTTPS da tailnet.</p>
    </div>

    <div class="pv-form">
      <input
        class="pv-port"
        type="number"
        inputmode="numeric"
        min="1"
        max="65535"
        placeholder="porta (ex: 3000)"
        bind:value={port}
        disabled={busy}
        onkeydown={(e) => { if (e.key === 'Enter') openTunnel(); }}
      />
      {#if url}
        <button class="pv-btn" disabled={busy} onclick={openTunnel}>trocar</button>
        <button class="pv-btn danger" disabled={busy} onclick={stopTunnel}>parar</button>
      {:else}
        <button class="pv-btn accent" disabled={busy || loading} onclick={openTunnel}>abrir</button>
      {/if}
    </div>

    {#if url}
      <div class="pv-bar">
        <span class="pv-url" title={url}>{url}</span>
        <a class="pv-ext" href={url} target="_blank" rel="noopener noreferrer">nova aba ↗</a>
      </div>
      <!-- trocar a src recria o iframe (não fica com o projeto antigo preso) -->
      {#key url}
        <iframe
          class="pv-frame"
          src={url}
          title="preview do projeto"
          allow="clipboard-read; clipboard-write"
        ></iframe>
      {/key}
    {:else if loading}
      <p class="pv-muted">carregando…</p>
    {:else}
      <p class="pv-muted">Nenhum preview no ar. Digite a porta do projeto e toque em <b>abrir</b>.</p>
    {/if}

    {#if error}<p class="pv-error">{error}</p>{/if}
  </div>
</BottomSheet>

<style>
  .pv { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; min-height: 0; }
  .pv-head { display: flex; flex-direction: column; gap: 2px; flex-shrink: 0; }
  .pv-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); margin: 0; }
  .pv-sub { margin: 0; font-size: var(--text-xs); color: var(--text-muted); line-height: 1.4; }

  .pv-form { display: flex; gap: var(--space-2); flex-shrink: 0; }
  .pv-port {
    flex: 1; min-width: 0; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base);
    color: var(--text-primary); font-size: var(--text-sm); font-family: var(--font-mono);
  }
  .pv-btn {
    flex-shrink: 0; padding: var(--space-2) var(--space-4); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer;
  }
  .pv-btn:disabled { opacity: 0.5; cursor: default; }
  .pv-btn.accent { background: var(--accent); color: var(--bg-base); border-color: transparent; }
  .pv-btn.danger { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }

  .pv-bar { display: flex; align-items: center; gap: var(--space-2); flex-shrink: 0; }
  .pv-url {
    flex: 1; min-width: 0; font-family: var(--font-mono); font-size: var(--text-xs);
    color: var(--text-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .pv-ext { flex-shrink: 0; font-size: var(--text-xs); color: var(--accent); text-decoration: none; }

  .pv-frame {
    width: 100%; height: 58vh; border: 1px solid var(--border-default);
    border-radius: var(--radius-md); background: #fff;
  }
  @media (min-width: 820px) { .pv-frame { height: 70vh; } }

  .pv-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .pv-error { margin: 0; font-size: var(--text-sm); color: var(--error); white-space: pre-wrap; word-break: break-word; }
</style>
