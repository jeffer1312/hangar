<script lang="ts">
  import type { SessionInfo, State } from '../lib/types';
  import { relativeTime } from '../lib/format';

  interface Props {
    session: SessionInfo;
    serverBadge?: { label: string; color: string } | null;
    onClick: () => void;
    onDelete: () => void;
  }
  let { session, serverBadge = null, onClick, onDelete }: Props = $props();

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

  // Titulo = NOME da sessao (tmux), pra distinguir sessoes na mesma pasta. O caminho (cwd) fica na
  // linha de baixo como contexto.
  const title = $derived(session.name);

  // O que identifica a sessao e a ULTIMA pasta do cwd (nome do projeto). Ellipsis padrao corta o
  // fim e some justo com ela; entao split em prefixo (truncavel) + basename (nunca encolhe).
  const cwdParts = $derived.by(() => {
    const p = (session.cwd ?? '').replace(/\/+$/, '');
    const i = p.lastIndexOf('/');
    return i < 0 ? { prefix: '', base: p } : { prefix: p.slice(0, i + 1), base: p.slice(i + 1) };
  });

  // Sessao sem vinculo confiavel (claude manual sem --session-id): NAO da pra abrir o chat com
  // seguranca (mostraria/trocaria a conversa errada). Marca "sem id" e bloqueia o clique.
  const untracked = $derived(session.tracked === false);

  // Swipe to delete state
  let pressing = $state(false);
</script>

<div
  class="session-card"
  class:pressing
  class:untracked
  role="button"
  tabindex="0"
  aria-disabled={untracked}
  onclick={() => !untracked && onClick()}
  onkeydown={(e) => e.key === 'Enter' && !untracked && onClick()}
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
      <span class="name-row">
        <span class="session-name">{title}</span>
        {#if untracked}
          <span class="untracked-badge" title="claude aberto sem --session-id: nao da pra rastrear o transcript com seguranca">⚠ sem id</span>
        {/if}
      </span>
      {#if serverBadge}
        <!-- Origem (qual servidor/PC) em linha PROPRIA: na linha do nome ficava espremida e cortada. -->
        <span class="server-line">
          <span
            class="server-badge"
            style="color: {serverBadge.color}; border-color: {serverBadge.color};"
          >{serverBadge.label}</span>
        </span>
      {/if}
      {#if session.cwd}
        <span class="session-cwd" title={session.cwd}>
          <span class="cwd-prefix">{cwdParts.prefix}</span><span class="cwd-base">{cwdParts.base}</span>
        </span>
      {/if}
      {#if untracked}
        <span class="untracked-hint">reabra com <code>claude --session-id …</code> (ou pelo wrapper) pra rastrear</span>
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

  /* Sem id confiavel: visualmente apagada e nao-clicavel (chat off). Delete continua valendo. */
  .session-card.untracked {
    cursor: not-allowed;
    opacity: 0.62;
    border-style: dashed;
  }
  .session-card.untracked.pressing {
    transform: none;
  }

  .untracked-badge {
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.02em;
    padding: 1px 7px;
    border-radius: var(--radius-full);
    color: var(--warning);
    border: 1px solid var(--warning);
    white-space: nowrap;
  }

  .untracked-hint {
    font-size: var(--text-xs);
    color: var(--warning);
    opacity: 0.85;
  }
  .untracked-hint code {
    font-family: var(--font-mono);
    font-size: 0.92em;
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

  .name-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
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

  /* Linha propria pra origem: chip alinhado a esquerda, nao estica. */
  .server-line {
    display: flex;
    margin: 1px 0 2px;
    min-width: 0;
  }
  .server-badge {
    max-width: 100%;
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.02em;
    padding: 1px 8px;
    border-radius: var(--radius-full);
    border: 1px solid;
    background: transparent;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .session-cwd {
    display: flex;
    min-width: 0;
    font-size: var(--text-sm);
    font-family: var(--font-mono);
  }
  /* Prefixo encolhe e ganha o ellipsis; o basename (nome do projeto) fica sempre inteiro. */
  .cwd-prefix {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
  }
  .cwd-base {
    flex: 0 0 auto;
    white-space: nowrap;
    color: var(--text-secondary);
    font-weight: 500;
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
