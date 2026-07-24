<script lang="ts">
  type WorkspaceView = 'chat' | 'board' | 'canvas';

  interface Props {
    view: WorkspaceView;
    onSelect: (view: WorkspaceView) => void;
    onOpenCommand: () => void;
  }

  let { view, onSelect, onOpenCommand }: Props = $props();

  const items: { id: WorkspaceView; label: string; short: string }[] = [
    { id: 'chat', label: 'Conversa', short: 'Chat' },
    { id: 'board', label: 'Quadro', short: 'Quadro' },
    { id: 'canvas', label: 'Canvas', short: 'Canvas' },
  ];
</script>

<div class="workspace-nav-wrap">
  <nav class="workspace-nav" aria-label="Visualização do espaço de trabalho">
    {#each items as item (item.id)}
      <button
        type="button"
        class:active={view === item.id}
        aria-current={view === item.id ? 'page' : undefined}
        onclick={() => onSelect(item.id)}
      >
        <span class="full-label">{item.label}</span>
        <span class="short-label">{item.short}</span>
      </button>
    {/each}
  </nav>

  <button
    type="button"
    class="command-button"
    onclick={onOpenCommand}
    aria-label="Abrir busca e comandos"
    title="Busca e comandos (Ctrl ou ⌘ + K)"
  >
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
         stroke-width="2" stroke-linecap="round" aria-hidden="true">
      <circle cx="11" cy="11" r="7"></circle>
      <path d="m20 20-3.2-3.2"></path>
    </svg>
    <kbd>⌘K</kbd>
  </button>
</div>

<style>
  .workspace-nav-wrap {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    pointer-events: auto;
  }

  .workspace-nav {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 3px;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-lg);
    background: color-mix(in srgb, var(--bg-elevated) 94%, transparent);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.22);
  }

  .workspace-nav button {
    min-width: 76px;
    height: 34px;
    padding: 0 var(--space-3);
    border-radius: calc(var(--radius-lg) - 3px);
    color: var(--text-secondary);
    font-family: inherit;
    font-size: var(--text-sm);
    font-weight: 560;
    transition: color 140ms var(--ease-out), background 140ms var(--ease-out);
  }

  .workspace-nav button:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .workspace-nav button.active {
    color: var(--text-primary);
    background: var(--bg-surface);
    box-shadow: inset 0 0 0 1px var(--border-default), 0 1px 4px rgba(0, 0, 0, 0.22);
  }

  .command-button {
    height: 40px;
    min-width: 72px;
    padding: 0 var(--space-2) 0 var(--space-3);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-lg);
    background: color-mix(in srgb, var(--bg-elevated) 94%, transparent);
    color: var(--text-secondary);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18);
  }

  .command-button:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  kbd {
    padding: 1px 5px;
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 10px;
    line-height: 16px;
  }

  .short-label { display: none; }

  @media (max-width: 1120px) {
    .workspace-nav button { min-width: 60px; padding-inline: var(--space-2); }
    .full-label { display: none; }
    .short-label { display: inline; }
    .command-button { min-width: 40px; width: 40px; padding: 0; }
    .command-button kbd { display: none; }
  }
</style>
