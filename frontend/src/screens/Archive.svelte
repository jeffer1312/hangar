<script lang="ts">
  import NavBar from '../components/NavBar.svelte';
  import MessageList from '../components/MessageList.svelte';
  import { getArchive, getArchiveHistory, archiveImageUrl, type ArchiveEntry } from '../lib/api';
  import type { ChatEvent } from '../lib/types';

  interface Props {
    onBack: () => void;
  }
  let { onBack }: Props = $props();

  let loading = $state(true);
  let error = $state('');
  let entries = $state<ArchiveEntry[]>([]);
  // Conversa aberta (read-only). Navegacao interna: lista <-> leitor (sem rota propria por item).
  let selected = $state<ArchiveEntry | null>(null);
  let events = $state<ChatEvent[]>([]);
  let loadingChat = $state(false);

  async function load() {
    loading = true;
    error = '';
    try {
      entries = await getArchive();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Erro ao carregar o arquivo';
    } finally {
      loading = false;
    }
  }
  $effect(() => { load(); });

  async function open(e: ArchiveEntry) {
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

  // Agrupa por cwd real (fallback: nome do dir do projeto), preservando a ordem por data.
  const groups = $derived.by(() => {
    const m = new Map<string, ArchiveEntry[]>();
    for (const e of entries) {
      const k = e.cwd ?? e.project;
      const g = m.get(k);
      if (g) g.push(e);
      else m.set(k, [e]);
    }
    return [...m.entries()];
  });

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
{:else}
  <div class="archive-screen">
    <NavBar title="Arquivo" showBack={true} onBack={onBack} />
    <div class="archive-list">
      {#if loading}
        <p class="muted">Carregando…</p>
      {:else if error}
        <p class="err">{error}</p>
      {:else if entries.length === 0}
        <p class="muted">Nenhuma conversa arquivada ainda.</p>
      {:else}
        {#each groups as [cwd, list] (cwd)}
          <div class="group">
            <div class="group-label">{cwd}</div>
            {#each list as e (e.project + e.session_id)}
              <button class="row" onclick={() => open(e)}>
                <span class="row-main">
                  <span class="row-preview">{e.preview || '(sem mensagens)'}</span>
                  <span class="row-meta">
                    {fmtDate(e.mtime)}{#if e.live}<b class="live"> · ativa</b>{/if}
                  </span>
                </span>
                <span class="chev" aria-hidden="true">›</span>
              </button>
            {/each}
          </div>
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

  .group { margin-bottom: var(--space-5); }
  .group-label {
    font-size: var(--text-xs);
    font-family: var(--font-mono);
    color: var(--text-muted);
    text-transform: none;
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
