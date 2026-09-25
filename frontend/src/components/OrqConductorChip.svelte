<script lang="ts">
  import * as m from '../paraglide/messages';
  import { relativeTime, type OrqWatchdog } from '@hangar/core';
  import { conductorChip } from '../lib/orqConductor';

  let { watchdog, finished = false }: { watchdog: OrqWatchdog | null | undefined; finished?: boolean } = $props();
  const chip = $derived(conductorChip(watchdog, finished));
</script>

{#if chip}
  <span class="conductor-chip k-{chip.kind}" class:alarm={chip.kind === 'stopped' && chip.alarm}>
    {#if chip.kind === 'alive'}
      {chip.lastCycle === null
        ? m.orq_conductor_alive_no_heartbeat()
        : m.orq_conductor_alive({ when: relativeTime(chip.lastCycle), who: chip.watching.join(', ') })}
    {:else if chip.kind === 'stopped'}
      {chip.lastCycle === null ? m.orq_conductor_stopped() : m.orq_conductor_stopped_since({ when: relativeTime(chip.lastCycle) })}
    {:else if chip.kind === 'none'}
      {m.orq_conductor_none()}
    {:else}
      {m.orq_conductor_unavailable()}
    {/if}
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
