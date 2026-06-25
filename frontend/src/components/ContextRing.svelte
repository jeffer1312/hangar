<script lang="ts">
  // Codex-style context-usage ring for the composer footer. Glanceable; exact tokens come
  // later via a tap popover. Color shifts only at danger thresholds (functional, not decoration).
  interface Props {
    pct?: number | null;
    size?: number;
  }
  let { pct = null, size = 22 }: Props = $props();

  const R = 9;
  const C = 2 * Math.PI * R;
  const known = $derived(typeof pct === 'number' && isFinite(pct as number));
  const value = $derived(known ? Math.min(100, Math.max(0, pct as number)) : 0);
  const offset = $derived(C * (1 - value / 100));
  const tone = $derived(value >= 90 ? 'error' : value >= 70 ? 'warn' : 'ok');
</script>

<div
  class="ring tone-{tone}"
  role="meter"
  aria-valuemin="0"
  aria-valuemax="100"
  aria-valuenow={known ? Math.round(value) : undefined}
  aria-label="Uso de contexto"
>
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
    <circle cx="12" cy="12" r={R} class="track" />
    {#if known}
      <circle
        cx="12"
        cy="12"
        r={R}
        class="arc"
        stroke-dasharray={C}
        stroke-dashoffset={offset}
        transform="rotate(-90 12 12)"
      />
    {/if}
  </svg>
  <span class="label">{known ? Math.round(value) + '%' : '—'}</span>
</div>

<style>
  .ring {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
  }
  svg {
    flex-shrink: 0;
  }
  .track {
    fill: none;
    stroke: var(--border-default);
    stroke-width: 3;
  }
  .arc {
    fill: none;
    stroke-width: 3;
    stroke-linecap: round;
    /* the one deliberately slow, storytelling motion: context filling */
    transition: stroke-dashoffset 600ms var(--ease-out), stroke 300ms ease;
  }
  .label {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-secondary);
  }
  .tone-ok .arc { stroke: var(--accent); }
  .tone-warn .arc { stroke: var(--warning); }
  .tone-error .arc { stroke: var(--error); }
  .tone-warn .label { color: var(--warning); }
  .tone-error .label { color: var(--error); }
</style>
