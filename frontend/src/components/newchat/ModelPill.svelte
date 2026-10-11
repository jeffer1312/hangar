<script lang="ts">
  import { onMount } from 'svelte';
  import { providerName, SESSION_PROVIDERS } from '@hangar/core';
  import { quotaFeed } from '../../lib/quotaFeed.svelte';
  import { faixaDeCota } from '../../lib/cota';
  import BottomSheet from '../BottomSheet.svelte';
  import ProviderGlyph from '../icons/ProviderGlyph.svelte';
  import { valorModelo } from '../../lib/modelosPorConta';
  import type { NewChatDraft } from '../../lib/newChatDraft.svelte';
  import * as m from '../../paraglide/messages';

  // Pílula de modelo dentro do compositor e a folha dela, na ordem do menu do nativo
  // (choices.rs render_model_menu): agentes, Provedor e conta ChatGPT, busca, modelos, raciocínio,
  // Fast, Contexto estendido (1M) e "Usar como padrão".
  let { draft, disabled = false }: { draft: NewChatDraft; disabled?: boolean } = $props();
  let open = $state(false);
  let query = $state('');
  onMount(() => {
    quotaFeed.retain();
    return () => quotaFeed.release();
  });
  $effect(() => { quotaFeed.setServidor(draft.server); });
  $effect(() => { if (open) query = ''; });

  const models = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return draft.models.filter((mod) => mod.id !== 'default'
      && (!q || `${mod.id} ${mod.name ?? ''}`.toLowerCase().includes(q)));
  });
  const engineNames = $derived(Object.keys(draft.engines));
  // Cota da conta ChatGPT escolhida: a credencial do proxy é a mesma conta Codex do /api/cotas.
  const proxyQuota = $derived.by(() => {
    const cred = draft.proxyAccounts.find((a) => a.account === draft.engineAccount)?.credential_id;
    const conta = cred ? faixaDeCota(quotaFeed.contas)?.find((c) => c.id === cred) : null;
    return conta?.janelas.map((j) => `${j.rotulo} ${Math.round(j.pct)}%`).join(' · ') ?? '';
  });
  const label = $derived(models.find((mod) => valorModelo(mod) === draft.model)?.name
    ?? (draft.model || providerName(draft.provider)));
</script>

<button type="button" class="pill" aria-label={`${m.native_create_model()}: ${label}`} {disabled} onclick={() => (open = true)}>
  <ProviderGlyph provider={draft.provider} size={16} />
  <span class="name">{label}</span>
  {#if draft.effort}<span class="effort">{draft.effort}</span>{/if}
</button>

<BottomSheet {open} onClose={() => (open = false)} ariaLabel={m.native_create_model()}>
  <div class="sheet">
    <div class="tabs" role="group" aria-label={m.native_create_provider_aria()}>
      {#each SESSION_PROVIDERS as p (p)}
        <button type="button" class="tab" class:on={draft.provider === p} aria-pressed={draft.provider === p}
          aria-label={providerName(p)} title={providerName(p)}
          disabled={draft.providers[p]?.disponivel === false} onclick={() => draft.setProvider(p)}>
          <ProviderGlyph provider={p} size={18} />
        </button>
      {/each}
    </div>

    {#if draft.provider === 'claude' && engineNames.length > 0}
      <label class="field">
        <span class="muted">{m.native_create_engine()}</span>
        <select value={draft.engine} onchange={(e) => draft.setEngine(e.currentTarget.value)}>
          <option value="">{m.native_create_own_account()}</option>
          {#each engineNames as name (name)}<option value={name}>{name}</option>{/each}
        </select>
      </label>
      {#if draft.engine && draft.proxyAccounts.length > 0}
        <label class="field">
          <span class="muted">{m.native_create_chatgpt_account()}</span>
          <select value={draft.engineAccount} onchange={(e) => draft.setEngineAccount(e.currentTarget.value)}>
            {#each draft.proxyAccounts as a (a.account)}<option value={a.account}>{a.label || a.email || a.account}</option>{/each}
          </select>
          {#if proxyQuota}<small class="muted">{proxyQuota}</small>{/if}
        </label>
      {/if}
    {/if}

    {#if draft.models.length > 0}
      <input class="search" type="search" bind:value={query} placeholder={m.newchat_model_search()} aria-label={m.newchat_model_search()} />
    {/if}

    {#if draft.modelsLoading}
      <p class="muted" role="status">{m.comum_carregando()}</p>
    {:else if draft.modelsError}
      <p class="muted" role="alert">{m.native_create_models_failed()}: {draft.modelsError}</p>
      <button type="button" class="retry" onclick={() => draft.loadModels()}>{m.sync_retry()}</button>
    {:else}
      <ul class="list">
        <li>
          <button type="button" class="row" class:on={draft.model === ''} aria-pressed={draft.model === ''}
            onclick={() => draft.setModel('')}>{m.native_create_default()}</button>
        </li>
        {#each models as mod (valorModelo(mod))}
          {@const v = valorModelo(mod)}
          <li>
            <button type="button" class="row" class:on={draft.model === v} aria-pressed={draft.model === v}
              onclick={() => draft.setModel(v)}>
              <span>{mod.name ?? mod.id}</span>
              {#if mod.context}<span class="muted">{mod.context}</span>{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    {#if draft.levels.length > 0}
      <div class="effort-row" role="group" aria-label={m.native_create_effort()}>
        <span class="muted">{m.native_create_effort()}</span>
        <button type="button" class="chip" class:on={draft.effort === ''} aria-pressed={draft.effort === ''}
          onclick={() => (draft.effort = '')}>{m.native_create_default()}</button>
        {#each draft.levels as lvl (lvl)}
          <button type="button" class="chip" class:on={draft.effort === lvl} aria-pressed={draft.effort === lvl}
            onclick={() => (draft.effort = lvl)}>{lvl}</button>
        {/each}
      </div>
    {/if}

    {#if draft.fastAvailable}
      <label class="toggle">
        <input type="checkbox" checked={draft.fastChoice === 'priority'}
          onchange={(e) => (draft.fastChoice = e.currentTarget.checked ? 'priority' : 'default')} />
        <span class="toggle-text"><b>{m.native_ctl_fast()}</b><small>{m.native_ctl_fast_hint()}</small></span>
      </label>
    {/if}
    {#if draft.contextAvailable}
      <label class="toggle">
        <input type="checkbox" checked={draft.contextOn} onchange={(e) => draft.setContext(e.currentTarget.checked)} />
        <span class="toggle-text"><b>{m.native_create_context_title()}</b><small>{m.native_create_engine_context_help()}</small></span>
      </label>
    {/if}
    <label class="toggle">
      <input type="checkbox" checked={draft.isDefault} onchange={(e) => draft.saveDefault(e.currentTarget.checked)} />
      <span class="toggle-text"><b>{m.native_create_default_for_harness({ harness: providerName(draft.provider) })}</b></span>
    </label>
  </div>
</BottomSheet>

<style>
  .pill {
    display: inline-flex; align-items: center; gap: 6px; min-height: 32px; max-width: 100%; padding: 4px 10px;
    border: 0; border-radius: var(--radius-full); background: transparent; color: var(--text-primary); cursor: pointer;
  }
  .pill:not(:disabled):active { background: var(--fill-subtle); }
  .pill:disabled { opacity: 0.6; cursor: default; }
  .name { max-width: 160px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: var(--text-xs); font-weight: var(--fw-semibold); }
  .effort { font-size: var(--text-xs); color: var(--text-muted); }
  .sheet { padding: var(--space-2) var(--space-4) calc(env(safe-area-inset-bottom) + var(--space-4)); display: flex; flex-direction: column; gap: var(--space-2); }
  .tabs { display: flex; gap: 2px; }
  .tab {
    width: 40px; height: 40px; display: inline-flex; align-items: center; justify-content: center;
    border: 0; border-radius: var(--radius-md); background: transparent; color: var(--text-secondary); cursor: pointer;
  }
  .tab.on { background: var(--accent-dim); color: var(--text-primary); }
  .tab:disabled { opacity: 0.35; cursor: default; }
  .list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 2px; max-height: 45vh; overflow-y: auto; }
  .row {
    width: 100%; min-height: 44px; display: flex; justify-content: space-between; align-items: center; gap: 8px;
    padding: 8px 12px; border: 0; border-radius: var(--radius-md); background: transparent;
    color: var(--text-primary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .row.on { background: var(--accent-dim); }
  .muted { margin: 0; font-size: var(--text-xs); color: var(--text-muted); }
  .retry { align-self: flex-start; border: 0; background: transparent; color: var(--accent); cursor: pointer; }
  .effort-row { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; padding-top: var(--space-2); border-top: 1px solid var(--border-subtle); }
  .chip {
    min-height: 32px; padding: 4px 10px; border: 0; border-radius: var(--radius-full);
    background: transparent; color: var(--text-secondary); font-size: var(--text-xs); cursor: pointer;
  }
  .chip.on { background: var(--accent-dim); color: var(--text-primary); }
  .field { display: flex; flex-direction: column; gap: 4px; }
  .field select, .search {
    min-height: 44px; padding: 0 12px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: var(--surface-inset); color: var(--text-primary); font-size: var(--text-sm);
  }
  .toggle { display: flex; align-items: center; gap: 12px; min-height: 44px; padding: 4px 4px; cursor: pointer; }
  .toggle input { width: 22px; height: 22px; flex-shrink: 0; accent-color: var(--accent); }
  .toggle-text { display: flex; flex-direction: column; gap: 2px; font-size: var(--text-sm); color: var(--text-primary); }
  .toggle-text b { font-weight: var(--fw-semibold); }
  .toggle-text small { font-size: var(--text-xs); color: var(--text-muted); }
</style>
