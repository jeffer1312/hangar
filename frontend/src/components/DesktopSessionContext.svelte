<script lang="ts">
  import type { State } from '../lib/types';
  import type { StatusFields } from '../lib/statusline';
  import { stateColors, stateLabels } from '../lib/format';

  interface Props {
    state: State;
    stateDetail?: string | null;
    status?: StatusFields | null;
    pairPeers?: string[] | null;
    serverLabel?: string;
    provider?: 'claude' | 'codex';
  }

  let {
    state, stateDetail = null, status = null, pairPeers = null,
    serverLabel = '', provider = 'claude',
  }: Props = $props();
</script>

<aside class="session-context" aria-label="Contexto da sessão">
  <header>Contexto da sessão</header>

  <section>
    <span class="section-label">Estado</span>
    <span class="state-chip" style="color: {stateColors[state]}">{stateLabels[state]}</span>
    {#if stateDetail}<p>{stateDetail}</p>{/if}
  </section>

  <section>
    <span class="section-label">Contexto</span>
    {#if status?.ctxPct != null}
      <div class="metric-row">
        <span>{Math.round(status.ctxPct)}%</span>
        {#if status.ctxTotal}<span>de {Math.round(status.ctxTotal / 1000)}k tokens</span>{/if}
      </div>
      <div class="progress" aria-label={`${Math.round(status.ctxPct)}% do contexto usado`}>
        <span style:width={`${status.ctxPct}%`}></span>
      </div>
    {:else}
      <p>medição indisponível</p>
    {/if}
  </section>

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
    <strong>{provider === 'codex' ? 'Codex' : 'Claude'}</strong>
    {#if status?.model}<p>{status.model}{status.effort ? ` · ${status.effort}` : ''}</p>{/if}
    {#if serverLabel}<p>{serverLabel}</p>{/if}
  </section>
</aside>

<style>
  .session-context {
    position: absolute;
    /* A navbar tem um fade visual abaixo da altura medida; começa depois dele para o título do
       painel não ficar sob o scrim (o conteúdo do chat pode rolar ali, um header fixo não). */
    top: calc(var(--nav-h, 56px) + var(--navbar-fade, 24px));
    right: 0;
    bottom: 0;
    z-index: 17;
    width: 248px;
    overflow-y: auto;
    border-left: 1px solid var(--border-subtle);
    background: color-mix(in srgb, var(--bg-base) 96%, var(--bg-surface));
  }

  header {
    min-height: 48px;
    display: flex;
    align-items: center;
    padding: 0 var(--space-4);
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-weight: 650;
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
