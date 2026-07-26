<script lang="ts">
  import RateChips from './RateChips.svelte';
  import type { State } from '../lib/types';
  import type { StatusFields } from '../lib/statusline';
  import { stateColors, stateLabels, ctxWindow } from '../lib/format';

  interface Props {
    state: State;
    stateDetail?: string | null;
    status?: StatusFields | null;
    pairPeers?: string[] | null;
    serverLabel?: string;
    provider?: 'claude' | 'codex';
    // Nome da sessao aberta: com a NavBar escondida (>=1280px) ele some da tela — e no overlay
    // do board/canvas nem a lista lateral esta a vista. Vai no header do painel.
    sessionName?: string;
    // ── Acoes da NavBar ──────────────────────────────────────────────────────
    // No desktop LARGO (>=1280px, mesma breakpoint deste painel) o Chat esconde a NavBar — a
    // informacao dela ja vivia toda aqui — e as ACOES migram pra faixa sob o header. Todas
    // opcionais: sem handler, sem botao (mesma regra da NavBar).
    onOpenTerminal?: () => void;
    terminalAlert?: boolean;
    onOpenRun?: () => void;
    runRunning?: boolean;
    onOpenAttachments?: () => void;
    onOpenActivity?: () => void;
    activityBadge?: number;
    activityRunning?: boolean;
    onExpandUsage?: () => void;
    limited?: boolean;
    limitReset?: string | null;
    // Sinal "rodando": hairline varrendo o topo do painel (a work-sweep saiu da NavBar junto).
    working?: boolean;
    // Chip do loop (🔁 N/M) — morava na NavBar; aqui vai junto do estado.
    loopLabel?: string | null;
    loopColor?: string;
    onLoopTap?: () => void;
    // Codex: tocar o provider abre os limites de uso (na NavBar era o badge tappavel).
    onProviderTap?: () => void;
  }

  let {
    state, stateDetail = null, status = null, pairPeers = null,
    serverLabel = '', provider = 'claude', sessionName = '',
    onOpenTerminal = undefined, terminalAlert = false,
    onOpenRun = undefined, runRunning = false,
    onOpenAttachments = undefined,
    onOpenActivity = undefined, activityBadge = 0, activityRunning = false,
    onExpandUsage = undefined, limited = false, limitReset = null,
    working = false,
    loopLabel = null, loopColor = undefined, onLoopTap = undefined,
    onProviderTap = undefined,
  }: Props = $props();

  const hasActions = $derived(onOpenTerminal || onOpenRun || onOpenAttachments || onOpenActivity);
  // RateChips se auto-esconde quando nao tem dial nem banner; a SECAO precisa do mesmo guarda,
  // senao sobra um rotulo "Limites" orfao. isFinite como o known() do RateChips: uma statusline
  // custom que escreva NaN/Infinity nao abre a secao pra um corpo vazio.
  const _known = (pct: number | undefined) => typeof pct === 'number' && isFinite(pct);
  const hasRate = $derived(limited || _known(status?.fiveHourPct) || _known(status?.weeklyPct));
</script>

<aside class="session-context" aria-label="Contexto da sessão">
  {#if working}<div class="ctx-sweep" aria-hidden="true"></div>{/if}
  <header>
    <span class="header-title">Contexto da sessão</span>
    {#if sessionName}<span class="header-session">{sessionName}</span>{/if}
  </header>

  {#if hasActions}
    <div class="ctx-actions">
      {#if onOpenTerminal}
        <button class="ctx-btn terminal-btn" class:alert={terminalAlert} onclick={onOpenTerminal} aria-label="Terminal (espelho da TUI)">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect x="2.5" y="4" width="19" height="16" rx="2"/>
            <path d="M6.5 9l3 3-3 3"/>
            <line x1="12.5" y1="15" x2="17" y2="15"/>
          </svg>
        </button>
      {/if}
      {#if onOpenRun}
        <button class="ctx-btn run-btn" class:running={runRunning} onclick={onOpenRun}
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
      {#if onOpenAttachments}
        <button class="ctx-btn" onclick={onOpenAttachments} aria-label="Anexos da sessão">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M21 11l-8.5 8.5a5 5 0 0 1-7-7L14 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L16 6"/>
          </svg>
        </button>
      {/if}
      {#if onOpenActivity}
        <button class="ctx-btn activity-btn" class:running={activityRunning} onclick={onOpenActivity} aria-label="Atividade">
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
    </div>
  {/if}

  <section>
    <span class="section-label">Estado</span>
    <span class="state-chip" style="color: {stateColors[state]}">{stateLabels[state]}</span>
    {#if loopLabel}
      <button type="button" class="loop-chip" style="color: {loopColor};" onclick={onLoopTap} aria-label="Loop da sessão: {loopLabel}">{loopLabel}</button>
    {/if}
    {#if stateDetail}<p>{stateDetail}</p>{/if}
  </section>

  <section>
    <span class="section-label">Contexto</span>
    {#if status?.ctxPct != null}
      <div class="metric-row">
        <span>{Math.round(status.ctxPct)}%</span>
        {#if status.ctxTotal}<span>de {ctxWindow(status.ctxTotal)} tokens</span>{/if}
      </div>
      <div class="progress" aria-label={`${Math.round(status.ctxPct)}% do contexto usado`}>
        <span style:width={`${status.ctxPct}%`}></span>
      </div>
    {:else}
      <p>medição indisponível</p>
    {/if}
  </section>

  {#if hasRate}
    <section>
      <span class="section-label">Limites</span>
      <RateChips {status} onExpand={onExpandUsage} {limited} {limitReset} />
    </section>
  {/if}

  <section>
    <span class="section-label">Grupo</span>
    {#if pairPeers?.length}
      <strong>🤝 {pairPeers.join(' · ')}</strong>
      <p>{pairPeers.length + 1} sessões pareadas</p>
    {:else}
      <p>sessão independente</p>
    {/if}
  </section>

  <section>
    <span class="section-label">Repositório</span>
    {#if status?.repo}
      <strong class="mono">{status.repo}</strong>
      <p class="mono">{status.branch ?? 'sem branch'}{status.dirty ? ' · alterações locais' : ''}</p>
    {:else}
      <p>não detectado</p>
    {/if}
  </section>

  <section>
    <span class="section-label">Execução</span>
    {#if onProviderTap}
      <button type="button" class="provider-tap" onclick={onProviderTap} aria-label="Limites de uso do provider">
        {provider === 'codex' ? 'Codex' : 'Claude'}
      </button>
    {:else}
      <strong>{provider === 'codex' ? 'Codex' : 'Claude'}</strong>
    {/if}
    {#if status?.model}<p>{status.model}{status.effort ? ` · ${status.effort}` : ''}</p>{/if}
    {#if serverLabel}<p>{serverLabel}</p>{/if}
  </section>
</aside>

<style>
  .session-context {
    position: absolute;
    /* A navbar tem um fade visual abaixo da altura medida; começa depois dele para o título do
       painel não ficar sob o scrim (o conteúdo do chat pode rolar ali, um header fixo não).
       >=1280px a navbar some (Chat esconde) e o painel sobe pro topo. */
    top: calc(var(--nav-h, 56px) + var(--navbar-fade, 24px));
    right: 0;
    bottom: 0;
    z-index: 17;
    width: 248px;
    overflow-y: auto;
    border-left: 1px solid var(--border-subtle);
    background: color-mix(in srgb, var(--bg-base) 96%, var(--bg-surface));
  }

  @media (min-width: 1280px) {
    /* Sem navbar, --nav-h carrega so o topInset (ex: faixa de atencao, 52px) — o painel comeca
       abaixo dela, nunca embaixo. */
    .session-context { top: var(--nav-h, 0px); }
  }

  header {
    min-height: 48px;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 0 var(--space-4);
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-weight: 650;
  }

  .header-title { flex-shrink: 0; }

  /* Nome da sessao: dim, mono como o crumb irmao da NavBar, trunca sem empurrar o titulo. */
  .header-session {
    min-width: 0;
    overflow: hidden;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-weight: 500;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Faixa de acoes (ex-botoes da NavBar). Alvos de 40px: painel tem 248px, 4x44 nao cabiam com
     folga; o desktop nao tem o requisito de toque do celular. */
  .ctx-actions {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-1) var(--space-3);
    border-bottom: 1px solid var(--border-subtle);
  }

  .ctx-btn {
    min-width: 40px;
    min-height: 40px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--accent);
    border-radius: var(--radius-md);
    transition: background 180ms ease-out;
  }
  .ctx-btn:active { background: var(--bg-hover); }

  .terminal-btn { color: var(--text-secondary); }
  .terminal-btn.alert { color: var(--accent); }
  .terminal-btn.alert svg { animation: breathe 1.4s ease-in-out infinite; }

  .run-btn { position: relative; }
  .run-btn.running { color: var(--success); }
  .run-btn.running::after {
    content: ''; position: absolute; top: 6px; right: 6px; width: 6px; height: 6px;
    border-radius: 50%; background: var(--success); animation: pulse-scale 1.6s var(--ease-out) infinite;
  }

  .activity-btn { position: relative; color: var(--text-secondary); }
  .activity-btn.running { color: var(--accent); }
  .activity-btn.running svg { animation: breathe 1.5s ease-in-out infinite; }
  @keyframes breathe {
    0%, 100% { opacity: 0.55; transform: scale(0.92); }
    50%      { opacity: 1;    transform: scale(1.05); }
  }
  @media (prefers-reduced-motion: reduce) {
    .terminal-btn.alert svg, .activity-btn.running svg { animation: none; }
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

  /* Turno ativo: hairline accent varrendo o TOPO do painel (a irma da work-sweep da NavBar).
     prefers-reduced-motion global (app.css) ja neutraliza o loop. */
  .ctx-sweep {
    position: sticky;
    top: 0;
    height: 2px;
    z-index: 1;
    background: linear-gradient(90deg, transparent, var(--accent), transparent);
    background-size: 50% 100%;
    background-repeat: no-repeat;
    animation: ctx-sweep 1.8s ease-in-out infinite;
  }
  @keyframes ctx-sweep {
    0%   { background-position: -60% 0; }
    100% { background-position: 160% 0; }
  }

  section {
    margin: 0 var(--space-4);
    padding: var(--space-4) 0;
    border-bottom: 1px solid var(--border-subtle);
  }

  section:last-child { border-bottom: 0; }

  .section-label {
    display: block;
    margin-bottom: var(--space-2);
    color: var(--text-muted);
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .state-chip {
    display: inline-flex;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    background: var(--bg-elevated);
    font-size: var(--text-xs);
    font-weight: 650;
  }

  /* Chip do loop (🔁 N/M): mono como os badges numericos; cor vem do tone via style inline. */
  .loop-chip {
    margin-left: var(--space-1);
    font-family: var(--font-mono); font-size: var(--text-xs); font-weight: 600;
    padding: 2px 8px; border-radius: var(--radius-full);
    background: var(--bg-surface); border: 1px solid var(--border-subtle); cursor: pointer;
  }
  .loop-chip:active { background: var(--bg-hover); }

  /* Provider tappavel (Codex -> limites): visual do <strong> irmao, comportamento de botao. */
  .provider-tap {
    display: block;
    width: 100%;
    padding: 0;
    text-align: left;
    overflow: hidden;
    color: var(--accent);
    font-size: var(--text-xs);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-radius: var(--radius-sm);
  }

  strong {
    display: block;
    overflow: hidden;
    color: var(--text-secondary);
    font-size: var(--text-xs);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  p {
    margin: 3px 0 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
    line-height: 1.45;
  }

  .mono { font-family: var(--font-mono); }

  .metric-row {
    display: flex;
    justify-content: space-between;
    gap: var(--space-2);
    color: var(--text-secondary);
    font-size: var(--text-xs);
  }

  .metric-row span:last-child { color: var(--text-muted); }

  .progress {
    height: 4px;
    margin-top: var(--space-3);
    overflow: hidden;
    border-radius: var(--radius-full);
    background: var(--bg-elevated);
  }

  .progress span {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
  }

  @media (max-width: 1279px) {
    .session-context { display: none; }
  }
</style>
