<script lang="ts">
  import { onMount } from 'svelte';
  import type { PresenceMode } from '@hangar/core';
  import SegmentedPicker from './SegmentedPicker.svelte';
  import { getActiveId, listOwnServers, type Server } from '../lib/auth';
  import { loadPresence, setPresenceAll, shownPresence, type PresenceResult } from '../lib/presence';
  import * as m from '../paraglide/messages';

  // `preferred`: a máquina desta tela (null = a ativa). O modo vale para todas; ela só decide qual
  // leitura aparece e de quem é o aviso do app do computador.
  let { preferred = null }: { preferred?: Server | null } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let results = $state<PresenceResult[]>([]);

  const preferredId = $derived(preferred?.id ?? getActiveId());
  const shown = $derived(shownPresence(results, preferredId));
  const preferredPresence = $derived(results.find((r) => r.server.id === preferredId)?.presence ?? null);
  const label = (mode: PresenceMode) => (mode === 'pc' ? m.push_presence_pc() : m.push_presence_away());
  const options = $derived([
    { v: 'pc' as const, label: m.push_presence_pc(), aria: m.push_presence_pc() },
    { v: 'away' as const, label: m.push_presence_away(), aria: m.push_presence_away() },
  ]);

  onMount(async () => {
    results = await loadPresence(listOwnServers());
    loading = false;
  });

  async function pick(mode: PresenceMode) {
    if (saving || mode === shown?.mode) return;
    saving = true;
    try {
      results = await setPresenceAll(listOwnServers(), mode);
    } finally {
      saving = false;
    }
  }
</script>

<!-- Nenhuma máquina com o recurso (todas antigas, ou nenhuma do dono): o controle não aparece. -->
{#if loading}
  <p class="pp-msg">{m.comum_carregando()}</p>
{:else if results.length}
  <div class="pp">
    <div class="pp-row">
      <strong>{m.push_presence_title()}</strong>
      {#if shown}
        <SegmentedPicker value={shown.mode} {options} ariaLabel={m.push_presence_title()}
                         describedBy="pp-help" disabled={saving} onPick={(v) => void pick(v)} />
      {/if}
    </div>
    <div id="pp-help" class="pp-help">
      <p>{m.push_presence_pc_help()}</p>
      <p>{m.push_presence_away_help()}</p>
    </div>
    {#if preferredPresence && !preferredPresence.desktop_alive}
      <p class="pp-msg">{m.push_presence_desktop_closed()}</p>
    {/if}
    {#each results as r (r.server.id)}
      {#if r.error}
        <p class="pp-msg erro" role="alert">{m.push_presence_server_error({ server: r.server.label, error: r.error })}</p>
      {:else if r.presence && shown && r.presence.mode !== shown.mode}
        <p class="pp-msg">{m.push_presence_differs({ server: r.server.label, mode: label(r.presence.mode) })}</p>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .pp { padding: var(--space-2) var(--space-4); }
  .pp-row {
    display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap;
    gap: var(--space-3); font-size: var(--text-sm); color: var(--text-primary);
  }
  .pp-help p, .pp-msg { margin: var(--space-1) 0 0; font-size: var(--text-xs); line-height: 1.45; color: var(--text-muted); }
  .pp-msg { padding: 0 var(--space-4); }
  .pp .pp-msg { padding: 0; }
  .pp-msg.erro { color: var(--error); }
</style>
