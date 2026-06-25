<script lang="ts">
  import type { SessionInfo, State } from '../lib/types';
  import { basename, relativeTime } from '../lib/format';

  interface Props {
    session: SessionInfo;
    onClick: () => void;
    onDelete: () => void;
  }
  let { session, onClick, onDelete }: Props = $props();

  const stateLabels: Record<State, string> = {
    working: 'em execução',
    idle: 'pronto',
    awaiting_input: 'aguardando',
    dead: 'encerrado',
  };

  const stateColors: Record<State, string> = {
    working: 'var(--accent)',
    idle: 'var(--success)',
    awaiting_input: 'var(--warning)',
    dead: 'var(--error)',
  };

  // Titulo = basename do projeto; cai pro nome da sessao quando nao ha cwd.
  const title = $derived(session.cwd ? basename(session.cwd) : session.name);

  // Swipe to delete state
  let pressing = $state(false);
</script>

<div
  class="session-card"
  class:pressing
  role="button"
  tabindex="0"
  onclick={onClick}
  onkeydown={(e) => e.key === 'Enter' && onClick()}
  onpointerdown={() => (pressing = true)}
  onpointerup={() => (pressing = false)}
  onpointerleave={() => (pressing = false)}
>
  <div class="card-left">
    <span
      class="state-dot"
      class:state-dot--pulse={session.state === 'working'}
      style="background: {stateColors[session.state]};"
      aria-hidden="true"
    >
      {#if session.state === 'dead'}✕{/if}
    </span>
    <div class="card-info">
      <span class="session-name">{title}</span>
      {#if session.cwd}
        <span class="session-cwd">{session.cwd}</span>
      {/if}
      {#if session.last_activity}
        <span class="session-activity">
          última atividade: {relativeTime(session.last_activity)}
        </span>
      {/if}
    </div>
  </div>

  <div class="card-right">
    <span class="state-badge" style="color: {stateColors[session.state]};">
      {stateLabels[session.state]}
    </span>
    <button
      class="delete-btn"
      onclick={(e) => { e.stopPropagation(); onDelete(); }}
      aria-label="Excluir sessão {session.name}"
      title="Excluir"
    >
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <polyline points="3 6 5 6 21 6"/>
        <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/>
        <path d="M10 11v6M14 11v6"/>
        <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2"/>
      </svg>
    </button>
  </div>
</div>

<style>
  .session-card {
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-xl);
    padding: var(--space-4);
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-3);
    min-height: 56px;
    cursor: pointer;
    transition: background 180ms ease-out, transform 80ms ease-in-out;
    margin-bottom: var(--space-3);
  }

  .session-card.pressing {
    transform: scale(0.97);
    background: var(--bg-elevated);
  }

  .card-left {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    flex: 1;
    min-width: 0;
  }

  .state-dot {
    width: 8px;
    height: 8px;
    border-radius: var(--radius-full);
    flex-shrink: 0;
    margin-top: 7px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 8px;
    color: #fff;
    font-weight: 700;
  }

  /* Pulsa so quando trabalhando; estatico nos demais estados. */
  .state-dot--pulse {
    animation: pulse-scale 1.4s ease-in-out infinite;
  }

  .card-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .session-name {
    font-size: var(--text-lg);
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .session-cwd {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
  }

  .session-activity {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }

  .card-right {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--space-1);
    flex-shrink: 0;
  }

  .state-badge {
    font-size: var(--text-xs);
    font-weight: 500;
    letter-spacing: 0.02em;
  }

  .delete-btn {
    width: 32px;
    height: 32px;
    min-width: 32px;
    min-height: 32px;
    color: var(--text-muted);
    border-radius: var(--radius-sm);
    transition: color 180ms ease-out, background 180ms ease-out;
    opacity: 0.6;
  }

  .session-card:hover .delete-btn {
    opacity: 1;
  }

  .delete-btn:active {
    background: rgba(255, 69, 58, 0.1);
    color: var(--error);
  }
</style>
