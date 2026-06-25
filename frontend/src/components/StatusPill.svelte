<script lang="ts">
  import type { State } from '../lib/types';

  interface Props {
    state: State;
    label?: string | null;
  }
  let { state, label }: Props = $props();

  const labels: Record<State, string> = {
    working: '',
    idle: 'Pronto',
    awaiting_input: 'Aguardando resposta',
    dead: 'Sessão encerrada',
  };

  const displayLabel = $derived(
    state === 'working' ? (label ?? 'Trabalhando…') : labels[state]
  );
</script>

<div class="pill-wrap">
  <div class="pill pill--{state}" role="status" aria-live="polite">
    <span class="dot dot--{state}" aria-hidden="true">
      {#if state === 'dead'}✕{/if}
    </span>
    <span class="pill-label">{displayLabel}</span>
  </div>
</div>

<style>
  .pill-wrap {
    display: inline-flex;
    flex-shrink: 0;
    pointer-events: none;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    height: 32px;
    padding: 0 14px;
    border-radius: var(--radius-lg);
    font-size: var(--text-sm);
    font-weight: 500;
    line-height: 1.4;
    transition: background-color 200ms ease-out, color 200ms ease-out;
  }

  .pill--working {
    background: var(--pill-working-bg);
    color: var(--pill-working-fg);
  }

  .pill--idle {
    background: var(--pill-idle-bg);
    color: var(--pill-idle-fg);
  }

  .pill--awaiting_input {
    background: var(--pill-input-bg);
    color: var(--pill-input-fg);
  }

  .pill--dead {
    background: var(--pill-dead-bg);
    color: var(--pill-dead-fg);
  }

  /* Dot indicator */
  .dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-full);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    font-size: 9px;
    line-height: 1;
  }

  .dot--working {
    background: currentColor;
    animation: spin 0.8s linear infinite;
  }

  .dot--idle {
    background: currentColor;
  }

  .dot--awaiting_input {
    background: currentColor;
    animation: pulse-scale 1s ease-in-out infinite;
  }

  .dot--dead {
    background: transparent;
    width: auto;
    height: auto;
    font-size: 11px;
    font-weight: 600;
  }
</style>
