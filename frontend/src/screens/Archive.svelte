<script lang="ts">
  import NavBar from '../components/NavBar.svelte';
  import MessageList from '../components/MessageList.svelte';
  import {
    getArchive, getArchiveFolder, getArchiveHistory, archiveImageUrl,
    type ArchiveFolder, type ArchiveEntry,
  } from '../lib/api';
  import type { ChatEvent } from '../lib/types';
  import { selectServer } from '../lib/auth';

  interface Props {
    onBack: () => void;
    // Deep-link vindo da busca (feature #10): abre direto uma conversa arquivada de um servidor
    // especifico, sem passar pela navegacao pasta-a-pasta.
    deepLink?: { serverId: string; project: string; sessionId: string } | null;
  }
  let { onBack, deepLink = null }: Props = $props();

  // Navegacao pasta-primeiro (3 niveis, estado interno): pastas -> conversas da pasta -> leitor.
  let loading = $state(true);
  let error = $state('');
  let folders = $state<ArchiveFolder[]>([]);
  let folder = $state<ArchiveFolder | null>(null);
  let entries = $state<ArchiveEntry[]>([]);
  let loadingEntries = $state(false);
  let selected = $state<ArchiveEntry | null>(null);
  let events = $state<ChatEvent[]>([]);
  let loadingChat = $state(false);

  async function load() {
    loading = true;
    error = '';
    try {
      folders = await getArchive();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Erro ao carregar o arquivo';
    } finally {
      loading = false;
    }
  }
  $effect(() => {
    if (deepLink) {
      // Aponta pro servidor dono ANTES de qualquer fetch (apiFetch le o ativo na hora da chamada),
      // carrega as pastas por baixo (pro "voltar" da conversa cair na lista) e abre a conversa direto.
      selectServer(deepLink.serverId);
      load();
      openConversation({
        project: deepLink.project, session_id: deepLink.sessionId,
        cwd: null, mtime: 0, preview: '', live: false,
      });
    } else {
      load();
    }
  });

  async function openFolder(f: ArchiveFolder) {
    folder = f;
    loadingEntries = true;
    entries = [];
    try {
      entries = await getArchiveFolder(f.project);
    } catch {
      error = 'Erro ao abrir a pasta';
      folder = null;
    } finally {
      loadingEntries = false;
    }
  }

  async function openConversation(e: ArchiveEntry) {
    selected = e;
    loadingChat = true;
    events = [];
    try {
      events = await getArchiveHistory(e.project, e.session_id);
    } catch {
      error = 'Erro ao abrir a conversa';
      selected = null;
    } finally {
      loadingChat = false;
    }
  }

  // Nome curto da pasta (ultimo segmento do cwd real; fallback: nome sanitizado do projeto).
  function folderName(f: ArchiveFolder): string {
    if (f.cwd) return f.cwd.split('/').filter(Boolean).pop() ?? f.cwd;
    return f.project;
  }

  function fmtDate(ts: number): string {
    return new Date(ts * 1000).toLocaleString('pt-BR', {
      day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit',
    });
  }
  const noop = () => {};
</script>

{#if selected}
  {@const sel = selected}
  <div class="archive-screen" style="--nav-h: 0px">
    <NavBar title={sel.preview || sel.session_id.slice(0, 8)} showBack={true} onBack={() => (selected = null)} />
    {#if loadingChat}
      <p class="muted">Carregando conversa…</p>
    {:else}
      <MessageList
        {events}
        stateEvent={null}
        pending={[]}
        sessionName={''}
        dockH={8}
        onSelectOption={noop}
        onCancel={noop}
        imageUrl={(id, idx) => archiveImageUrl(sel.project, sel.session_id, id, idx)}
      />
    {/if}
  </div>
{:else if folder}
  {@const f = folder}
  <div class="archive-screen">
    <NavBar title={folderName(f)} showBack={true} onBack={() => (folder = null)} />
    <div class="archive-list">
      {#if f.cwd}<div class="group-label">{f.cwd}</div>{/if}
      {#if loadingEntries}
        <p class="muted">Carregando…</p>
      {:else if entries.length === 0}
        <p class="muted">Nenhuma conversa nesta pasta.</p>
      {:else}
        {#each entries as e (e.session_id)}
          <button class="row" onclick={() => openConversation(e)}>
            <span class="row-main">
              <span class="row-preview">{e.preview || '(sem mensagens)'}</span>
              <span class="row-meta">
                {fmtDate(e.mtime)}{#if e.live}<b class="live"> · ativa</b>{/if}
              </span>
            </span>
            <span class="chev" aria-hidden="true">›</span>
          </button>
        {/each}
      {/if}
    </div>
  </div>
{:else}
  <div class="archive-screen">
    <NavBar title="Arquivo" showBack={true} onBack={onBack} />
    <div class="archive-list">
      {#if loading}
        <p class="muted">Carregando…</p>
      {:else if error}
        <p class="err">{error}</p>
      {:else if folders.length === 0}
        <p class="muted">Nenhuma conversa arquivada ainda.</p>
      {:else}
        {#each folders as f (f.project)}
          <button class="row" onclick={() => openFolder(f)}>
            <span class="row-icon" aria-hidden="true">📁</span>
            <span class="row-main">
              <span class="row-preview">{folderName(f)}</span>
              <span class="row-meta">{f.count} conversa{f.count === 1 ? '' : 's'} · {fmtDate(f.mtime)}</span>
            </span>
            <span class="chev" aria-hidden="true">›</span>
          </button>
        {/each}
      {/if}
    </div>
  </div>
{/if}

<style>
  .archive-screen {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg-base);
  }

  .archive-list {
    flex: 1;
    overflow-y: auto;
    padding: var(--space-3) var(--space-4) var(--space-8);
    max-width: 700px;
    width: 100%;
    margin: 0 auto;
  }

  .muted { color: var(--text-secondary); padding: var(--space-4); }
  .err { color: var(--error); padding: var(--space-4); }

  .group-label {
    font-size: var(--text-xs);
    font-family: var(--font-mono);
    color: var(--text-muted);
    padding: var(--space-2) var(--space-1);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    text-align: left;
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    margin-bottom: var(--space-2);
  }
  .row:active { background: var(--bg-hover); }

  .row-icon { flex-shrink: 0; }

  .row-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .row-preview {
    font-size: var(--text-sm);
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row-meta { font-size: var(--text-xs); color: var(--text-muted); }
  .live { color: var(--success); }
  .chev { color: var(--text-muted); flex-shrink: 0; }
</style>
