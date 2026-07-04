<script lang="ts">
  import NavBar from '../components/NavBar.svelte';
  import MessageList from '../components/MessageList.svelte';
  import {
    getArchive, getArchiveFolder, getArchiveHistory, archiveImageUrl, resumeArchivedConversation,
    type ArchiveFolder, type ArchiveEntry,
  } from '../lib/api';
  import type { ChatEvent } from '../lib/types';
  import { selectServer, listServers, getActiveId, serverColor } from '../lib/auth';

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
  let resuming = $state(false);
  let resumeError = $state('');

  // Servidor DE ONDE navegar o arquivo: apiFetch usa o servidor ATIVO, entao sem um seletor o arquivo
  // so mostrava o servidor ativo e nao dava pra saber/escolher de qual servidor abrir (multi-servidor).
  const servers = listServers();
  let activeServerId = $state(getActiveId());
  function pickServer(id: string) {
    if (id === activeServerId) return;
    selectServer(id);
    activeServerId = id;
    folder = null;      // volta pro nivel de pastas do servidor novo
    selected = null;
    load();
  }

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
      activeServerId = deepLink.serverId;   // mantem o seletor coerente ao voltar da conversa
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
    resumeError = '';
    try {
      events = await getArchiveHistory(e.project, e.session_id);
    } catch {
      error = 'Erro ao abrir a conversa';
      selected = null;
    } finally {
      loadingChat = false;
    }
  }

  // "Retomar conversa": sobe uma sessao tmux nova (claude --resume <uuid>) no cwd original e navega
  // pro chat dela. Serverid do deep-link (se veio da busca) e reaplicado ANTES de trocar de tela, pra
  // o chat abrir no servidor DONO da conversa (mesma convencao de openCompareSession no App.svelte).
  async function resumeConversation() {
    if (!selected) return;
    resuming = true;
    resumeError = '';
    try {
      const info = await resumeArchivedConversation(selected.project, selected.session_id);
      if (deepLink) selectServer(deepLink.serverId);
      window.location.hash = '#/chat/' + encodeURIComponent(info.name);
    } catch (e) {
      resumeError = e instanceof Error ? e.message : 'Erro ao retomar a conversa';
    } finally {
      resuming = false;
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
    <div class="resume-bar">
      <button class="resume-btn" onclick={resumeConversation} disabled={resuming}>
        {resuming ? 'Retomando…' : 'Retomar conversa'}
      </button>
      {#if resumeError}<p class="resume-err">{resumeError}</p>{/if}
    </div>
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
    {#if servers.length >= 2}
      <!-- Seletor: de qual servidor navegar o arquivo. So aparece com 2+ servidores. -->
      <div class="srv-picker" role="tablist" aria-label="Servidor do arquivo">
        {#each servers as s (s.id)}
          <button
            class="srv-pill" class:on={s.id === activeServerId}
            role="tab" aria-selected={s.id === activeServerId}
            onclick={() => pickServer(s.id)}
          >
            <span class="srv-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
            {s.label}
          </button>
        {/each}
      </div>
    {/if}
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

  /* Seletor de servidor (multi-servidor): de qual servidor navegar o arquivo. */
  .srv-picker {
    display: flex;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-4) 0;
    max-width: 700px;
    width: 100%;
    margin: 0 auto;
    overflow-x: auto;
  }
  .srv-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
    padding: 5px 12px;
    border-radius: var(--radius-full);
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--text-secondary);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    transition: color 160ms var(--ease-out), background 160ms var(--ease-out), border-color 160ms var(--ease-out);
  }
  .srv-pill.on {
    color: var(--text-primary);
    background: var(--bg-hover);
    border-color: var(--border-default);
  }
  .srv-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }

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

  .resume-bar {
    padding: var(--space-3) var(--space-4) 0;
    max-width: 700px;
    width: 100%;
    margin: 0 auto;
  }
  .resume-btn {
    width: 100%;
    height: 44px;
    background: var(--accent-dim);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-weight: 600;
    transition: background 180ms var(--ease-out);
  }
  .resume-btn:hover:not(:disabled) { background: var(--accent); color: #fff; }
  .resume-btn:disabled { opacity: 0.6; cursor: default; }
  .resume-err { color: var(--error); font-size: var(--text-xs); margin: var(--space-2) 0 0; }
</style>
