<script lang="ts">
  // Barra de progresso do plano. Segmentada por Task quando cabe; única quando não cabe. A escolha é
  // do componente, não do chamador — três caminhos de render, zero configuração.
  import { planBadge } from '../lib/plan';
  import type { SessionInfo } from '../lib/types';

  interface Props {
    session: Pick<SessionInfo, 'plan_name' | 'plan_task' | 'plan_task_total' | 'plan_done' | 'plan_total' | 'plan_complete' | 'plan_tasks'>;
    // rail recolhido da sidebar: 34px de trilho não segmentam. Prop, não medição em runtime.
    compact?: boolean;
  }

  let { session, compact = false }: Props = $props();

  const badge = $derived(planBadge(session));
  // > 8 Tasks: segmento de ~20px vira listra ilegível — melhor uma barra honesta.
  const segments = $derived(
    !compact && session.plan_tasks && session.plan_tasks.length > 1 && session.plan_tasks.length <= 8
      ? session.plan_tasks
      : null,
  );
</script>

{#if badge}
  <span class="planrow" class:compact title={badge.title}>
    <span
      class="bar"
      class:solid={!segments}
      role="progressbar"
      aria-valuenow={Math.round(badge.pct)}
      aria-valuemin="0"
      aria-valuemax="100"
      aria-label={badge.title}
    >
      {#if segments}
        {#each segments as [d, t]}
          <span class="s" class:done={t > 0 && d >= t}><i style:width={`${t > 0 ? (d / t) * 100 : 0}%`}></i></span>
        {/each}
      {:else}
        <span class="s" class:done={badge.complete}><i style:width={`${badge.pct}%`}></i></span>
      {/if}
    </span>
    {#if !compact}
      <span class="lbl">{session.plan_done ?? 0}/{session.plan_total ?? 0}</span>
    {/if}
  </span>
{/if}

<style>
  .planrow { display: flex; align-items: center; gap: 8px; margin-top: 6px; }
  /* Rail recolhido: absoluto na base da row, pra não empurrar as iniciais (mesmo motivo do
     .prov-rail). A row precisa de position:relative — conferir antes de plugar. */
  .planrow.compact {
    position: absolute;
    right: 6px;
    bottom: 2px;
    left: 6px;
    margin-top: 0;
  }
  .bar { display: flex; flex: 1; gap: 3px; min-width: 0; }
  .bar.solid { gap: 0; }
  .s {
    flex: 1;
    height: 5px;
    overflow: hidden;
    border-radius: var(--radius-full);
    background: var(--bg-elevated);
  }
  .s i {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--accent);
    transition: width 400ms var(--ease-out);
  }
  .s.done i { background: var(--success); }
  .lbl {
    flex: 0 0 auto;
    color: var(--text-muted);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
  }
</style>
