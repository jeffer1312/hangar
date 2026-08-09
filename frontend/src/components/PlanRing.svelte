<script lang="ts">
  interface Props { pct: number; complete?: boolean; size?: number }
  let { pct, complete = false, size = 24 }: Props = $props();
  const R = 9;
  const C = 2 * Math.PI * R;
  // Normaliza pct pra 0..100 SEM nunca produzir NaN: Math.min/max propagam NaN (o texto/ARIA/
  // stroke sairiam com "NaN"), e ±Infinity precisa de comportamento determinístico (Infinity -> 100,
  // -Infinity -> 0, pelo clamp de comparação — `isFinite` os descartaria junto com o NaN).
  const value = $derived(
    Number.isNaN(pct) ? 0
      : pct >= 100 ? 100
      : pct <= 0 ? 0
      : pct,
  );
  const offset = $derived(C * (1 - value / 100));
</script>

<div class="ring" class:done={complete} role="meter"
  aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(value)}
  aria-label={`Progresso do plano: ${Math.round(value)}%`}>
  <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
    <circle cx="12" cy="12" r={R} class="track" />
    <circle cx="12" cy="12" r={R} class="arc" stroke-dasharray={C}
      stroke-dashoffset={offset} transform="rotate(-90 12 12)" />
    <text x="12" y="12.5" text-anchor="middle" dominant-baseline="middle">{Math.round(value)}</text>
  </svg>
</div>

<style>
  .ring { display: inline-flex; flex-shrink: 0; }
  .track { fill: none; stroke: var(--border-default); stroke-width: 3; }
  .arc { fill: none; stroke: var(--text-secondary); stroke-width: 3; stroke-linecap: round; transition: stroke-dashoffset 600ms var(--ease-out), stroke 300ms ease; }
  text { font-family: var(--font-mono); font-size: 9px; font-weight: 600; fill: var(--text-secondary); }
  .done .arc { stroke: var(--success); }
  .done text { fill: var(--success); }
</style>
