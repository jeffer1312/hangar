<script lang="ts">
  // Ring de uso de contexto: a % vai DENTRO do circulo (centralizada). Cor muda so nos limiares.
  interface Props {
    pct?: number | null;
    size?: number;
  }
  let { pct = null, size = 26 }: Props = $props();

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
    <text x="12" y="12.5" class="ring-text" text-anchor="middle" dominant-baseline="middle">
      {known ? Math.round(value) : '—'}
    </text>
  </svg>
</div>

<style>
  .ring {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
  }
  svg { flex-shrink: 0; }
  .track {
    fill: none;
    stroke: var(--border-default);
    stroke-width: 3;
  }
  .arc {
    fill: none;
    stroke-width: 3;
    stroke-linecap: round;
    transition: stroke-dashoffset 600ms var(--ease-out), stroke 300ms ease;
  }
  /* % dentro do anel: numero (sem '%' pra caber); o anel ja diz que e percentual. */
  .ring-text {
    font-family: var(--font-mono);
    font-size: 9px;
    font-weight: 600;
    fill: var(--text-secondary);
  }
  .tone-ok .arc { stroke: var(--accent); }
  .tone-warn .arc { stroke: var(--warning); }
  .tone-error .arc { stroke: var(--error); }
  .tone-warn .ring-text { fill: var(--warning); }
  .tone-error .ring-text { fill: var(--error); }
</style>
