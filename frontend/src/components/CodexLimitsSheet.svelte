<script lang="ts">
  // Task B: limites de uso da conta Codex (account/rateLimits/read via GET /limits) — so sessoes
  // Codex, aberto pelo badge "Codex" da NavBar. Fetch-on-open, mesmo padrao do RunSheet.
  import * as m from '../paraglide/messages';
  import BottomSheet from './BottomSheet.svelte';
  import { getLimits } from '@hangar/core';
  import { resetsIn } from '@hangar/core';
  import type { SessionLimits, RateLimitWindow } from '@hangar/core';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  let limits = $state<SessionLimits | null>(null);
  let err = $state<string | null>(null);

  async function load() {
    err = null;
    try {
      limits = await getLimits(sessionName);
    } catch (e) {
      err = String(e);
    }
  }

  $effect(() => {
    if (open) load();
  });

  function label(w: RateLimitWindow): string {
    const pct = w.usedPercent != null ? `${Math.round(w.usedPercent)}%` : '—';
    const inTxt = resetsIn(w.resetsAt);
    const reset = inTxt ? ` · reseta ${inTxt}` : '';
    return m.codex_limite_uso({ pct, reset });
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={m.codex_limites_titulo()}>
  <h2 class="sheet-title">{m.ctx_limites()}</h2>

  {#if err}
    <p class="err">{err}</p>
  {:else if !limits}
    <p class="empty">{m.comum_carregando()}</p>
  {:else if !limits.primary && !limits.secondary}
    <p class="empty">{m.codex_sem_dados_limite()}</p>
  {:else}
    <ul class="limit-list">
      {#if limits.primary}
        <li class="limit-row">
          <span class="limit-label">{m.codex_limite_principal()}</span>
          <span class="limit-value">{label(limits.primary)}</span>
        </li>
      {/if}
      {#if limits.secondary}
        <li class="limit-row">
          <span class="limit-label">{m.codex_limite_secundario()}</span>
          <span class="limit-value">{label(limits.secondary)}</span>
        </li>
      {/if}
    </ul>
    {#if limits.planType}<p class="plan">{m.codex_plano({ plano: limits.planType })}</p>{/if}
  {/if}
</BottomSheet>

<style>
  .sheet-title { font-size: var(--text-xl); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-4); }
  .err { color: var(--error); font-size: var(--text-sm); }
  .empty { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: var(--space-4) 0; }
  .limit-list { list-style: none; display: flex; flex-direction: column; gap: var(--space-2); }
  .limit-row { display: flex; align-items: center; justify-content: space-between; gap: var(--space-3);
    padding: var(--space-2) var(--space-3); border-radius: var(--radius-md); background: var(--bg-surface); }
  .limit-label { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .limit-value { font-size: var(--text-sm); color: var(--text-secondary); }
  .plan { margin-top: var(--space-3); font-size: var(--text-xs); color: var(--text-muted); }
</style>
