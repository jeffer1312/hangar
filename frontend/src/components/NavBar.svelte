<script lang="ts">
  import RateChips from './RateChips.svelte';
  import type { StatusFields } from '../lib/statusline';

  interface Props {
    title?: string;
    showBack?: boolean;
    onBack?: () => void;
    onMenu?: () => void;
    // Quando presente, o titulo vira um chip tappavel com chevron (troca de sessao).
    onTitleTap?: () => void;
    status?: StatusFields | null;
    onExpandUsage?: () => void;
    onOpenActivity?: () => void;
    activityBadge?: number;
    // Tem trabalho VIVO (workflow/agent rodando) -> o botao de atividade "respira" pra sinalizar
    // que nao travou, mesmo quando o badge nao conta (workflow em background nao entra no badge).
    activityRunning?: boolean;
    // Espelho do pane (overlays so-TUI): abre o terminal espelhado. terminalAlert pulsa o botao
    // quando ha um overlay aberto que SO da pra interagir pela TUI.
    onOpenTerminal?: () => void;
    terminalAlert?: boolean;
  }
  let { title = 'claude pocket', showBack = false, onBack, onMenu, onTitleTap, status = null, onExpandUsage, onOpenActivity, activityBadge = 0, activityRunning = false, onOpenTerminal, terminalAlert = false }: Props = $props();
</script>

<nav class="navbar">
  <div class="navbar-inner">
    {#if showBack}
      <button class="nav-btn back-btn" onclick={onBack} aria-label="Voltar">
        <svg width="10" height="17" viewBox="0 0 10 17" fill="none" aria-hidden="true">
          <path d="M9 1L1.5 8.5L9 16" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      </button>
    {:else}
      <div class="nav-spacer"></div>
    {/if}

    {#if onTitleTap}
      <button class="title-chip" onclick={onTitleTap} aria-label="Trocar de sessão">
        <span class="chip-text">{title}</span>
        <svg class="chip-chevron" width="11" height="7" viewBox="0 0 11 7" fill="none" aria-hidden="true">
          <path d="M1 1l4.5 4.5L10 1" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      </button>
    {:else}
      <span class="navbar-title">{title}</span>
    {/if}

    <div class="nav-right">
      {#if onOpenTerminal}
        <button class="nav-btn terminal-btn" class:alert={terminalAlert} onclick={onOpenTerminal} aria-label="Terminal (espelho da TUI)">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="2.5" y="4" width="19" height="16" rx="2"/>
            <path d="M6.5 9l3 3-3 3"/>
            <line x1="12.5" y1="15" x2="17" y2="15"/>
          </svg>
        </button>
      {/if}
      {#if onOpenActivity}
        <button class="nav-btn activity-btn" class:running={activityRunning} onclick={onOpenActivity} aria-label="Atividade">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <polyline points="3 5 4.5 6.5 7 4"/>
            <polyline points="3 11.5 4.5 13 7 10.5"/>
            <line x1="10" y1="5.5" x2="20" y2="5.5"/>
            <line x1="10" y1="12" x2="20" y2="12"/>
            <line x1="10" y1="18.5" x2="20" y2="18.5"/>
          </svg>
          {#if activityBadge > 0}<span class="activity-badge">{activityBadge}</span>{/if}
        </button>
      {/if}
      {#if status && onExpandUsage}
        <RateChips {status} onExpand={onExpandUsage} />
      {:else if onMenu}
        <button class="nav-btn menu-btn" onclick={onMenu} aria-label="Menu">
          <svg width="20" height="5" viewBox="0 0 20 5" fill="currentColor" aria-hidden="true">
            <circle cx="2.5" cy="2.5" r="2.5"/>
            <circle cx="10" cy="2.5" r="2.5"/>
            <circle cx="17.5" cy="2.5" r="2.5"/>
          </svg>
        </button>
      {:else}
        <div class="nav-spacer"></div>
      {/if}
    </div>
  </div>
</nav>

<style>
  .navbar {
    background: var(--bg-base);   /* OPACO (#0d0d0f) — sem alpha, ou a camada promovida pode piscar */
    border-bottom: 1px solid var(--border-subtle);
    padding-top: env(safe-area-inset-top);
    flex-shrink: 0;
    z-index: 20;
    /* Camada GPU própria OPACA: o preto do bug do iOS aparece EM CIMA, sobre a navbar; numa camada
       separada e pintada, o backing largado do scroller não consegue sobrepor a navbar de preto.
       Seguro: a navbar não tem descendente position:fixed (os sheets ficam fora dela). Ver WebKit
       #89475 + Apple Dev Forums 705172. */
    transform: translateZ(0);
    -webkit-backface-visibility: hidden;
    backface-visibility: hidden;
  }

  .navbar-inner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 48px;
    padding: 0 var(--space-4);
  }

  .navbar-title {
    font-size: var(--text-lg);
    font-weight: 500;
    color: var(--text-primary);
    letter-spacing: -0.01em;
    flex: 1;
    text-align: center;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Titulo tappavel: chip centralizado com chevron (abre o switcher de sessoes). */
  .title-chip {
    flex: 1;
    min-width: 0;
    height: 36px;
    min-height: 36px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    padding: 0 var(--space-3);
    border-radius: var(--radius-md);
    transition: background 160ms var(--ease-out);
  }

  .title-chip:active {
    background: var(--bg-hover);
  }

  .chip-text {
    font-size: var(--text-lg);
    font-weight: 500;
    color: var(--text-primary);
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .chip-chevron {
    flex-shrink: 0;
    color: var(--text-secondary);
  }

  .nav-btn {
    min-width: 44px;
    min-height: 44px;
    color: var(--accent);
    border-radius: var(--radius-md);
    transition: background 180ms ease-out;
    flex-shrink: 0;
  }

  .nav-btn:active {
    background: var(--bg-hover);
  }

  .nav-spacer {
    min-width: 44px;
  }

  /* Grupo à direita: botão de atividade + chips de uso. */
  .nav-right {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    flex-shrink: 0;
  }

  .terminal-btn {
    color: var(--text-secondary);
  }
  /* Overlay so-TUI aberto: pulsa em accent pra sinalizar que precisa interagir pela TUI. */
  .terminal-btn.alert { color: var(--accent); }
  .terminal-btn.alert svg { animation: breathe 1.4s ease-in-out infinite; }
  @media (prefers-reduced-motion: reduce) {
    .terminal-btn.alert svg { animation: none; }
  }

  .activity-btn {
    position: relative;
    color: var(--text-secondary);
  }

  /* Workflow/agent vivo: icone tinge de accent e "respira" (liveness). transform no svg nao causa
     reflow. prefers-reduced-motion -> so a cor, sem animar. */
  .activity-btn.running { color: var(--accent); }
  .activity-btn.running svg { animation: breathe 1.5s ease-in-out infinite; }
  @keyframes breathe {
    0%, 100% { opacity: 0.55; transform: scale(0.92); }
    50%      { opacity: 1;    transform: scale(1.05); }
  }
  @media (prefers-reduced-motion: reduce) {
    .activity-btn.running svg { animation: none; }
  }

  .activity-badge {
    position: absolute;
    top: 4px;
    right: 2px;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: var(--radius-full);
    background: var(--accent);
    color: #fff;
    font-size: 10px;
    font-weight: 600;
    line-height: 16px;
    text-align: center;
  }
</style>
