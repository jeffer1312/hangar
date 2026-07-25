<script lang="ts">
  import ModalDialog from './ModalDialog.svelte';
  import type { AggSession } from '../lib/types';
  import {
    filterWorkspaceItems,
    workspaceSessionItems,
    type PaletteItem,
    type WorkspaceAction,
    type WorkspaceView,
  } from '../lib/workspaceCommands';

  interface Props {
    open: boolean;
    rows: AggSession[];
    view: WorkspaceView;
    actions: WorkspaceAction[];
    onClose: () => void;
    onOpenSession: (session: AggSession) => void;
  }

  let { open, rows, view, actions, onClose, onOpenSession }: Props = $props();
  let query = $state('');
  let selected = $state(0);
  let searchInput = $state<HTMLInputElement>();

  const items = $derived.by<PaletteItem[]>(() => {
    const base: PaletteItem[] = [
      ...actions.map((action) => ({
        key: `action:${action.id}`,
        kind: 'action' as const,
        action,
        title: action.title,
        detail: action.detail,
        keywords: action.keywords,
        group: action.group,
        disabled: action.disabled,
      })),
      ...workspaceSessionItems(rows),
    ];
    return filterWorkspaceItems(base, query);
  });

  $effect(() => {
    if (open) {
      query = '';
      selected = 0;
    }
  });

  $effect(() => {
    void items.length;
    if (selected >= items.length) selected = Math.max(0, items.length - 1);
  });

  function choose(item: PaletteItem | undefined) {
    if (!item || item.disabled) return;
    onClose();
    if (item.kind === 'session') onOpenSession(item.session);
    else queueMicrotask(item.action.run);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      e.stopImmediatePropagation();
      selected = items.length ? (selected + 1) % items.length : 0;
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      e.stopImmediatePropagation();
      selected = items.length ? (selected - 1 + items.length) % items.length : 0;
    } else if (e.key === 'Enter') {
      e.preventDefault();
      e.stopImmediatePropagation();
      choose(items[selected]);
    }
  }

  $effect(() => {
    if (!open) return;
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });
</script>

{#if open}
  <ModalDialog
    {open}
    ariaLabel="Busca e comandos"
    onClose={onClose}
    initialFocus={searchInput}
    className="palette"
    layer="command"
  >
      <div class="palette-search">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
             stroke-width="2" stroke-linecap="round" aria-hidden="true">
          <circle cx="11" cy="11" r="7"></circle>
          <path d="m20 20-3.2-3.2"></path>
        </svg>
        <input
          bind:this={searchInput}
          bind:value={query}
          oninput={() => (selected = 0)}
          placeholder="Ir para uma sessão ou visualização…"
          aria-label="Buscar sessão ou comando"
          aria-controls="workspace-command-results"
          aria-activedescendant={items[selected]?.key}
          autocomplete="off"
          spellcheck={false}
        />
        <kbd>Esc</kbd>
      </div>

      <div class="palette-results" id="workspace-command-results" role="listbox">
        {#if items.length}
          {#each items as item, i (item.key)}
            {#if i === 0 || items[i - 1].group !== item.group}
              <div class="result-group" role="presentation">{item.group}</div>
            {/if}
            <button
              id={item.key}
              type="button"
              role="option"
              aria-selected={i === selected}
              aria-disabled={item.disabled ? 'true' : undefined}
              class:selected={i === selected}
              class:disabled={item.disabled}
              onmouseenter={() => (selected = i)}
              onclick={() => choose(item)}
            >
              <span class="result-icon" data-kind={item.kind}>
                {#if item.kind === 'session'}
                  <span class="server-dot" style="background: {item.session.serverColor}"></span>
                {:else if item.action.id === 'view:chat'}C
                {:else if item.action.id === 'view:board'}Q
                {:else if item.action.id === 'view:canvas'}◆
                {:else}·{/if}
              </span>
              <span class="result-copy">
                <span class="result-title">{item.title}</span>
                <span class="result-detail">{item.detail}</span>
              </span>
              {#if item.kind === 'action' && item.action.id === `view:${view}`}
                <span class="current">atual</span>
              {:else if item.kind === 'session' && item.session.state === 'awaiting_input'}
                <span class="attention">aguardando</span>
              {:else if item.kind === 'action' && item.action.shortcut}
                <kbd>{item.action.shortcut}</kbd>
              {/if}
            </button>
          {/each}
        {:else}
          <p class="palette-empty">Nenhuma sessão ou comando encontrado.</p>
        {/if}
      </div>

      <footer>
        <span><kbd>↑</kbd><kbd>↓</kbd> navegar</span>
        <span><kbd>↵</kbd> abrir</span>
      </footer>
  </ModalDialog>
{/if}

<style>
  :global(.modal-dialog.palette) {
    align-self: flex-start;
    margin-top: min(14vh, 120px);
  }

  :global(.palette) {
    width: min(660px, 100%);
    max-height: min(620px, 74vh);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border: 1px solid var(--border-default);
    border-radius: var(--radius-xl, 16px);
    background: var(--bg-elevated);
    box-shadow: 0 24px 80px rgba(0, 0, 0, 0.48);
    animation: palette-in 160ms var(--ease-out);
  }

  @keyframes palette-in {
    from { opacity: 0; transform: translateY(-8px) scale(0.985); }
    to { opacity: 1; transform: none; }
  }

  :global(.palette-search) {
    min-height: 58px;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 0 var(--space-4);
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-muted);
  }

  :global(.palette-search input) {
    flex: 1;
    min-width: 0;
    height: 56px;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--text-primary);
    font: inherit;
    font-size: var(--text-base);
  }

  :global(.palette-search input::placeholder) { color: var(--text-muted); }

  kbd {
    min-width: 22px;
    padding: 1px 5px;
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    background: var(--bg-surface);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 10px;
    line-height: 17px;
    text-align: center;
  }

  :global(.palette-results) {
    overflow-y: auto;
    padding: var(--space-2);
  }

  :global(.result-group) {
    padding: var(--space-3) var(--space-3) var(--space-1);
    color: var(--text-muted);
    font-size: 10px;
    font-weight: 650;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  :global(.palette-results button) {
    width: 100%;
    min-height: 54px;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    text-align: left;
  }

  :global(.palette-results button.selected) { background: var(--bg-hover); }
  :global(.palette-results button.disabled) {
    cursor: not-allowed;
    opacity: 0.48;
  }

  .result-icon {
    width: 30px;
    height: 30px;
    flex: 0 0 30px;
    display: grid;
    place-items: center;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: var(--bg-surface);
    color: var(--text-secondary);
    font-size: var(--text-xs);
    font-weight: 700;
  }

  .server-dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    box-shadow: 0 0 0 4px color-mix(in srgb, currentColor 10%, transparent);
  }

  .result-copy {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .result-title {
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-weight: 580;
  }

  .result-detail {
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--text-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .current, .attention {
    flex-shrink: 0;
    padding: 2px 7px;
    border-radius: var(--radius-full);
    color: var(--text-muted);
    background: var(--bg-surface);
    font-size: 10px;
    font-weight: 650;
  }

  .attention {
    color: var(--warning);
    background: color-mix(in srgb, var(--warning) 12%, transparent);
  }

  .palette-empty {
    padding: var(--space-8) var(--space-4);
    color: var(--text-muted);
    text-align: center;
  }

  footer {
    min-height: 38px;
    display: flex;
    align-items: center;
    gap: var(--space-4);
    padding: 0 var(--space-4);
    border-top: 1px solid var(--border-subtle);
    color: var(--text-muted);
    font-size: var(--text-xs);
  }

  footer span { display: inline-flex; align-items: center; gap: 4px; }

  @media (prefers-reduced-motion: reduce) {
    :global(.palette) { animation: none; }
  }
</style>
