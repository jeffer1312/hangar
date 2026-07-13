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
    // Feature #8 (rate-limit radar): repassado pro RateChips -- pode existir mesmo sem status
    // (o banner de limite e independente da statusline custom do usuario).
    limited?: boolean;
    limitReset?: string | null;
    onOpenActivity?: () => void;
    activityBadge?: number;
    // Tem trabalho VIVO (workflow/agent rodando) -> o botao de atividade "respira" pra sinalizar
    // que nao travou, mesmo quando o badge nao conta (workflow em background nao entra no badge).
    activityRunning?: boolean;
    // Espelho do pane (overlays so-TUI): abre o terminal espelhado. terminalAlert pulsa o botao
    // quando ha um overlay aberto que SO da pra interagir pela TUI.
    onOpenTerminal?: () => void;
    terminalAlert?: boolean;
    // Play do runner detectado (npm/etc): abre o RunSheet. runRunning "respira" o botao (verde)
    // quando ha um processo rodando (sinal de que ha algo pra observar/parar).
    onOpenRun?: () => void;
    runRunning?: boolean;
    // Turno ativo (Claude trabalhando) -> barra fina varre o rodape da navbar (sinal "rodando").
    working?: boolean;
    // Subtitulo opcional sob o titulo (ex: "4 sessões"); subtitleHot e o trecho em destaque ambar
    // (ex: "1 aguardando"), so renderiza quando ha acao pendente.
    subtitle?: string | null;
    subtitleHot?: string | null;
    // Desktop: breadcrumb (servidor › sessao › branch) no lugar do titulo centralizado + pilula de estado.
    crumbs?: { server: string; session: string; branch?: string; dirty?: boolean } | null;
    stateLabel?: string;
    stateColor?: string;
    // Badge discreto do provider (ex: "Codex") junto do titulo/crumb — so aparece quando != Claude
    // (Claude e o caso comum, sem ruido visual extra). Ver Chat.svelte.
    providerLabel?: string | null;
  }
  let { title = 'claude pocket', showBack = false, onBack, onMenu, onTitleTap, status = null, onExpandUsage, limited = false, limitReset = null, onOpenActivity, activityBadge = 0, activityRunning = false, onOpenTerminal, terminalAlert = false, onOpenRun, runRunning = false, working = false, subtitle = null, subtitleHot = null, crumbs = null, stateLabel, stateColor, providerLabel = null }: Props = $props();
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

    {#if crumbs}
      <div class="crumbs">
        {#if crumbs.server}
          <span class="crumb crumb-dim">{crumbs.server}</span>
          <span class="crumb-sep" aria-hidden="true">›</span>
        {/if}
        <span class="crumb crumb-strong">{crumbs.session}</span>
        {#if crumbs.branch}
          <span class="crumb-sep" aria-hidden="true">›</span>
          <span class="crumb crumb-branch">{crumbs.branch}{#if crumbs.dirty}<span class="dirty">*</span>{/if}</span>
        {/if}
        {#if providerLabel}<span class="provider-badge">{providerLabel}</span>{/if}
        {#if stateLabel}<span class="state-pill" style="color: {stateColor};">{stateLabel}</span>{/if}
      </div>
    {:else if onTitleTap}
      <button class="title-chip" onclick={onTitleTap} aria-label="Trocar de sessão">
        <span class="chip-text">{title}</span>
        {#if providerLabel}<span class="provider-badge">{providerLabel}</span>{/if}
        <svg class="chip-chevron" width="11" height="7" viewBox="0 0 11 7" fill="none" aria-hidden="true">
          <path d="M1 1l4.5 4.5L10 1" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      </button>
    {:else if subtitle}
      <div class="navbar-titlewrap">
        <span class="navbar-title navbar-title--tight">{title}</span>
        <span class="navbar-sub">{subtitle}{#if subtitleHot} · <span class="navbar-sub-hot">{subtitleHot}</span>{/if}</span>
      </div>
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
      {#if onOpenRun}
        <button class="nav-btn run-btn" class:running={runRunning} onclick={onOpenRun}
                aria-label={runRunning ? 'Rodando (abrir)' : 'Rodar projeto'}>
          <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            {#if runRunning}
              <rect x="6" y="6" width="12" height="12" rx="2" />
            {:else}
              <path d="M8 5v14l11-7z" />
            {/if}
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
      {#if (status && onExpandUsage) || limited}
        <RateChips {status} onExpand={onExpandUsage} {limited} {limitReset} />
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
  {#if working}<div class="work-sweep" aria-hidden="true"></div>{/if}
</nav>

<style>
  .navbar {
    /* Glass num ::before LEAF (bare, sem descendente posicionado), igual ao composer. Host
       transparente + stacking context proprio. Fundo quase opaco SEM blur (WebKit/iOS) -> sem o bug
       #89475 (bloco preto no momentum); refração liquid só no Chromium (data-liquid). */
    position: relative;
    isolation: isolate;
    background: transparent;
    /* Sem barra: nada de border/box-shadow. O ::before pinta o glass (scrim). Fica colada no topo
       (overlay via .navbar-mount no Chat); o conteudo rola POR BAIXO. */
    padding-top: env(safe-area-inset-top);
    z-index: 20;
  }
  .navbar::before {
    content: "";
    position: absolute;
    inset: 0 0 -24px 0;       /* estende 24px abaixo p/ o fade do glass */
    z-index: -1;
    pointer-events: none;
    /* "Glass" sem blur (seguro no iOS): solido sob os botoes (legivel) e some num fade de 24px abaixo
       -> o conteudo que rola por baixo aparece esfumado na costura, sem barra dura. */
    background: linear-gradient(to bottom,
      var(--bg-base) 0%,
      var(--bg-base) calc(100% - 24px),
      transparent 100%);
  }
  /* Chromium (data-liquid): refracao SVG real. O blur fica — Chromium não tem o bug do WebKit. */
  :global(html[data-liquid]) .navbar::before {
    background: var(--glass-bg);
    backdrop-filter: url(#liquid-glass) blur(16px) saturate(180%);
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

  /* Desktop: breadcrumb servidor › sessao › branch + pilula de estado (esquerda, nao centralizado). */
  .crumbs {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2); overflow: hidden;
  }
  .crumb { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-size: var(--text-sm); }
  .crumb-dim { color: var(--text-muted); flex-shrink: 2; }
  .crumb-strong { color: var(--text-primary); font-weight: 600; flex-shrink: 1; }
  .crumb-branch { color: var(--text-secondary); font-family: var(--font-mono); font-size: var(--text-xs); flex-shrink: 1; }
  .crumb-sep { color: var(--text-muted); flex-shrink: 0; }
  .dirty { color: var(--warning); }
  .state-pill {
    flex-shrink: 0; margin-left: var(--space-1);
    font-size: var(--text-xs); font-weight: 600; letter-spacing: 0.02em;
    padding: 2px 9px; border-radius: var(--radius-full); background: var(--bg-elevated);
  }

  /* Badge discreto do provider (ex: "Codex") junto do titulo/crumb. */
  .provider-badge {
    flex-shrink: 0; margin-left: var(--space-1);
    font-size: var(--text-xs); font-weight: 600; letter-spacing: 0.02em;
    padding: 2px 9px; border-radius: var(--radius-full);
    background: var(--accent-dim); color: var(--accent);
  }

  /* Titulo + subtitulo empilhados (ex: lista de sessoes com resumo). */
  .navbar-titlewrap {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 1px;
  }
  .navbar-title--tight {
    flex: 0 0 auto;
    font-size: var(--text-base);
    line-height: 1.2;
    max-width: 100%;
  }
  .navbar-sub {
    font-size: var(--text-xs);
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }
  .navbar-sub-hot {
    color: var(--warning);
    font-weight: 600;
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

  .run-btn { position: relative; }
  .run-btn.running { color: var(--success); }
  .run-btn.running::after {
    content: ''; position: absolute; top: 6px; right: 6px; width: 6px; height: 6px;
    border-radius: 50%; background: var(--success); animation: pulse-scale 1.6s var(--ease-out) infinite;
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

  /* Turno ativo: hairline accent varrendo o rodape da navbar (familia Respiracao "Trabalhando").
     prefers-reduced-motion global (app.css) ja neutraliza o loop. */
  .work-sweep {
    position: absolute;
    left: 0; right: 0; bottom: 0;
    height: 2px;
    z-index: 1;
    background: linear-gradient(90deg, transparent, var(--accent), transparent);
    background-size: 50% 100%;
    background-repeat: no-repeat;
    animation: work-sweep 1.8s ease-in-out infinite;
  }
  @keyframes work-sweep {
    0%   { background-position: -60% 0; }
    100% { background-position: 160% 0; }
  }
</style>
