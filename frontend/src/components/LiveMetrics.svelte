<script lang="ts">
  // Linha compacta de metricas, alinhada a direita, para a status row do composer.
  // Mostra o tempo decorrido e o custo lado a lado; nada e renderizado se ambos faltarem.
  interface Props {
    timeLabel?: string | null;
    costUsd?: number | null;
  }
  let { timeLabel = null, costUsd = null }: Props = $props();

  const hasTime = $derived(typeof timeLabel === 'string' && timeLabel.length > 0);
  const hasCost = $derived(typeof costUsd === 'number' && isFinite(costUsd));
  const costLabel = $derived(hasCost ? '$' + (costUsd as number).toFixed(2) : '');
</script>

{#if hasTime || hasCost}
  <div class="metrics">
    {#if hasTime}<span class="metric">{timeLabel}</span>{/if}
    {#if hasTime && hasCost}<span class="sep" aria-hidden="true">·</span>{/if}
    {#if hasCost}<span class="metric">{costLabel}</span>{/if}
  </div>
{/if}

<style>
  .metrics {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    text-align: right;
  }

  .metric {
    white-space: nowrap;
  }

  .sep {
    padding: 0 var(--space-1);
  }
</style>
