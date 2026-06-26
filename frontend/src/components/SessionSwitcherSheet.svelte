<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import ThemeToggle from './ThemeToggle.svelte';
  import { basename, relativeTime } from '../lib/format';
  import type { SessionInfo, State } from '../lib/types';

  // Troca de sessao sem voltar pra home: busca + lista das outras sessoes vivas (a atual
  // fica marcada) + uma linha "Nova sessão".
  interface Props {
    open: boolean;
    sessions: SessionInfo[];
    currentName: string;
    onPick: (name: string) => void;
    onNew: () => void;
    onClose: () => void;
  }
  let { open, sessions, currentName, onPick, onNew, onClose }: Props = $props();

  let query = $state('');
  $effect(() => {
    if (open) query = '';
  });

  const urgency: Record<State, number> = {
    awaiting_input: 0,
    working: 1,
    idle: 2,
    dead: 3,
  };

  const stateColors: Record<State, string> = {
    working: 'var(--accent)',
    idle: 'var(--success)',
    awaiting_input: 'var(--warning)',
    dead: 'var(--error)',
  };

  // Ordena por atividade (desc) + urgencia; aplica busca por nome/cwd.
  const sorted = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return [...sessions]
      .sort((a, b) => {
        const byAct = (b.last_activity ?? 0) - (a.last_activity ?? 0);
        if (byAct !== 0) return byAct;
        return urgency[a.state] - urgency[b.state];
      })
      .filter(
        (s) => !q || s.name.toLowerCase().includes(q) || (s.cwd ?? '').toLowerCase().includes(q),
      );
  });

  function title(s: SessionInfo): string {
    return s.cwd ? basename(s.cwd) : s.name;
  }

  function tap(s: SessionInfo) {
    if (s.name === currentName) {
      onClose();
      return;
    }
    onPick(s.name);
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Trocar de sessão">
  <h2 class="sheet-title">Sessões</h2>

  <input
    type="text"
    class="search"
    bind:value={query}
    placeholder="Buscar sessão"
    autocomplete="off"
    autocorrect="off"
    autocapitalize="off"
    spellcheck={false}
    aria-label="Buscar sessão"
  />

  <div class="list">
    {#if sorted.length === 0}
      <p class="empty">Nenhuma sessão encontrada.</p>
    {:else}
      {#each sorted as s (s.name)}
        <button
          class="row"
          class:row--current={s.name === currentName}
          onclick={() => tap(s)}
        >
          <span class="dot" style="background: {stateColors[s.state]};" aria-hidden="true"></span>
          <span class="row-main">
            <span class="row-name">{title(s)}</span>
            {#if s.cwd}<span class="row-cwd">{s.cwd}</span>{/if}
          </span>
          {#if s.name === currentName}
            <span class="badge-current">atual</span>
          {:else if s.last_activity}
            <span class="row-time">{relativeTime(s.last_activity)}</span>
          {/if}
        </button>
      {/each}
    {/if}

    <button class="row row--new" onclick={onNew}>
      <span class="plus" aria-hidden="true">+</span>
      <span class="row-name row-name--new">Nova sessão</span>
    </button>
  </div>

  <div class="theme-row">
    <span class="theme-label">Tema</span>
    <ThemeToggle />
  </div>
</BottomSheet>

<style>
  .sheet-title {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-4);
  }

  .theme-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: var(--space-4);
    padding-top: var(--space-3);
    border-top: 1px solid var(--border-subtle);
  }
  .theme-label {
    font-size: var(--text-sm);
    color: var(--text-secondary);
  }

  .search {
    width: 100%;
    height: 44px;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    padding: 0 var(--space-3);
    outline: none;
    margin-bottom: var(--space-4);
    transition: border-color 180ms var(--ease-out);
  }
  .search::placeholder {
    color: var(--text-muted);
  }
  .search:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .list {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    max-height: 56vh;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .row {
    width: 100%;
    min-height: 56px;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    text-align: left;
    background: transparent;
    transition: background 160ms var(--ease-out);
  }
  .row:active {
    background: var(--bg-hover);
  }
  .row--current {
    background: var(--bg-surface);
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: var(--radius-full);
    flex-shrink: 0;
  }

  .row-main {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }

  .row-name {
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-cwd {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-time {
    flex-shrink: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
  }

  .badge-current {
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 2px 7px;
    border-radius: var(--radius-full);
    color: var(--accent);
    background: var(--accent-dim);
  }

  /* Linha "Nova sessão" */
  .row--new {
    margin-top: var(--space-1);
    border-top: 1px solid var(--border-subtle);
    border-radius: 0;
    padding-top: var(--space-3);
  }

  .plus {
    width: 8px;
    text-align: center;
    font-size: var(--text-lg);
    font-weight: 600;
    color: var(--accent);
    flex-shrink: 0;
  }

  .row-name--new {
    color: var(--accent);
  }

  .empty {
    font-size: var(--text-sm);
    color: var(--text-muted);
    text-align: center;
    padding: var(--space-4) 0;
  }
</style>
