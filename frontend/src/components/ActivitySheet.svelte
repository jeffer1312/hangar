<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import type { Activity, TaskStatus } from '../lib/activity';

  interface Props {
    open: boolean;
    activity: Activity;
    onClose: () => void;
  }
  let { open, activity, onClose }: Props = $props();

  // Símbolo por status da tarefa.
  function mark(status: TaskStatus): string {
    if (status === 'completed') return '✓';
    if (status === 'in_progress') return '◐';
    return '○';
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Atividade">
  <div class="activity">
    <div class="activity-head">
      <h2 class="activity-title">Atividade</h2>
      {#if activity.total > 0}
        <span class="activity-count">{activity.done}/{activity.total}</span>
      {/if}
    </div>

    {#if activity.agents.length > 0}
      <div class="section">
        <span class="section-label">Agentes & workflows</span>
        {#each activity.agents as a (a.id)}
          <div class="agent-row">
            {#if a.running}
              <span class="agent-spin" aria-hidden="true">⟳</span>
            {:else}
              <span class="agent-mark" aria-hidden="true">{a.kind === 'workflow' ? '⚙' : '●'}</span>
            {/if}
            <span class="agent-desc">{a.description}</span>
            {#if a.running}
              <span class="agent-tag">rodando</span>
            {:else if a.kind === 'workflow'}
              <span class="agent-tag muted">workflow</span>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    {#if activity.tasks.length > 0}
      <div class="section">
        <span class="section-label">Tarefas</span>
        {#each activity.tasks as t (t.id)}
          <div class="task-row" class:done={t.status === 'completed'} class:active={t.status === 'in_progress'}>
            <span class="task-mark" aria-hidden="true">{mark(t.status)}</span>
            <span class="task-text">{t.status === 'in_progress' && t.activeForm ? t.activeForm : t.title}</span>
          </div>
        {/each}
      </div>
    {/if}

    {#if activity.tasks.length === 0 && activity.agents.length === 0}
      <p class="activity-empty">Nada rolando agora.</p>
    {/if}
  </div>
</BottomSheet>

<style>
  .activity { display: flex; flex-direction: column; gap: var(--space-4); padding: var(--space-2) 0; }
  .activity-head { display: flex; align-items: baseline; justify-content: space-between; }
  .activity-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .activity-count {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
  }

  .section { display: flex; flex-direction: column; gap: var(--space-2); }
  .section-label {
    font-size: var(--text-xs);
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .agent-row { display: flex; align-items: center; gap: var(--space-2); }
  .agent-spin { color: var(--accent); animation: spin 0.9s linear infinite; flex-shrink: 0; width: 1.1em; text-align: center; }
  .agent-mark { color: var(--text-muted); flex-shrink: 0; width: 1.1em; text-align: center; }
  .agent-desc { font-size: var(--text-sm); color: var(--text-primary); flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .agent-tag { font-size: var(--text-xs); color: var(--accent); flex-shrink: 0; }
  .agent-tag.muted { color: var(--text-muted); }

  .task-row { display: flex; align-items: baseline; gap: var(--space-2); }
  .task-mark { font-size: var(--text-sm); color: var(--text-muted); flex-shrink: 0; width: 1.1em; text-align: center; }
  .task-text { font-size: var(--text-sm); color: var(--text-secondary); }

  .task-row.active .task-mark { color: var(--accent); }
  .task-row.active .task-text { color: var(--text-primary); }
  .task-row.done .task-mark { color: var(--success, #3fb950); }
  .task-row.done .task-text { color: var(--text-muted); text-decoration: line-through; }

  .activity-empty { font-size: var(--text-sm); color: var(--text-muted); text-align: center; padding: var(--space-4) 0; }
</style>
