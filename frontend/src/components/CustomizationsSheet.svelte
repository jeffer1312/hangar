<script lang="ts">
  import { untrack } from 'svelte';
  import { getClaudeCustomizationsForServer, type ClaudeCustomizationDelta, type ClaudeCustomizations, type Server } from '@hangar/core';
  import BottomSheet from './BottomSheet.svelte';
  import {
    emptyDelta, looseSkillEnabled, pluginEnabled, pluginSkillEnabled, setLooseSkill, setPlugin, setPluginSkill,
  } from '../lib/customizations';
  import * as m from '../paraglide/messages';

  // Plugins e skills só desta sessão (create/customizations.rs): rascunho na folha, vale ao Aplicar.
  interface Props {
    open: boolean;
    server: Server | null;
    cwd: string | null;
    configDir: string | null;
    applied: ClaudeCustomizationDelta;
    onApply: (delta: ClaudeCustomizationDelta, catalog: ClaudeCustomizations) => void;
    onClose: () => void;
  }
  let { open, server, cwd, configDir, applied, onApply, onClose }: Props = $props();

  let catalog = $state<ClaudeCustomizations | null>(null);
  let loadError = $state('');
  let loading = $state(false);
  let draft = $state<ClaudeCustomizationDelta>(emptyDelta());
  let tab = $state<'plugins' | 'skills'>('plugins');
  let query = $state('');
  let expanded = $state<Record<string, boolean>>({});
  let seq = 0;

  async function load() {
    const mine = ++seq;
    if (!server || !cwd) return;
    loading = true;
    loadError = '';
    try {
      const c = await getClaudeCustomizationsForServer(server, cwd, configDir);
      if (mine === seq) catalog = c;
    } catch (e) {
      if (mine === seq) loadError = e instanceof Error ? e.message : String(e);
    } finally {
      if (mine === seq) loading = false;
    }
  }

  // O catálogo é da máquina, conta e pasta: trocar qualquer um relê.
  $effect(() => {
    void [server?.id, cwd, configDir];
    untrack(() => { catalog = null; });
  });
  $effect(() => {
    if (!open) return;
    untrack(() => {
      draft = structuredClone($state.snapshot(applied));
      query = '';
      if (!catalog) void load();
    });
  });

  const q = $derived(query.trim().toLowerCase());
  const match = (name: string, desc?: string) => !q || `${name} ${desc ?? ''}`.toLowerCase().includes(q);
  const plugins = $derived((catalog?.plugins ?? []).filter((p) => match(p.name, p.description) || p.skills.some((s) => match(s.name, s.description))));
  const skills = $derived((catalog?.skills ?? []).filter((s) => match(s.name, s.description)));
</script>

<BottomSheet {open} {onClose} ariaLabel={m.native_create_customizations_title()}>
  <div class="sheet">
    <h2 class="title">{m.native_create_customizations_title()}</h2>
    <p class="help">{m.native_create_customizations_session_only()}</p>

    {#if loading && !catalog}
      <p class="help" role="status">{m.comum_carregando()}</p>
    {:else if loadError}
      <p class="help err" role="alert">{m.native_create_customizations_failed({ reason: loadError })}</p>
      <p class="help">{m.native_create_customizations_failure_help()}</p>
      <button type="button" class="btn" onclick={() => load()}>{m.native_create_try_again()}</button>
    {:else if catalog}
      {#each catalog.warnings ?? [] as w (w)}<p class="help warn">{w}</p>{/each}
      <div class="tabs" role="tablist">
        <button type="button" role="tab" class="tab" class:on={tab === 'plugins'} aria-selected={tab === 'plugins'}
          onclick={() => (tab = 'plugins')}>{m.native_create_customizations_plugins()}</button>
        <button type="button" role="tab" class="tab" class:on={tab === 'skills'} aria-selected={tab === 'skills'}
          onclick={() => (tab = 'skills')}>{m.native_create_customizations_skills()}</button>
      </div>
      <input class="search" type="search" bind:value={query} placeholder={m.native_create_customizations_search()}
        aria-label={m.native_create_customizations_search()} />

      <ul class="list">
        {#if tab === 'plugins'}
          {#each plugins as p (p.id)}
            {@const on = pluginEnabled(draft, p)}
            <li>
              <label class="row">
                <input type="checkbox" checked={on} onchange={(e) => (draft = setPlugin(draft, catalog!, p.id, e.currentTarget.checked))} />
                <span class="txt"><b>{p.name}</b>{#if p.description}<small>{p.description}</small>{/if}</span>
              </label>
              {#if p.skills.length}
                <button type="button" class="expand" aria-expanded={!!expanded[p.id]}
                  onclick={() => (expanded = { ...expanded, [p.id]: !expanded[p.id] })}>
                  {m.native_create_customizations_expand({ name: p.name })} ({p.skills.length})
                </button>
                {#if expanded[p.id]}
                  <ul class="sub">
                    {#each p.skills as s (s.name)}
                      <li>
                        <label class="row" class:off={!on || !s.enabled || s.blocked}>
                          <input type="checkbox" checked={on && pluginSkillEnabled(draft, s)} disabled={!on || !s.enabled || s.blocked}
                            onchange={(e) => (draft = setPluginSkill(draft, catalog!, p.id, s.name, e.currentTarget.checked))} />
                          <span class="txt"><span>{s.name}</span>
                            {#if s.blocked || !s.enabled}<small>{m.native_create_customizations_inherited()}</small>
                            {:else if s.description}<small>{s.description}</small>{/if}</span>
                        </label>
                      </li>
                    {/each}
                  </ul>
                {/if}
              {/if}
            </li>
          {:else}
            <li class="help">{q ? m.native_create_customizations_no_results() : m.native_create_customizations_empty()}</li>
          {/each}
        {:else}
          {#each skills as s (s.name)}
            <li>
              <label class="row" class:off={s.blocked}>
                <input type="checkbox" checked={looseSkillEnabled(draft, s)} disabled={s.blocked}
                  onchange={(e) => (draft = setLooseSkill(draft, catalog!, s.name, e.currentTarget.checked))} />
                <span class="txt"><span>{s.name}</span>
                  {#if s.blocked}<small>{m.native_create_customizations_inherited()}</small>
                  {:else if s.description}<small>{s.description}</small>{/if}</span>
              </label>
            </li>
          {:else}
            <li class="help">{q ? m.native_create_customizations_no_results() : m.native_create_customizations_empty()}</li>
          {/each}
        {/if}
      </ul>

      <p class="help">{m.native_create_customizations_hooks_help()}</p>
      <div class="actions">
        <button type="button" class="btn" onclick={() => (draft = emptyDelta())}>{m.native_create_customizations_restore()}</button>
        <button type="button" class="btn primary" onclick={() => { onApply(draft, catalog!); onClose(); }}>
          {m.native_create_customizations_apply()}
        </button>
      </div>
    {/if}
  </div>
</BottomSheet>

<style>
  .sheet { padding: var(--space-2) var(--space-4) calc(env(safe-area-inset-bottom) + var(--space-4)); display: flex; flex-direction: column; gap: var(--space-2); }
  .title { margin: 0; font-size: var(--text-base); font-weight: var(--fw-semibold); color: var(--text-primary); }
  .help { margin: 0; font-size: var(--text-xs); color: var(--text-muted); }
  .help.err { color: var(--error); }
  .help.warn { color: var(--warning); }
  .tabs { display: flex; gap: 4px; }
  .tab { flex: 1; min-height: 44px; border: 0; border-radius: var(--radius-md); background: transparent; color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer; }
  .tab.on { background: var(--accent-dim); color: var(--text-primary); }
  .search { min-height: 44px; padding: 0 12px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-inset); color: var(--text-primary); font-size: var(--text-sm); }
  .list, .sub { list-style: none; margin: 0; padding: 0; }
  .list { max-height: 50vh; overflow-y: auto; }
  .sub { padding-left: 32px; }
  .row { display: flex; align-items: center; gap: 12px; min-height: 44px; padding: 4px 0; cursor: pointer; }
  .row.off { opacity: 0.6; }
  .row input { width: 22px; height: 22px; flex-shrink: 0; accent-color: var(--accent); }
  .txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; font-size: var(--text-sm); color: var(--text-primary); }
  .txt small { font-size: var(--text-xs); color: var(--text-muted); }
  .expand { min-height: 44px; padding: 0 0 0 34px; border: 0; background: transparent; color: var(--accent); font-size: var(--text-xs); text-align: left; cursor: pointer; }
  .actions { display: flex; gap: var(--space-2); }
  .btn { flex: 1; min-height: 44px; border: 1px solid var(--fill-subtle); border-radius: var(--radius-md); background: transparent; color: var(--text-primary); font-size: var(--text-sm); cursor: pointer; }
  .btn.primary { background: var(--accent-dim); }
</style>
