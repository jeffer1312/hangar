<script lang="ts">
  import { onMount } from 'svelte';
  import { autoHideDiverges, ownUpdatableServers, readAutoHideAll, saveAutoHideAll, type AutoHideResult } from '@hangar/core';
  import Select from '../Select.svelte';
  import * as m from '../../paraglide/messages';
  import { listOwnServers } from '../../lib/auth';
  import { sessionsStore } from '../../lib/sessionsStore.svelte';
  import { hiddenSection } from '../../lib/hiddenSection.svelte';

  // Uma configuração para todas as máquinas próprias: lê de cada uma, grava em cada uma e mostra
  // onde ficou diferente. Convite e entrada temporária de par ficam de fora (o servidor do outro
  // recusaria com 403).
  const servers = ownUpdatableServers(listOwnServers());
  const labelOf = (id: string) => servers.find((s) => s.id === id)?.label ?? id;

  // null = lendo. `saved` só existe depois de gravar: aí as linhas dizem onde gravou.
  let results = $state<AutoHideResult[] | null>(null);
  let saved = $state<AutoHideResult[] | null>(null);
  let enabled = $state(false);
  let hours = $state(3);
  let busy = $state(false);

  const HOURS = Array.from({ length: 168 }, (_, i) => ({ value: String(i + 1), label: m.session_auto_hide_hours({ n: i + 1 }) }));
  const allFailed = $derived(results !== null && results.every((r) => !r.ok));
  const differs = $derived(results && !allFailed ? autoHideDiverges(results) : []);
  const hiddenCount = $derived(sessionsStore.rows.filter((r) => r.hidden_from_list).length);

  async function load() {
    results = null;
    saved = null;
    const r = await readAutoHideAll(servers);
    // Base = a configuração da maioria; as divergentes aparecem com "Aplicar em todas".
    const off = new Set(autoHideDiverges(r));
    const base = r.find((x) => x.ok && x.config && !off.has(x.serverId))?.config;
    if (base) {
      enabled = base.enabled;
      hours = Math.min(168, Math.max(1, Math.round(base.after_minutes / 60)));
    }
    results = r;
  }

  // Recebe o valor novo: o anterior é lido antes de trocar, para a falha total poder voltar a ele.
  async function save(next: { enabled: boolean; hours: number } = { enabled, hours }) {
    busy = true;
    const before = { enabled, hours };
    enabled = next.enabled;
    hours = next.hours;
    try {
      const r = await saveAutoHideAll(servers, { enabled, after_minutes: hours * 60 });
      saved = r;
      // Nenhuma gravou: os controles voltam ao que as máquinas guardam, e a tela não vira a de erro de leitura.
      if (r.every((x) => !x.ok)) {
        enabled = before.enabled;
        hours = before.hours;
      } else {
        results = r;
      }
    } finally {
      busy = false;
    }
  }

  function seeHidden() {
    hiddenSection.open = true;
    window.location.hash = '#/';
  }

  onMount(() => { if (servers.length) void load(); });
</script>

<div class="ss">
  {#if servers.length === 0}
    <p class="ss-info">{m.lista_nenhum_servidor()}</p>
  {:else if results === null}
    <p class="ss-info" role="status">{m.comum_carregando()}</p>
  {:else if allFailed}
    <ul class="ss-results" role="alert">
      {#each results as r (r.serverId)}
        <li class="erro"><strong>{labelOf(r.serverId)}</strong> {r.error}</li>
      {/each}
    </ul>
    <button class="ss-btn" onclick={load}>{m.busca_tentar_de_novo()}</button>
  {:else}
    <div class="ss-row">
      <label class="ss-label" for="auto-hide-on">{m.session_auto_hide_label()}</label>
      <input id="auto-hide-on" type="checkbox" class="switch" checked={enabled} disabled={busy}
             onchange={(e) => { void save({ enabled: e.currentTarget.checked, hours }); }} />
    </div>
    <div class="ss-row">
      <Select ariaLabel={m.session_auto_hide_label()} value={String(hours)} opcoes={HOURS}
              disabled={busy || !enabled}
              onchange={(v) => { void save({ enabled, hours: Number(v) }); }} />
    </div>

    {#if differs.length > 0}
      <div class="ss-differs" role="status">
        <span>{m.session_auto_hide_differs({ servers: differs.map(labelOf).join(', ') })}</span>
        <button class="ss-btn" disabled={busy} onclick={() => save()}>{m.session_auto_hide_apply_all()}</button>
      </div>
    {/if}

    {#if saved}
      <ul class="ss-results" role="status">
        {#each saved as r (r.serverId)}
          <li class:erro={!r.ok}>
            {r.ok ? m.session_auto_hide_saved_on({ server: labelOf(r.serverId) })
                  : m.session_auto_hide_not_saved_on({ server: labelOf(r.serverId), error: r.error ?? '' })}
          </li>
        {/each}
      </ul>
    {:else}
      <!-- Máquina que não respondeu na leitura: o motivo fica à vista ao lado do nome dela. -->
      {@const failed = results.filter((r) => !r.ok)}
      {#if failed.length}
        <ul class="ss-results">
          {#each failed as r (r.serverId)}
            <li class="erro"><strong>{labelOf(r.serverId)}</strong> {r.error}</li>
          {/each}
        </ul>
      {/if}
    {/if}
  {/if}

  {#if hiddenCount > 0}
    <button class="ss-btn ss-see" onclick={seeHidden}>{m.session_hidden_see({ n: hiddenCount })}</button>
  {/if}
</div>

<style>
  .ss { container-type: inline-size; display: flex; flex-direction: column; gap: var(--space-2); }
  .ss-row {
    display: flex; align-items: center; justify-content: space-between; gap: var(--space-4);
    padding: var(--space-3) 0; border-bottom: 1px solid var(--border-subtle);
  }
  .ss-label { color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .ss-info { margin: 0; color: var(--text-muted); font-size: var(--text-sm); }
  .ss-differs {
    display: flex; align-items: center; justify-content: space-between; gap: var(--space-3);
    color: var(--text-secondary); font-size: var(--text-xs);
  }
  .ss-results { margin: 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 4px; }
  .ss-results li { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.35; }
  .ss-results li.erro { color: var(--error); }
  .ss-btn {
    align-self: flex-start; min-height: 36px; padding: 0 var(--space-3);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    background: transparent; color: var(--text-primary); font-size: var(--text-sm);
  }
  .ss-btn:disabled { opacity: 0.5; }
  .ss-see { margin-top: var(--space-3); }
  @container (max-width: 360px) {
    .ss-differs { flex-direction: column; align-items: flex-start; }
  }
</style>
