<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { getWorkflows, getWorkflow, getWorkflowAgent } from '../lib/api';
  import type { Activity, TaskStatus } from '../lib/activity';
  import type { WorkflowSummary, WorkflowDetail, WorkflowAgentDetail } from '../lib/types';

  interface Props {
    open: boolean;
    activity: Activity;
    sessionName: string;
    onClose: () => void;
  }
  let { open, activity, sessionName, onClose }: Props = $props();

  // 3 níveis: lista geral -> detalhe do workflow (fases+agentes) -> detalhe do agente (prompt+result).
  let level = $state<'list' | 'workflow' | 'agent'>('list');
  let runId = $state<string | null>(null);
  let workflows = $state<WorkflowSummary[]>([]);
  let detail = $state<WorkflowDetail | null>(null);
  let agentDetail = $state<WorkflowAgentDetail | null>(null);
  let loading = $state(false);

  // Ao abrir, volta pra lista e (re)carrega os workflows do disco (via backend).
  $effect(() => {
    if (!open) {
      level = 'list';
      runId = null;
      detail = null;
      agentDetail = null;
      return;
    }
    getWorkflows(sessionName).then((w) => (workflows = w)).catch(() => {});
  });

  async function openWorkflow(rid: string) {
    runId = rid;
    level = 'workflow';
    detail = null;
    loading = true;
    try {
      detail = await getWorkflow(sessionName, rid);
    } catch {
      detail = null;
    } finally {
      loading = false;
    }
  }

  async function openAgent(agentId: string | null) {
    if (!runId || !agentId) return;
    level = 'agent';
    agentDetail = null;
    loading = true;
    try {
      agentDetail = await getWorkflowAgent(sessionName, runId, agentId);
    } catch {
      agentDetail = null;
    } finally {
      loading = false;
    }
  }

  function back() {
    if (level === 'agent') {
      level = 'workflow';
      agentDetail = null;
    } else if (level === 'workflow') {
      level = 'list';
      detail = null;
      runId = null;
    }
  }

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
    return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
  }
  function stateGlyph(state: string | null): string {
    if (state === 'done') return '✓';
    if (state === 'error') return '✕';
    return '⟳';
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Atividade">
  {#if level === 'list'}
    <div class="activity">
      <div class="activity-head">
        <h2 class="activity-title">Atividade</h2>
        {#if activity.total > 0}<span class="activity-count">{activity.done}/{activity.total}</span>{/if}
      </div>

      {#if workflows.length > 0}
        <div class="section">
          <span class="section-label">Workflows</span>
          {#each workflows as w (w.runId)}
            <button class="row-btn" onclick={() => openWorkflow(w.runId)}>
              {#if w.running}
                <span class="ring-spin" aria-hidden="true"></span>
              {:else}
                <span class="wf-glyph" aria-hidden="true">⚙</span>
              {/if}
              <span class="row-name">{w.name}</span>
              <span class="row-meta">{w.agentCount} ag · {fmtTokens(w.totalTokens)}</span>
              <span class="row-chevron" aria-hidden="true">›</span>
            </button>
          {/each}
        </div>
      {/if}

      {#if runningAgents.length > 0}
        <div class="section">
          <span class="section-label">Rodando agora</span>
          {#each runningAgents as a (a.id)}
            <div class="agent-row">
              <span class="ring-spin" aria-hidden="true"></span>
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
  {:else if level === 'workflow'}
    <div class="activity">
      <button class="back-link" onclick={back} aria-label="Voltar">‹ Workflows</button>
      {#if loading}
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
            {#each detail.phases as p}<span class="wf-phase-chip">{p.title}</span>{/each}
          </div>
        {/if}
        <div class="section">
          {#each detail.agents as a, i (i)}
            <button class="wf-agent" onclick={() => openAgent(a.agentId)} disabled={!a.agentId}>
              <div class="wf-agent-top">
                {#if a.state === 'progress'}
                  <span class="ring-spin" aria-hidden="true"></span>
                {:else}
                  <span class="wf-agent-state wf-agent-state--{a.state}" aria-hidden="true">{stateGlyph(a.state)}</span>
                {/if}
                <span class="wf-agent-label">{a.label ?? 'agente'}</span>
                {#if a.phaseTitle}<span class="wf-agent-phase">{a.phaseTitle}</span>{/if}
                <span class="wf-agent-nums">{fmtTokens(a.tokens)}{a.durationMs ? ` · ${fmtDur(a.durationMs)}` : ''}</span>
                {#if a.agentId}<span class="row-chevron" aria-hidden="true">›</span>{/if}
              </div>
              {#if a.resultPreview}
                <p class="wf-agent-preview">{a.resultPreview}</p>
              {/if}
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {:else}
    <!-- Detalhe do agente: prompt + resultado completo + ferramentas -->
    <div class="activity">
      <button class="back-link" onclick={back} aria-label="Voltar">‹ {detail?.name ?? 'Workflow'}</button>
      {#if loading}
        <p class="activity-empty">Carregando…</p>
      {:else if !agentDetail}
        <p class="activity-empty">Não encontrei esse agente.</p>
      {:else}
        <div class="wf-detail-head">
          <h2 class="activity-title">
            {#if agentDetail.state === 'progress'}
              <span class="ring-spin" aria-hidden="true"></span>
            {:else}
              <span class="wf-agent-state wf-agent-state--{agentDetail.state}" aria-hidden="true">{stateGlyph(agentDetail.state)}</span>
            {/if}
            {agentDetail.label}
          </h2>
        </div>
        <div class="wf-detail-meta">
          {fmtTokens(agentDetail.tokens)} tokens · {agentDetail.toolCalls} tools{agentDetail.durationMs ? ` · ${fmtDur(agentDetail.durationMs)}` : ''}{agentDetail.model ? ` · ${agentDetail.model}` : ''}
        </div>

        {#if agentDetail.tools.length > 0}
          <div class="wf-phases">
            {#each agentDetail.tools as t}<span class="wf-phase-chip">{t.name}{t.count > 1 ? ` ×${t.count}` : ''}</span>{/each}
          </div>
        {/if}

        <div class="scroll-body">
          {#if agentDetail.prompt}
            <div class="ag-block">
              <span class="section-label">Prompt</span>
              <p class="ag-text">{agentDetail.prompt}</p>
            </div>
          {/if}
          {#if agentDetail.result}
            <div class="ag-block">
              <span class="section-label">Resultado</span>
              <pre class="ag-result">{agentDetail.result}</pre>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</BottomSheet>

<style>
  .activity { display: flex; flex-direction: column; gap: var(--space-4); padding: var(--space-2) 0; }
  .activity-head { display: flex; align-items: baseline; justify-content: space-between; }
  .activity-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); display: inline-flex; align-items: center; gap: var(--space-2); }
  .activity-count { font-family: var(--font-mono); font-size: var(--text-sm); font-variant-numeric: tabular-nums; color: var(--text-secondary); }

  .section { display: flex; flex-direction: column; gap: var(--space-2); }
  .section-label { font-size: var(--text-xs); color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.04em; }

  /* Linha tappável (workflow) */
  .row-btn { display: flex; align-items: center; gap: var(--space-2); width: 100%; min-height: 40px; padding: var(--space-1) 0; text-align: left; border-radius: 0; justify-content: flex-start; }
  .row-btn:active { background: var(--bg-hover); }
  .wf-glyph { color: var(--text-muted); flex-shrink: 0; width: 1.2em; text-align: center; }

  /* Spinner = ANEL (nao glyph de texto): um char girando orbita fora do eixo e parece quadrado;
     um anel simetrico gira perfeito no centro. Usado em todo "rodando" do painel. */
  .ring-spin {
    box-sizing: border-box;
    display: inline-block;
    flex-shrink: 0;
    width: 0.95em;
    height: 0.95em;
    border: 2px solid var(--accent);
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
    vertical-align: -0.12em;
  }
  .row-name { font-size: var(--text-sm); color: var(--text-primary); flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .row-meta { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; flex-shrink: 0; }
  .row-chevron { color: var(--text-muted); flex-shrink: 0; }

  .agent-row { display: flex; align-items: center; gap: var(--space-2); }
  .agent-desc { font-size: var(--text-sm); color: var(--text-primary); flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .task-row { display: flex; align-items: baseline; gap: var(--space-2); }
  .task-mark { font-size: var(--text-sm); color: var(--text-muted); flex-shrink: 0; width: 1.1em; text-align: center; }
  .task-text { font-size: var(--text-sm); color: var(--text-secondary); }
  .task-row.active .task-mark { color: var(--accent); }
  .task-row.active .task-text { color: var(--text-primary); }
  .task-row.done .task-mark { color: var(--success, #3fb950); }
  .task-row.done .task-text { color: var(--text-muted); text-decoration: line-through; }

  .activity-empty { font-size: var(--text-sm); color: var(--text-muted); text-align: center; padding: var(--space-4) 0; }

  .back-link { color: var(--accent); font-size: var(--text-sm); padding: 0; justify-content: flex-start; align-self: flex-start; }

  /* Detalhe do workflow */
  .wf-detail-head { display: flex; align-items: center; gap: var(--space-2); justify-content: space-between; }
  .wf-status { font-size: var(--text-xs); padding: 1px var(--space-2); border-radius: var(--radius-full); background: var(--bg-hover); color: var(--text-secondary); flex-shrink: 0; }
  .wf-status--completed { color: var(--success, #3fb950); }
  .wf-status--killed, .wf-status--error { color: var(--error); }
  .wf-status--running { color: var(--accent); }
  .wf-detail-meta { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .wf-phases { display: flex; flex-wrap: wrap; gap: var(--space-1); }
  .wf-phase-chip { font-size: var(--text-xs); color: var(--text-secondary); background: var(--bg-hover); padding: 2px var(--space-2); border-radius: var(--radius-sm); }

  .wf-agent { display: flex; flex-direction: column; gap: 2px; padding: var(--space-2) 0; border-bottom: 1px solid var(--border-subtle); width: 100%; text-align: left; border-radius: 0; }
  .wf-agent:last-child { border-bottom: none; }
  .wf-agent:active:not(:disabled) { background: var(--bg-hover); }
  .wf-agent:disabled { opacity: 1; }
  .wf-agent-top { display: flex; align-items: center; gap: var(--space-2); }
  .wf-agent-state { flex-shrink: 0; width: 1.1em; text-align: center; }
  .wf-agent-state--done { color: var(--success, #3fb950); }
  .wf-agent-state--error { color: var(--error); }
  .wf-agent-label { font-size: var(--text-sm); color: var(--text-primary); font-weight: 500; }
  .wf-agent-phase { font-size: var(--text-xs); color: var(--text-muted); }
  .wf-agent-nums { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; margin-left: auto; flex-shrink: 0; }
  .wf-agent-preview { font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.5; padding-left: calc(1.1em + var(--space-2)); overflow: hidden; display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; }

  /* Detalhe do agente */
  .scroll-body { max-height: 56vh; overflow-y: auto; display: flex; flex-direction: column; gap: var(--space-4); }
  .ag-block { display: flex; flex-direction: column; gap: var(--space-1); }
  .ag-text { font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.5; white-space: pre-wrap; word-break: break-word; }
  .ag-result { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-primary); line-height: 1.5; white-space: pre-wrap; word-break: break-word; background: var(--bg-surface); padding: var(--space-3); border-radius: var(--radius-sm); margin: 0; }
</style>
