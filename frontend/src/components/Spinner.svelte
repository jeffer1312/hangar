<script lang="ts">
  import { onDestroy } from 'svelte';

  interface Props {
    label?: string | null;
  }
  let { label = null }: Props = $props();

  const stateLabel = $derived(label ?? 'Trabalhando…');

  // Cronometro vivo de liveness (igual "(20s…)" do Claude Code). Conta do mount; o tempo de
  // sessao AUTORITATIVO vive no UsageSheet (status.sessionTime).
  let elapsedLabel = $state('00:00');
  let startedAt = Date.now();
  function fmtElapsed(ms: number): string {
    const total = Math.max(0, Math.floor(ms / 1000));
    const mm = Math.floor(total / 60);
    const ss = total % 60;
    return String(mm).padStart(2, '0') + ':' + String(ss).padStart(2, '0');
  }
  const timer = setInterval(() => {
    elapsedLabel = fmtElapsed(Date.now() - startedAt);
  }, 1000);
  onDestroy(() => clearInterval(timer));
</script>

<div class="spinner" role="status" aria-live="polite">
  <span class="dot" aria-hidden="true"></span>
  <span class="spinner-label">{stateLabel}</span>
  <span class="spinner-time">{elapsedLabel}</span>
</div>

<style>
  /* Linha slim, sem bubble/card — estilo da linha de spinner do Claude Code. */
  .spinner { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-1); animation: bubble-in 200ms var(--ease-out); }
  .dot { width: 7px; height: 7px; border-radius: var(--radius-full); flex-shrink: 0; background: var(--pill-working-fg); animation: pulse-scale 1.4s ease-in-out infinite; }
  .spinner-label { font-size: var(--text-sm); color: var(--text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .spinner-time { font-family: var(--font-mono); font-size: var(--text-xs); font-variant-numeric: tabular-nums; color: var(--text-muted); flex-shrink: 0; }
</style>
