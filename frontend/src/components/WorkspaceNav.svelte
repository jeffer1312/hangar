<script lang="ts">
  type WorkspaceView = 'chat' | 'board' | 'canvas';

  interface Props {
    view: WorkspaceView;
    onSelect: (view: WorkspaceView) => void;
    onOpenCommand: () => void;
  }

  let { view, onSelect, onOpenCommand }: Props = $props();

  // Rotulos CURTOS de proposito: a coluna tem 248px por padrao, e "Conversa" + "Quadro" + "Canvas"
  // + o botao de busca nao cabem sem cortar palavra no meio. O nome longo vive no title.
  const items: { id: WorkspaceView; label: string; title: string }[] = [
    { id: 'chat', label: 'Chat', title: 'Conversa' },
    { id: 'board', label: 'Quadro', title: 'Quadro' },
    { id: 'canvas', label: 'Canvas', title: 'Canvas' },
  ];
</script>

<div class="workspace-nav-wrap">
  <nav class="workspace-nav" aria-label="Visualização do espaço de trabalho">
    {#each items as item (item.id)}
      <button
        type="button"
        class:active={view === item.id}
        aria-current={view === item.id ? 'page' : undefined}
        title={item.title}
        aria-label={item.title}
        onclick={() => onSelect(item.id)}
      >{item.label}</button>
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
  </button>
</div>

<style>
  .workspace-nav-wrap {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    min-width: 0;
  }

  .workspace-nav {
    display: flex;
    flex: 1;
    min-width: 0;
    align-items: center;
    gap: 2px;
    padding: 3px;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-lg);
    /* `--surface-raised` em vez de uma alfa fixa de 94%: assim o segmentado entra no veu do papel de
       parede junto com o resto (CLAUDE.md, "Transparencia") e anda com o slider Solidez, em vez de
       ficar uma caixa chapada boiando sobre a foto. */
    background: var(--surface-raised);
  }

  .workspace-nav button {
    flex: 1;
    min-width: 0;      /* vence o min-width global de 44px: 3 botoes em 248px */
    height: 30px;
    min-height: 0;
    padding: 0 var(--space-1);
    overflow: hidden;
    border-radius: calc(var(--radius-lg) - 3px);
    color: var(--text-secondary);
    font-family: inherit;
    font-size: var(--text-xs);
    font-weight: 560;
    white-space: nowrap;
    text-overflow: ellipsis;
    transition: color 140ms var(--ease-out), background 140ms var(--ease-out);
  }

  .workspace-nav button:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .workspace-nav button.active {
    color: var(--text-primary);
    background: var(--bg-surface);
    box-shadow: inset 0 0 0 1px var(--border-default);
  }

  /* So o icone: o atalho fica no title, o texto "⌘K" custaria a largura de um dos tres botoes. */
  .command-button {
    flex-shrink: 0;
    width: 34px;
    min-width: 34px;
    height: 34px;
    min-height: 0;
    padding: 0;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-lg);
    background: color-mix(in srgb, var(--bg-elevated) 94%, transparent);
    color: var(--text-secondary);
  }

  .command-button:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

</style>
