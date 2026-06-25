<script lang="ts">
  import StatusPill from './StatusPill.svelte';
  import type { State } from '../lib/types';

  interface Props {
    state: State;
    label?: string | null;
    statusLine?: string | null;
  }
  let { state, label, statusLine }: Props = $props();
</script>

<!-- Bottom status bar: the session's raw terminal statusline (verbatim) + the state pill. -->
<div class="status-bar">
  <div class="status-text">
    {#if statusLine}<pre>{statusLine}</pre>{:else}<span class="muted">sem statusline</span>{/if}
  </div>
  <StatusPill {state} {label} />
</div>

<style>
  .status-bar {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-4);
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-surface);
    max-width: 600px;
    width: 100%;
    margin: 0 auto;
  }

  .status-text {
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
  }

  .status-text pre {
    margin: 0;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.4;
    color: var(--text-secondary);
    white-space: pre;        /* keep it identical to the terminal; scroll if too wide */
  }

  .muted {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }
</style>
