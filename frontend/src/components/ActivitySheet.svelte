<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { getWorkflows, getWorkflow } from '../lib/api';
  import type { Activity, TaskStatus } from '../lib/activity';
  import type { WorkflowSummary, WorkflowDetail } from '../lib/types';

  interface Props {
    open: boolean;
    activity: Activity;
    sessionName: string;
    onClose: () => void;
  }
  let { open, activity, sessionName, onClose }: Props = $props();

  // view: 'list' (lista geral) ou um runId (detalhe do workflow, estilo /workflows do terminal).
  let view = $state<'list' | string>('list');
  let workflows = $state<WorkflowSummary[]>([]);
  let detail = $state<WorkflowDetail | null>(null);
  let loadingDetail = $state(false);

  // Ao abrir, carrega a lista de workflows (do disco, via backend). Ao fechar, volta pra lista.
  $effect(() => {
    if (!open) {
      view = 'list';
      detail = null;
      return;
    }
    getWorkflows(sessionName).then((w) => (workflows = w)).catch(() => {});
  });

  async function openDetail(runId: string) {
    view = runId;
    detail = null;
    loadingDetail = true;
    try {
      detail = await getWorkflow(sessionName, runId);
    } catch {
      detail = null;
    } finally {
      loadingDetail = false;
    }
  }

  function back() {
    view = 'list';
    detail = null;
  }

  // Só agentes bloqueantes (Agent) rodando — workflows têm sua própria seção (do backend).
  const runningAgents = $derived(activity.agents.filter((a) => a.kind === 'agent' && a.running));

  function mark(status: TaskStatus): string {
    if (status === 'completed') return '✓';
    if (status === 'in_progress') return '◐';
    return '○';
  }

  function fmtTokens(n: number): string {
    if (!n) return '0';
    return n < 1000 ? String(n) : (n / 1000).toFixed(1) + 'k';
  }

  function fmtDur(ms: number): string {
    if (!ms) return '';
    if (ms < 1000) return ms + 'ms';
    const s = ms / 1000;
    if (s < 60) return s.toFixed(1) + 's';
    const m = Math.floor(s / 60);
    return `${m}m ${Math.round(s % 60)}s`;
  }

  function stateGlyph(state: string | null): string {
    if (state === 'done') return '✓';
    if (state === 'error') return '✕';
    return '⟳'; // progress
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Atividade">
  {#if view === 'list'}
    <div class="activity">
      <div class="activity-head">
        <h2 class="activity-title">Atividade</h2>
        {#if activity.total > 0}
          <span class="activity-count">{activity.done}/{activity.total}</span>
        {/if}
      </div>

      {#if workflows.length > 0}
        <div class="section">
          <span class="section-label">Workflows</span>
          {#each workflows as w (w.runId)}
            <button class="wf-row" onclick={() => openDetail(w.runId)}>
              <span class="wf-glyph" class:run={w.running} aria-hidden="true">{w.running ? '⟳' : '⚙'}</span>
              <span class="wf-name">{w.name}</span>
              <span class="wf-meta">{w.agentCount} ag · {fmtTokens(w.totalTokens)}</span>
              <span class="wf-chevron" aria-hidden="true">›</span>
            </button>
          {/each}
        </div>
      {/if}

      {#if runningAgents.length > 0}
        <div class="section">
          <span class="section-label">Rodando agora</span>
          {#each runningAgents as a (a.id)}
            <div class="agent-row">
              <span class="agent-spin" aria-hidden="true">⟳</span>
              <span class="agent-desc">{a.description}</span>
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

      {#if workflows.length === 0 && activity.tasks.length === 0 && runningAgents.length === 0}
        <p class="activity-empty">Nada rolando agora.</p>
      {/if}
    </div>
  {:else}
    <!-- Detalhe do workflow (estilo /workflows): fases + agentes -->
    <div class="activity">
      <div class="detail-head">
        <button class="back-link" onclick={back} aria-label="Voltar">‹ Workflows</button>
      </div>

      {#if loadingDetail}
        <p class="activity-empty">Carregando…</p>
      {:else if !detail}
        <p class="activity-empty">Não encontrei esse run.</p>
      {:else}
        <div class="wf-detail-head">
          <h2 class="activity-title">{detail.name}</h2>
          <span class="wf-status wf-status--{detail.status}">{detail.status}</span>
        </div>
        <div class="wf-detail-meta">
          {detail.agents.length} agentes · {fmtTokens(detail.totalTokens)} tokens{detail.durationMs ? ` · ${fmtDur(detail.durationMs)}` : ''}
        </div>

        {#if detail.phases.length > 0}
          <div class="wf-phases">
            {#each detail.phases as p}
              <span class="wf-phase-chip">{p.title}</span>
            {/each}
          </div>
        {/if}

        <div class="section">
          {#each detail.agents as a, i (i)}
            <div class="wf-agent">
              <div class="wf-agent-top">
                <span class="wf-agent-state wf-agent-state--{a.state}" aria-hidden="true">{stateGlyph(a.state)}</span>
                <span class="wf-agent-label">{a.label ?? 'agente'}</span>
                {#if a.phaseTitle}<span class="wf-agent-phase">{a.phaseTitle}</span>{/if}
                <span class="wf-agent-nums">{fmtTokens(a.tokens)}{a.durationMs ? ` · ${fmtDur(a.durationMs)}` : ''}</span>
              </div>
              {#if a.resultPreview}
                <p class="wf-agent-preview">{a.resultPreview}</p>
              {:else if a.lastToolSummary}
                <p class="wf-agent-preview muted">{a.lastToolName}: {a.lastToolSummary}</p>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</BottomSheet>

<style>
  .activity { display: flex; flex-direction: column; gap: var(--space-4); padding: var(--space-2) 0; }
  .activity-head { display: flex; align-items: baseline; justify-content: space-between; }
  .activity-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .activity-count { font-family: var(--font-mono); font-size: var(--text-sm); font-variant-numeric: tabular-nums; color: var(--text-secondary); }

  .section { display: flex; flex-direction: column; gap: var(--space-2); }
  .section-label { font-size: var(--text-xs); color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.04em; }

  /* Linha de workflow (tappável -> drill-in) */
  .wf-row {
    display: flex; align-items: center; gap: var(--space-2);
    width: 100%; min-height: 40px; padding: var(--space-1) 0;
    text-align: left; border-radius: 0; justify-content: flex-start;
  }
  .wf-row:active { background: var(--bg-hover); }
  .wf-glyph { color: var(--text-muted); flex-shrink: 0; width: 1.2em; text-align: center; }
  .wf-glyph.run { color: var(--accent); animation: spin 0.9s linear infinite; }
  .wf-name { font-size: var(--text-sm); color: var(--text-primary); flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wf-meta { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; flex-shrink: 0; }
  .wf-chevron { color: var(--text-muted); flex-shrink: 0; }

  .agent-row { display: flex; align-items: center; gap: var(--space-2); }
  .agent-spin { color: var(--accent); animation: spin 0.9s linear infinite; flex-shrink: 0; width: 1.1em; text-align: center; }
  .agent-desc { font-size: var(--text-sm); color: var(--text-primary); flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .task-row { display: flex; align-items: baseline; gap: var(--space-2); }
  .task-mark { font-size: var(--text-sm); color: var(--text-muted); flex-shrink: 0; width: 1.1em; text-align: center; }
  .task-text { font-size: var(--text-sm); color: var(--text-secondary); }
  .task-row.active .task-mark { color: var(--accent); }
  .task-row.active .task-text { color: var(--text-primary); }
  .task-row.done .task-mark { color: var(--success, #3fb950); }
  .task-row.done .task-text { color: var(--text-muted); text-decoration: line-through; }

  .activity-empty { font-size: var(--text-sm); color: var(--text-muted); text-align: center; padding: var(--space-4) 0; }

  /* ── Detalhe do workflow ── */
  .detail-head { margin-bottom: var(--space-1); }
  .back-link { color: var(--accent); font-size: var(--text-sm); padding: 0; justify-content: flex-start; }
  .wf-detail-head { display: flex; align-items: center; gap: var(--space-2); justify-content: space-between; }
  .wf-status {
    font-size: var(--text-xs); padding: 1px var(--space-2); border-radius: var(--radius-full);
    background: var(--bg-hover); color: var(--text-secondary); flex-shrink: 0;
  }
  .wf-status--completed { color: var(--success, #3fb950); }
  .wf-status--killed, .wf-status--error { color: var(--error); }
  .wf-status--running { color: var(--accent); }
  .wf-detail-meta { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .wf-phases { display: flex; flex-wrap: wrap; gap: var(--space-1); }
  .wf-phase-chip { font-size: var(--text-xs); color: var(--text-secondary); background: var(--bg-hover); padding: 2px var(--space-2); border-radius: var(--radius-sm); }

  .wf-agent { display: flex; flex-direction: column; gap: 2px; padding: var(--space-2) 0; border-bottom: 1px solid var(--border-subtle); }
  .wf-agent:last-child { border-bottom: none; }
  .wf-agent-top { display: flex; align-items: center; gap: var(--space-2); }
  .wf-agent-state { flex-shrink: 0; width: 1.1em; text-align: center; }
  .wf-agent-state--done { color: var(--success, #3fb950); }
  .wf-agent-state--error { color: var(--error); }
  .wf-agent-state--progress { color: var(--accent); animation: spin 0.9s linear infinite; }
  .wf-agent-label { font-size: var(--text-sm); color: var(--text-primary); font-weight: 500; }
  .wf-agent-phase { font-size: var(--text-xs); color: var(--text-muted); }
  .wf-agent-nums { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; margin-left: auto; flex-shrink: 0; }
  .wf-agent-preview { font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.5; padding-left: calc(1.1em + var(--space-2)); }
  .wf-agent-preview.muted { color: var(--text-muted); }
</style>
