<script lang="ts">
  import type { OrqWatchdog } from '@hangar/core';
  import { conductorChip, conductorChipLabel } from '../lib/orqConductor';

  let { watchdog, finished = false }: { watchdog: OrqWatchdog | null | undefined; finished?: boolean } = $props();
  const chip = $derived(conductorChip(watchdog, finished));
</script>

{#if chip}
  <span class="conductor-chip k-{chip.kind}" class:alarm={chip.kind === 'stopped' && chip.alarm}>
    {conductorChipLabel(chip)}
  </span>
{/if}

<style>
  .conductor-chip {
    min-width: 0; max-width: 100%;
    padding: 2px 8px; border-radius: 999px;
    background: var(--surface-raised); color: var(--text-secondary);
    font-size: var(--text-xs); font-weight: 600;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .conductor-chip.k-alive { color: var(--success); }
  .conductor-chip.alarm { color: var(--warning); }
</style>
