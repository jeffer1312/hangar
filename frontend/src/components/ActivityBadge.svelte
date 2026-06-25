<script lang="ts">
  import { onDestroy } from 'svelte';
  import LiveMetrics from './LiveMetrics.svelte';
  import IconInterrupt from './icons/IconInterrupt.svelte';

  interface Props {
    label?: string | null;     // atividade do Claude (ex "Pollinating…")
    costUsd?: number | null;
    onCancel: () => void;
  }
  let { label = null, costUsd = null, onCancel }: Props = $props();

  const stateLabel = $derived(label ?? 'Trabalhando…');

  // Cronometro local: conta a partir do mount (o badge so existe em working). Aproximacao
  // client-side (reseta no reconnect do SSE) — ancora de liveness, nao tempo autoritativo.
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

<div class="activity-badge" role="status" aria-live="polite">
  <div class="badge-left">
    <span class="dot" aria-hidden="true"></span>
    <span class="state-label">{stateLabel}</span>
    <LiveMetrics timeLabel={elapsedLabel} {costUsd} />
  </div>
  <button class="stop-btn" onclick={onCancel} aria-label="Interromper Claude">
    <IconInterrupt size={16} />
  </button>
</div>

<style>
  .activity-badge {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    margin: var(--space-2) 0;
    padding: var(--space-2) var(--space-3);
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    animation: bubble-in 200ms var(--ease-out);
  }

  .badge-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-full);
    flex-shrink: 0;
    background: var(--pill-working-fg);
    animation: pulse-scale 1.4s ease-in-out infinite;
  }

  .state-label {
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .stop-btn {
    width: 36px;
    height: 36px;
    min-width: 36px;
    min-height: 36px;
    flex-shrink: 0;
    background: transparent;
    border: 1px solid var(--error);
    border-radius: var(--radius-md);
    color: var(--error);
    transition: background 180ms var(--ease-out);
  }

  .stop-btn:active {
    background: rgba(255, 69, 58, 0.08);
  }
</style>
