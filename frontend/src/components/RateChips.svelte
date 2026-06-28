<script lang="ts">
  import type { StatusFields } from '../lib/statusline';

  interface Props {
    status: StatusFields | null;
    onExpand: () => void;
  }
  let { status, onExpand }: Props = $props();

  // Saturacao da janela: verde calmo -> ambar -> vermelho.
  function pctClass(pct: number | undefined): string {
    if (typeof pct !== 'number' || !isFinite(pct)) return '';
    if (pct >= 90) return 'hot';
    if (pct >= 70) return 'warm';
    return 'cool';
  }

  const has = $derived(
    typeof status?.fiveHourPct === 'number' || typeof status?.weeklyPct === 'number'
  );
</script>

{#if status && has}
  <div class="rate-chips">
    {#if typeof status.fiveHourPct === 'number'}
      <button class="rchip {pctClass(status.fiveHourPct)}" onclick={onExpand} aria-label="Janela de 5 horas">
        <span aria-hidden="true">⚡</span>{status.fiveHourPct}%
      </button>
    {/if}
    {#if typeof status.weeklyPct === 'number'}
      <button class="rchip {pctClass(status.weeklyPct)}" onclick={onExpand} aria-label="Janela de 7 dias">
        <span aria-hidden="true">📅</span>{status.weeklyPct}%
      </button>
    {/if}
  </div>
{/if}

<style>
  .rate-chips {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    flex-shrink: 0;
  }

  .rchip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 28px;
    min-height: 0;
    min-width: 0;
    padding: 0 var(--space-2);
    background: var(--bg-hover);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .rchip.cool { color: var(--success); }
  .rchip.warm { color: var(--warning); }
  .rchip.hot  { color: var(--error); }
</style>
