<script lang="ts">
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
  // claude-opus-4-8 -> "Opus 4.8" · claude-fable-5 -> "Fable 5" · claude-haiku-4-5-2025... -> "Haiku 4.5"
  function modelShort(m: string | null): string {
    if (!m) return '';
    const s = m.toLowerCase();
    const two = s.match(/(\d+)-(\d+)/);
    const ver = two ? `${two[1]}.${two[2]}` : (s.match(/-(\d+)(?:-|$)/)?.[1] ?? '');
    if (s.includes('opus')) return `Opus ${ver}`.trim();
    if (s.includes('sonnet')) return `Sonnet ${ver}`.trim();
    if (s.includes('haiku')) return `Haiku ${ver}`.trim();
    if (s.includes('fable')) return `Fable ${ver}`.trim();
    return m.replace(/^claude-/, '');
  }
  function modelClass(m: string | null): string {
    const s = (m ?? '').toLowerCase();
    if (s.includes('opus')) return 'm-opus';
    if (s.includes('sonnet')) return 'm-sonnet';
    if (s.includes('haiku')) return 'm-haiku';
    if (s.includes('fable')) return 'm-fable';
    return 'm-other';
  }
  // Agrupa agentes por fase, na ORDEM das phases declaradas (agentes sem fase caem em ''). Running
  // não tem phaseTitle -> 1 grupo só (sem header). done/total alimenta o contador por fase.
  const phaseGroups = $derived.by(() => {
    if (!detail) return [];
    const order = detail.phases.map((p) => p.title ?? '');
    const by = new Map<string, typeof detail.agents>();
    for (const a of detail.agents) {
      const k = a.phaseTitle ?? '';
      if (!by.has(k)) by.set(k, []);
      by.get(k)!.push(a);
    }
    return [...by.keys()]
      .sort((x, y) => (order.indexOf(x) < 0 ? 999 : order.indexOf(x)) - (order.indexOf(y) < 0 ? 999 : order.indexOf(y)))
      .map((k) => {
        const list = by.get(k)!;
        return { title: k, agents: list, done: list.filter((a) => a.state === 'done').length, total: list.length };
      });
  });

  // Navegação de fases: mostra UMA fase por vez (como o terminal: Phases | agentes da fase),
  // em vez de despejar todos os agentes num scroll só. Ao (re)carregar, abre na 1ª fase ainda
  // em andamento (senão a 1ª). hasPhaseNav = só quando há fases nomeadas (running solto não tem).
  let selectedPhaseIdx = $state(0);
  $effect(() => {
    if (!detail) return;
    const running = phaseGroups.findIndex((g) => g.done < g.total);
    selectedPhaseIdx = running >= 0 ? running : 0;
  });
  const activePhase = $derived(phaseGroups[selectedPhaseIdx] ?? null);
  const hasPhaseNav = $derived(phaseGroups.length > 1 || !!phaseGroups[0]?.title);

  // Título do header do modal por nível.
  const headerTitle = $derived(
    level === 'list' ? 'Atividade'
    : level === 'workflow' ? (detail?.name ?? 'Workflow')
    : (agentDetail?.label ?? 'Agente')
  );

  function onBackdrop(e: MouseEvent) {
    if (e.target === e.currentTarget) onClose();
  }
  function onKeydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === 'Escape') {
      if (level === 'list') onClose();
      else back();
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={onBackdrop}>
    <div class="modal" role="dialog" aria-modal="true" aria-label="Atividade">
      <header class="modal-head">
        {#if level !== 'list'}
          <button class="modal-icon-btn" onclick={back} aria-label="Voltar">‹</button>
        {/if}
        <h2 class="modal-title">{headerTitle}</h2>
        {#if level === 'list' && activity.total > 0}
          <span class="activity-count">{activity.done}/{activity.total}</span>
        {:else if level === 'workflow' && detail}
          <span class="wf-status wf-status--{detail.status}">{detail.status}</span>
        {/if}
        <button class="modal-icon-btn modal-close" onclick={onClose} aria-label="Fechar">✕</button>
      </header>

      <div class="modal-body">
        {#if level === 'list'}
          <div class="activity">
            {#if workflows.length > 0}
              <div class="section">
                <span class="section-label">Workflows</span>
                <div class="wf-cards">
                  {#each workflows as w (w.runId)}
                    <button class="wf-card" onclick={() => openWorkflow(w.runId)}>
                      <span class="wf-card-icon" class:running={w.running} aria-hidden="true">
                        {#if w.running}<span class="ring-spin"></span>{:else}✓{/if}
                      </span>
                      <div class="wf-card-body">
                        <span class="wf-card-name">{w.name}</span>
                        <span class="wf-card-meta">{w.agentCount} agentes · {fmtTokens(w.totalTokens)} tokens{w.phaseCount ? ` · ${w.phaseCount} fases` : ''}</span>
                      </div>
                      <span class="wf-card-badge" class:running={w.running}>{w.running ? 'rodando' : 'concluído'}</span>
                      <span class="row-chevron" aria-hidden="true">›</span>
                    </button>
                  {/each}
                </div>
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
            {#if loading}
              <p class="activity-empty">Carregando…</p>
            {:else if !detail}
              <p class="activity-empty">Não encontrei esse run.</p>
            {:else}
              <div class="wf-metrics">
                <span class="wf-metric"><b>{detail.agents.length}</b> agentes</span>
                <span class="wf-metric"><b>{fmtTokens(detail.totalTokens)}</b> tokens</span>
                {#if detail.phases.length}<span class="wf-metric"><b>{detail.phases.length}</b> fases</span>{/if}
                {#if detail.durationMs}<span class="wf-metric">{fmtDur(detail.durationMs)}</span>{/if}
              </div>

              <div class="wf-cols">
                {#if hasPhaseNav}
                  <nav class="phase-nav" aria-label="Fases">
                    {#each phaseGroups as g, idx (g.title)}
                      <button class="phase-tab" class:active={idx === selectedPhaseIdx} onclick={() => (selectedPhaseIdx = idx)}>
                        <span class="phase-tab-name">{g.title || 'Agentes'}</span>
                        <span class="phase-tab-count" class:phase-tab-count--done={g.done === g.total}>{g.done}/{g.total}</span>
                      </button>
                    {/each}
                  </nav>
                {/if}
                <div class="phase-agents">
                  {#each (activePhase?.agents ?? []) as a, i (a.agentId ?? i)}
                    <button class="wf-agent" onclick={() => openAgent(a.agentId)} disabled={!a.agentId}>
                      <div class="wf-agent-top">
                        {#if a.state === 'progress'}
                          <span class="ring-spin" aria-hidden="true"></span>
                        {:else}
                          <span class="wf-agent-state wf-agent-state--{a.state}" aria-hidden="true">{stateGlyph(a.state)}</span>
                        {/if}
                        <span class="wf-agent-label">{a.label ?? 'agente'}</span>
                        {#if a.model}<span class="model-badge {modelClass(a.model)}">{modelShort(a.model)}</span>{/if}
                        {#if a.agentId}<span class="row-chevron" aria-hidden="true">›</span>{/if}
                      </div>
                      <div class="wf-agent-stats">
                        <span>{fmtTokens(a.tokens)} tok</span>
                        {#if a.toolCalls}<span>· {a.toolCalls} tools</span>{/if}
                        {#if a.lastToolName}<span class="wf-agent-tool">· {a.lastToolName}</span>{/if}
                        {#if a.durationMs}<span>· {fmtDur(a.durationMs)}</span>{/if}
                      </div>
                    </button>
                  {/each}
                </div>
              </div>

              {#if detail.summary}
                <div class="ag-block wf-summary">
                  <span class="section-label">Resumo</span>
                  <p class="ag-text">{detail.summary}</p>
                </div>
              {/if}
            {/if}
          </div>
        {:else}
          <!-- Detalhe do agente: prompt + resultado completo + ferramentas -->
          <div class="activity">
            {#if loading}
              <p class="activity-empty">Carregando…</p>
            {:else if !agentDetail}
              <p class="activity-empty">Não encontrei esse agente.</p>
            {:else}
              <div class="wf-detail-meta">
                <span class="wf-agent-state wf-agent-state--{agentDetail.state}">{stateGlyph(agentDetail.state)}</span>
                {fmtTokens(agentDetail.tokens)} tokens · {agentDetail.toolCalls} tools{agentDetail.durationMs ? ` · ${fmtDur(agentDetail.durationMs)}` : ''}{agentDetail.model ? ` · ${modelShort(agentDetail.model)}` : ''}
              </div>

              {#if agentDetail.tools.length > 0}
                <div class="wf-phases">
                  {#each agentDetail.tools as t}<span class="wf-phase-chip">{t.name}{t.count > 1 ? ` ×${t.count}` : ''}</span>{/each}
                </div>
              {/if}

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
            {/if}
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  /* ── Modal responsivo: mobile = full-screen; desktop (≥720px) = card central largo ── */
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: flex-end;
    justify-content: center;
  }
  .modal {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100dvh;
    max-height: 100dvh;
    background: var(--bg-elevated);
    animation: slide-up 220ms var(--ease-out) both;
  }
  @media (min-width: 720px) {
    .backdrop { align-items: center; padding: var(--space-5); }
    .modal {
      width: 100%;
      max-width: 820px;
      height: auto;
      max-height: 85vh;
      border: 1px solid var(--border-subtle);
      border-radius: var(--radius-xl);
      box-shadow: 0 24px 64px rgba(0, 0, 0, 0.5);
      animation: zoom-in 180ms var(--ease-out) both;
    }
  }
  @keyframes slide-up { from { transform: translateY(100%); } to { transform: translateY(0); } }
  @keyframes zoom-in { from { opacity: 0; transform: scale(0.97); } to { opacity: 1; transform: scale(1); } }

  .modal-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
    padding: var(--space-4) var(--space-5);
    padding-top: calc(env(safe-area-inset-top) + var(--space-4));
    border-bottom: 1px solid var(--border-subtle);
  }
  @media (min-width: 720px) { .modal-head { padding-top: var(--space-4); } }
  .modal-title {
    flex: 1;
    min-width: 0;
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .modal-icon-btn {
    width: 32px;
    height: 32px;
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
    background: transparent;
    border: none;
    border-radius: var(--radius-sm);
    font-size: 22px;
    line-height: 1;
    -webkit-appearance: none;
    appearance: none;
  }
  .modal-icon-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
  .modal-icon-btn:active { background: var(--bg-hover); }
  .modal-close { font-size: 15px; }

  .modal-body {
    flex: 1;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
    padding: var(--space-4) var(--space-5);
    padding-bottom: calc(env(safe-area-inset-bottom) + var(--space-5));
  }

  .activity { display: flex; flex-direction: column; gap: var(--space-4); }
  .activity-count { font-family: var(--font-mono); font-size: var(--text-sm); font-variant-numeric: tabular-nums; color: var(--text-secondary); flex-shrink: 0; }

  .section { display: flex; flex-direction: column; gap: var(--space-2); }
  .section-label { font-size: var(--text-xs); color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.04em; }

  /* Card de workflow (lista) */
  .wf-cards { display: flex; flex-direction: column; gap: var(--space-2); }
  .wf-card { display: flex; align-items: center; gap: var(--space-3); width: 100%; text-align: left; padding: var(--space-3); background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg, 12px); transition: background 150ms ease-out; }
  .wf-card:hover { background: var(--bg-hover); }
  .wf-card-icon { width: 28px; height: 28px; flex-shrink: 0; display: inline-flex; align-items: center; justify-content: center; border-radius: var(--radius-full); background: var(--bg-hover); color: var(--success, #3fb950); font-size: 13px; }
  .wf-card-icon.running { background: transparent; color: var(--accent); }
  .wf-card-body { display: flex; flex-direction: column; gap: 2px; flex: 1; min-width: 0; }
  .wf-card-name { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wf-card-meta { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .wf-card-badge { flex-shrink: 0; font-size: 10px; font-weight: 600; padding: 1px 8px; border-radius: var(--radius-full); color: var(--text-muted); background: var(--bg-hover); }
  .wf-card-badge.running { color: var(--accent); }

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

  /* Detalhe do workflow */
  .wf-status { font-size: var(--text-xs); padding: 1px var(--space-2); border-radius: var(--radius-full); background: var(--bg-hover); color: var(--text-secondary); flex-shrink: 0; }
  .wf-status--completed { color: var(--success, #3fb950); }
  .wf-status--killed, .wf-status--error { color: var(--error); }
  .wf-status--running { color: var(--accent); }
  .wf-detail-meta { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; display: flex; align-items: center; gap: var(--space-1); flex-wrap: wrap; }
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
  .wf-agent-label { font-size: var(--text-sm); color: var(--text-primary); font-weight: 500; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .wf-agent-top .row-chevron { margin-left: auto; }

  /* Badge de modelo colorido por familia */
  .model-badge { flex-shrink: 0; font-size: 10px; font-weight: 600; letter-spacing: 0.01em; padding: 1px 6px; border-radius: var(--radius-full); border: 1px solid currentColor; white-space: nowrap; }
  .model-badge.m-opus   { color: var(--accent); }
  .model-badge.m-sonnet { color: #4a9eff; }
  .model-badge.m-haiku  { color: var(--success, #3fb950); }
  .model-badge.m-fable  { color: var(--warning, #d29922); }
  .model-badge.m-other  { color: var(--text-muted); }

  /* Métricas do workflow (números em destaque) */
  .wf-metrics { display: flex; flex-wrap: wrap; gap: var(--space-3); }
  .wf-metric { font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .wf-metric b { color: var(--text-primary); font-weight: 600; }

  /* Fases: tabs horizontais (mobile) / rail vertical (desktop) + agentes só da fase ativa */
  .wf-cols { display: flex; flex-direction: column; gap: var(--space-3); }
  .phase-nav { display: flex; flex-direction: row; gap: var(--space-1); overflow-x: auto; -webkit-overflow-scrolling: touch; padding-bottom: 2px; }
  .phase-tab { display: flex; align-items: center; gap: var(--space-2); flex-shrink: 0; padding: var(--space-1) var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-full); background: transparent; color: var(--text-secondary); font-size: var(--text-xs); white-space: nowrap; }
  .phase-tab.active { background: var(--bg-hover); color: var(--text-primary); border-color: var(--accent); }
  .phase-tab-name { font-weight: 600; }
  .phase-tab-count { font-family: var(--font-mono); font-variant-numeric: tabular-nums; color: var(--text-muted); }
  .phase-tab-count--done { color: var(--success, #3fb950); }
  .phase-agents { display: flex; flex-direction: column; min-width: 0; }

  @media (min-width: 720px) {
    .wf-cols { flex-direction: row; align-items: flex-start; gap: var(--space-5); }
    .phase-nav { flex-direction: column; width: 184px; flex-shrink: 0; overflow: visible; position: sticky; top: 0; }
    .phase-tab { width: 100%; justify-content: space-between; border-radius: var(--radius-sm); }
    .phase-agents { flex: 1; }
  }

  /* Linha de stats do agente */
  .wf-agent-stats { display: flex; flex-wrap: wrap; gap: 4px; font-size: var(--text-xs); color: var(--text-muted); font-variant-numeric: tabular-nums; padding-left: calc(1.1em + var(--space-2)); }
  .wf-agent-tool { color: var(--text-secondary); }
  .wf-summary { margin-top: var(--space-2); }

  /* Detalhe do agente */
  .ag-block { display: flex; flex-direction: column; gap: var(--space-1); }
  .ag-text { font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.5; white-space: pre-wrap; word-break: break-word; }
  .ag-result { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-primary); line-height: 1.5; white-space: pre-wrap; word-break: break-word; background: var(--bg-surface); padding: var(--space-3); border-radius: var(--radius-sm); margin: 0; }
</style>
