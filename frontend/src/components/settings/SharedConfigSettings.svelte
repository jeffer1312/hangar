<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import {
    CONFIG_SYNC_ITEMS, applyConfigSyncForServer, configSyncItemLabel, configSyncWarningText,
    diffManifests, getConfigSyncBundleForServer, getConfigSyncManifestForServer,
    type ConfigSyncDiff, type ConfigSyncItem, type ConfigSyncItemResult, type ConfigSyncManifest,
    type ConfigSyncReport,
  } from '@hangar/core';
  import { listServers, onServersChanged, type Server } from '../../lib/auth';
  import ConfirmSheet from '../ConfirmSheet.svelte';
  import * as m from '../../paraglide/messages';

  // O servidor do seletor do modal é só a origem sugerida: a tela fala com várias máquinas, e o
  // navegador leva o pacote de uma para as outras.
  let { server }: { server: Server } = $props();

  let serverVersion = $state(0);
  $effect(() => onServersChanged(() => serverVersion++));
  const servers = $derived.by(() => {
    serverVersion;   // listServers() não é reativo
    return listServers();
  });

  // Só o valor inicial: trocar o servidor do modal remonta a tela pelo `{#key}`.
  let originId = $state(untrack(() => server.id));
  let targetIds = $state<string[]>([]);
  let items = $state<ConfigSyncItem[]>([...CONFIG_SYNC_ITEMS]);
  let busy = $state('');
  let error = $state('');
  let confirming = $state(false);
  let diffs = $state<Record<string, Partial<Record<ConfigSyncItem, ConfigSyncDiff>> | string>>({});
  let reports = $state<Record<string, ConfigSyncReport | string>>({});
  const controller = new AbortController();
  onDestroy(() => controller.abort());

  const origin = $derived(servers.find((s) => s.id === originId) ?? null);
  const candidates = $derived(servers.filter((s) => s.id !== originId));
  const targets = $derived(candidates.filter((s) => targetIds.includes(s.id)));
  const allTargets = $derived(candidates.length > 0 && candidates.every((s) => targetIds.includes(s.id)));
  const ready = $derived(!!origin && targets.length > 0 && items.length > 0);

  function toggle<T>(list: T[], value: T): T[] {
    return list.includes(value) ? list.filter((v) => v !== value) : [...list, value];
  }

  function toggleAllTargets() {
    targetIds = allTargets ? [] : candidates.map((s) => s.id);
  }

  // 404 na rota = Hangar de lá ainda não tem a feature; o resto mostra a mensagem do servidor.
  function errorText(e: unknown, s: Server): string {
    if ((e as { status?: number } | null)?.status === 404) return m.shared_config_outdated({ machine: s.label });
    return m.shared_config_error({ machine: s.label, error: e instanceof Error ? e.message : String(e) });
  }

  function statusText(r: ConfigSyncItemResult): string {
    if (r.status === 'applied') return m.shared_config_status_applied();
    if (r.status === 'failed') return m.shared_config_status_failed();
    return m.shared_config_status_same();
  }

  async function compare() {
    if (!origin) return;
    error = '';
    diffs = {};
    reports = {};
    busy = m.shared_config_comparing();
    try {
      let base: ConfigSyncManifest;
      try {
        base = await getConfigSyncManifestForServer(origin, controller.signal);
      } catch (e) {
        error = errorText(e, origin);
        return;
      }
      const next: typeof diffs = {};
      await Promise.all(targets.map(async (t) => {
        try {
          next[t.id] = diffManifests(base, await getConfigSyncManifestForServer(t, controller.signal), items);
        } catch (e) {
          next[t.id] = errorText(e, t);
        }
      }));
      diffs = next;
    } finally {
      busy = '';
    }
  }

  // O pacote sai UMA vez da origem e vai, em sequência, para cada destino.
  async function send() {
    if (!origin) return;
    error = '';
    reports = {};
    try {
      busy = m.shared_config_packing({ machine: origin.label });
      let bundle: Blob;
      try {
        bundle = await getConfigSyncBundleForServer(origin, items, controller.signal);
      } catch (e) {
        error = errorText(e, origin);
        return;
      }
      for (const t of targets) {
        busy = m.shared_config_sending({ machine: t.label });
        try {
          reports[t.id] = await applyConfigSyncForServer(t, items, bundle, controller.signal);
        } catch (e) {
          reports[t.id] = errorText(e, t);
        }
      }
    } finally {
      busy = '';
    }
  }
</script>

<section class="shared">
  <p class="rotulo">{m.shared_config_title()}</p>
  <p>{m.shared_config_intro()}</p>
  {#if servers.length < 2}
    <p role="status">{m.shared_config_need_two()}</p>
  {:else}
    <label class="campo">
      <span class="rotulo">{m.shared_config_origin()}</span>
      <select bind:value={originId} disabled={!!busy}>
        {#each servers as s (s.id)}<option value={s.id}>{s.label}</option>{/each}
      </select>
    </label>

    <fieldset disabled={!!busy}>
      <legend class="rotulo">{m.shared_config_targets()}</legend>
      <label class="linha"><input type="checkbox" class="switch" checked={allTargets} onchange={toggleAllTargets} />{m.shared_config_all_targets()}</label>
      {#each candidates as s (s.id)}
        <label class="linha"><input type="checkbox" class="switch" checked={targetIds.includes(s.id)} onchange={() => (targetIds = toggle(targetIds, s.id))} />{s.label}</label>
      {/each}
    </fieldset>

    <fieldset disabled={!!busy}>
      <legend class="rotulo">{m.shared_config_items()}</legend>
      {#each CONFIG_SYNC_ITEMS as item (item)}
        <label class="linha"><input type="checkbox" class="switch" checked={items.includes(item)} onchange={() => (items = toggle(items, item))} />{configSyncItemLabel(item)}</label>
      {/each}
    </fieldset>

    {#if !ready}<p>{m.shared_config_pick()}</p>{/if}
    <div class="acoes">
      <button type="button" disabled={!ready || !!busy} aria-busy={!!busy} onclick={compare}>{m.shared_config_compare()}</button>
      <button type="button" class="primaria" disabled={!ready || !!busy} onclick={() => (confirming = true)}>{m.shared_config_send()}</button>
    </div>
    {#if busy}<p role="status">{busy}</p>{/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}

    {#each targets as t (t.id)}
      {@const report = reports[t.id]}
      {@const diff = diffs[t.id]}
      {#if report !== undefined || diff !== undefined}
        <div class="destino">
          <p class="rotulo">{t.label}</p>
          {#if typeof report === 'string'}
            <p class="error" role="alert">{report}</p>
          {:else if report}
            <ul>
              {#each Object.entries(report.items) as [item, result] (item)}
                {#if result}
                  <li>
                    <span class="item">{configSyncItemLabel(item as ConfigSyncItem)}</span>
                    <span class="estado" class:falhou={result.status === 'failed'}>{statusText(result)}</span>
                    {#if result.changed.length}<span class="nomes">{result.changed.join(', ')}</span>{/if}
                    {#each result.warnings as w, i (i)}<span class="nomes">{configSyncWarningText(w)}</span>{/each}
                  </li>
                {/if}
              {/each}
            </ul>
            {#if report.backup}<p>{m.shared_config_backup({ path: report.backup })}</p>{/if}
          {:else if typeof diff === 'string'}
            <p class="error" role="alert">{diff}</p>
          {:else if diff}
            <ul>
              {#each items as item (item)}
                {@const d = diff[item]}
                {#if d}
                  <li>
                    <span class="item">{configSyncItemLabel(item)}</span>
                    <span class="estado">{m.shared_config_diff_line({ added: d.added.length, changed: d.changed.length, same: d.same.length, onlyTarget: d.onlyTarget.length })}</span>
                    {#if d.added.length || d.changed.length}<span class="nomes">{[...d.added, ...d.changed].join(', ')}</span>{/if}
                  </li>
                {/if}
              {/each}
            </ul>
          {/if}
        </div>
      {/if}
    {/each}
  {/if}
</section>

<ConfirmSheet open={confirming} title={m.shared_config_confirm_title()}
  message={m.shared_config_confirm_message({ origin: origin?.label ?? '', targets: targets.map((t) => t.label).join(', ') })}
  confirmLabel={m.shared_config_confirm_label()} onConfirm={() => { void send(); }} onClose={() => (confirming = false)} />

<style>
  .shared { display: flex; flex-direction: column; gap: var(--space-4); container-type: inline-size; }
  p { margin: 0; color: var(--text-secondary); font-size: var(--text-sm); line-height: 1.5; }
  .rotulo {
    color: var(--text-muted); font-size: var(--label-size); font-weight: var(--label-weight);
    text-transform: uppercase; letter-spacing: var(--label-tracking);
  }
  fieldset { display: flex; flex-direction: column; gap: var(--space-2); margin: 0; padding: 0; border: 0; min-width: 0; }
  .campo { display: flex; flex-direction: column; gap: var(--space-2); }
  .linha { display: flex; align-items: center; gap: var(--space-3); color: var(--text-primary); font-size: var(--text-sm); }
  select { padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-inset); color: var(--text-primary); font: inherit; }
  .acoes { display: flex; flex-wrap: wrap; gap: var(--space-3); }
  button { padding: var(--space-3) var(--space-4); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-raised); color: var(--text-primary); font: inherit; font-size: var(--text-sm); cursor: pointer; }
  .primaria { border-color: var(--accent); }
  button:disabled { opacity: .6; cursor: default; }
  .destino { display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); }
  ul { margin: 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: var(--space-2); }
  li { display: flex; flex-wrap: wrap; column-gap: var(--space-3); font-size: var(--text-sm); }
  .item { color: var(--text-primary); font-weight: 600; }
  .estado { color: var(--text-secondary); }
  .falhou, .error { color: var(--error); }
  .nomes { flex-basis: 100%; color: var(--text-muted); overflow-wrap: anywhere; }
  select:focus-visible, button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
</style>
