<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import {
    CONFIG_SYNC_ITEMS, applyConfigSyncForServer, configSyncItemLabel,
    configSyncRows, configSyncWarningText, diffManifests, getConfigSyncBundleForServer,
    getConfigSyncManifestForServer, translateConfigSyncTextsForServer,
    type ConfigSyncDiff, type ConfigSyncGroup, type ConfigSyncItem, type ConfigSyncItemResult,
    type ConfigSyncManifest, type ConfigSyncReport, type ConfigSyncRow,
  } from '@hangar/core';
  import { listServers, onServersChanged, type Server } from '../../lib/auth';
  import ConfirmSheet from '../ConfirmSheet.svelte';
  import { localeAtual } from '../../lib/locale';
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
  let base = $state<ConfigSyncManifest | null>(null);
  // Entradas marcadas por item escolhível. Sem comparação não há escolha: o item vai inteiro.
  let picked = $state<Partial<Record<ConfigSyncItem, string[]>>>({});
  // Descrição original → no idioma da tela. Chega depois da prévia; até lá vale o original.
  let translated = $state<Record<string, string>>({});
  let translateError = $state('');
  let translating = $state(false);
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

  // Prévia feita com outra origem ou outros itens decidiria a sobrescrita errada.
  function clearResults() {
    diffs = {};
    reports = {};
    base = null;
    picked = {};
    translated = {};
    translateError = '';
    translating = false;
    run++;
  }

  // Tradução que chega depois de outra comparação (ou de trocar a origem) é descartada.
  let run = 0;
  async function translateDescriptions(from: Server, manifest: ConfigSyncManifest) {
    const mine = run;
    const texts = [...new Set(Object.values(manifest.items).flatMap((i) => Object.values(i?.descriptions ?? {})))];
    if (!texts.length) return;
    translating = true;
    try {
      const r = await translateConfigSyncTextsForServer(from, texts, localeAtual(), controller.signal);
      if (mine !== run) return;
      translated = Object.fromEntries(texts.map((t, i) => [t, r.texts[i] ?? t]));
      translateError = r.error;
    } catch (e) {
      // 404 = Hangar de lá sem a tradução: os originais já estão na tela, não é erro.
      if (mine === run && (e as { status?: number } | null)?.status !== 404) {
        translateError = e instanceof Error ? e.message : String(e);
      }
    } finally {
      if (mine === run) translating = false;
    }
  }

  const isChange = (r: ConfigSyncRow) => r.status === 'added' || r.status === 'changed';
  const GROUPS: { group: ConfigSyncGroup; title?: () => string; hint?: () => string }[] = [
    { group: 'settings', title: m.shared_config_group_settings, hint: m.shared_config_group_settings_hint },
    { group: 'files', title: m.shared_config_group_files },
    { group: 'entries' },
    { group: 'refs', title: m.shared_config_group_refs, hint: m.shared_config_group_refs_hint },
  ];

  const okDiffs = $derived(targets.map((t) => diffs[t.id]).filter((d) => d !== undefined && typeof d !== 'string'));
  const preview = $derived(items.filter((item) => okDiffs.some((d) => d[item])).map((item) => {
    const meta = base?.items[item];
    const descriptions = Object.fromEntries(Object.entries(meta?.descriptions ?? {}).map(([k, v]) => [k, translated[v] ?? v]));
    const rows = configSyncRows(item, okDiffs.flatMap((d) => d[item] ?? []), { labels: meta?.labels, descriptions });
    const count = (s: ConfigSyncRow['status']) => rows.filter((r) => r.status === s).length;
    return {
      item, rows, changes: rows.filter(isChange),
      same: rows.filter((r) => r.status === 'same'), onlyTarget: rows.filter((r) => r.status === 'onlyTarget'),
      line: m.shared_config_diff_line({ added: count('added'), changed: count('changed'), same: count('same'), onlyTarget: count('onlyTarget') }),
    };
  }).filter((p) => p.rows.length > 0));

  function togglePick(item: ConfigSyncItem, key: string) {
    picked = { ...picked, [item]: toggle(picked[item] ?? [], key) };
  }

  function pickAll(item: ConfigSyncItem, rows: ConfigSyncRow[], on: boolean) {
    picked = { ...picked, [item]: on ? rows.filter((r) => r.selectable).map((r) => r.key) : [] };
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
    clearResults();
    busy = m.shared_config_comparing();
    try {
      let from: ConfigSyncManifest;
      try {
        from = await getConfigSyncManifestForServer(origin, controller.signal);
      } catch (e) {
        error = errorText(e, origin);
        return;
      }
      const next: typeof diffs = {};
      await Promise.all(targets.map(async (t) => {
        try {
          next[t.id] = diffManifests(from, await getConfigSyncManifestForServer(t, controller.signal), items);
        } catch (e) {
          next[t.id] = errorText(e, t);
        }
      }));
      diffs = next;
      base = from;
      void translateDescriptions(origin, from);
      // Começa marcado o que o envio muda; o igual não precisa ir.
      picked = Object.fromEntries(preview.map((p) => [p.item, p.changes.filter((r) => r.selectable).map((r) => r.key)]));
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
        bundle = await getConfigSyncBundleForServer(origin, items, controller.signal, $state.snapshot(picked));
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
      <select bind:value={originId} disabled={!!busy} onchange={clearResults}>
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
        <label class="linha"><input type="checkbox" class="switch" checked={items.includes(item)} onchange={() => { items = toggle(items, item); clearResults(); }} />{configSyncItemLabel(item)}</label>
      {/each}
    </fieldset>

    {#if !ready}<p>{m.shared_config_pick()}</p>{/if}
    <div class="acoes">
      <button type="button" disabled={!ready || !!busy} aria-busy={!!busy} onclick={compare}>{m.shared_config_compare()}</button>
      <button type="button" class="primaria" disabled={!ready || !!busy} onclick={() => (confirming = true)}>{m.shared_config_send()}</button>
    </div>
    {#if busy}<p role="status">{busy}</p>{/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}

    {#if translating}<p role="status" class="dica">{m.shared_config_translating()}</p>{/if}
    {#if translateError}<p class="dica" role="status">{m.shared_config_translate_failed({ error: translateError })}</p>{/if}
    {#each preview as p (p.item)}
      {@const choice = picked[p.item]}
      {@const selectable = choice !== undefined}
      <details class="bloco" open={preview.length === 1 && p.changes.length > 0}>
        <summary>
          <span class="item">{configSyncItemLabel(p.item)}</span>
          <span class="estado">{p.line}</span>
          {#if selectable}
            <span class="marcados">{m.shared_config_picked({ picked: choice.length, total: p.changes.filter((r) => r.selectable).length })}</span>
          {/if}
        </summary>
        {#if selectable && p.changes.length}
          <div class="acoes-lista">
            <button type="button" class="link" disabled={!!busy} onclick={() => pickAll(p.item, p.changes, true)}>{m.shared_config_pick_all()}</button>
            <button type="button" class="link" disabled={!!busy} onclick={() => pickAll(p.item, p.changes, false)}>{m.shared_config_pick_none()}</button>
          </div>
        {/if}
        {#each GROUPS as g (g.group)}
          {@const rows = p.changes.filter((r) => r.group === g.group)}
          {#if rows.length}
            <div class="grupo">
              {#if g.title}<p class="grupo-titulo">{g.title()}</p>{/if}
              {#if g.hint && selectable}<p class="dica">{g.hint()}</p>{/if}
              <ul class="linhas">
                {#each rows as r (r.key)}
                  <li>
                    <label class="entrada">
                      <input type="checkbox" checked={choice?.includes(r.key) ?? false} disabled={!!busy || !r.selectable} onchange={() => togglePick(p.item, r.key)} />
                      <span class="corpo">
                        <span class="topo">
                          <span class="nome">{r.name}</span>
                          <span class="selo" class:novo={r.status === 'added'}>{r.status === 'added' ? m.shared_config_row_added() : m.shared_config_row_changed()}</span>
                        </span>
                        {#if r.description}<span class="descricao">{r.description}</span>{/if}
                        {#each r.scripts as s (s.name)}
                          <span class="script"><span class="nome">{s.name}</span>{#if s.description}<span class="descricao">{s.description}</span>{/if}</span>
                        {/each}
                      </span>
                    </label>
                  </li>
                {/each}
              </ul>
            </div>
          {/if}
        {/each}
        {#if p.same.length}
          <details class="resto">
            <summary>{m.shared_config_same({ count: p.same.length })}</summary>
            <p class="nomes">{p.same.map((r) => r.name).join(' · ')}</p>
          </details>
        {/if}
        {#if p.onlyTarget.length}
          <details class="resto">
            <summary>{m.shared_config_only_target({ count: p.onlyTarget.length })}</summary>
            <p class="nomes">{p.onlyTarget.map((r) => r.name).join(' · ')}</p>
          </details>
        {/if}
      </details>
    {/each}

    {#each targets as t (t.id)}
      {@const report = reports[t.id]}
      {@const diff = diffs[t.id]}
      {#if report !== undefined || typeof diff === 'string' || (diff && okDiffs.length > 1)}
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
  .bloco { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-3) var(--space-4); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); }
  .bloco > summary { display: flex; flex-wrap: wrap; align-items: baseline; column-gap: var(--space-3); row-gap: var(--space-1); cursor: pointer; font-size: var(--text-sm); list-style-position: outside; }
  .marcados { color: var(--accent); font-variant-numeric: tabular-nums; }
  .acoes-lista { display: flex; gap: var(--space-4); margin-bottom: var(--space-2); }
  .link { padding: 0; border: 0; background: transparent; color: var(--accent); font-size: var(--text-xs); }
  .grupo { display: flex; flex-direction: column; gap: var(--space-1); }
  .grupo + .grupo { margin-top: var(--space-4); }
  .grupo-titulo { color: var(--text-muted); font-size: var(--text-xs); font-weight: 600; }
  .dica { color: var(--text-muted); font-size: var(--text-xs); }
  .linhas { gap: var(--space-1); margin-top: var(--space-1); }
  .entrada { display: flex; align-items: flex-start; gap: var(--space-3); min-width: 0; padding: var(--space-2) 0; cursor: pointer; }
  .entrada input { flex: none; margin: 3px 0 0; }
  .corpo { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .topo { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .descricao { color: var(--text-secondary); font-size: var(--text-xs); line-height: 1.45; max-width: 72ch;
    display: -webkit-box; -webkit-line-clamp: 2; line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
  .script { display: flex; flex-direction: column; gap: 1px; margin-top: var(--space-1); padding-left: var(--space-3); border-left: 1px solid var(--border-subtle); }
  .nome { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-primary); overflow-wrap: anywhere; }
  .selo { padding: 0 var(--space-2); border-radius: var(--radius-full); background: color-mix(in oklch, var(--warning) 16%, transparent); color: var(--warning); font-size: var(--text-xs); line-height: 1.6; }
  .selo.novo { background: var(--accent-dim); color: var(--accent); }
  .resto { margin-top: var(--space-3); font-size: var(--text-xs); }
  .resto summary { color: var(--text-secondary); cursor: pointer; }
  .resto .nomes { margin-top: var(--space-2); font-family: var(--font-mono); font-size: var(--text-xs); }
  select:focus-visible, button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
</style>
