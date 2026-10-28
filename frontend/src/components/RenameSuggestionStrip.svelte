<script lang="ts">
  import * as m from '../paraglide/messages';
  import { dismissRenameSuggestion, renameSession, type Server } from '@hangar/core';

  let { server, sessionName, suggestion, onRenamed, onDismissed }: {
    server: Server;
    sessionName: string;
    suggestion: string;
    onRenamed: (name: string) => void;
    onDismissed: () => void;
  } = $props();

  let busy = $state(false);
  let error = $state<string | null>(null);

  async function act(run: () => Promise<void>) {
    if (busy) return;
    busy = true;
    error = null;
    try { await run(); }
    catch (e) { error = e instanceof Error ? e.message : m.sessao_falha_renomear(); }
    finally { busy = false; }
  }

  const apply = () => act(async () => onRenamed((await renameSession(sessionName, suggestion, server)).name));
  const dismiss = () => act(async () => { await dismissRenameSuggestion(server, sessionName); onDismissed(); });
</script>

<div class="rename-suggestion" role="group" aria-label={m.rename_suggest_title({ name: suggestion })}>
  <p class="rename-title">{m.rename_suggest_title({ name: suggestion })}</p>
  {#if error}
    <p class="rename-error" role="alert">{error}</p>
  {/if}
  <div class="rename-actions">
    <button type="button" class="rename-dismiss" onclick={dismiss} disabled={busy}>{m.rename_suggest_dismiss()}</button>
    <button type="button" class="rename-apply" onclick={apply} disabled={busy}>{m.rename_suggest_apply()}</button>
  </div>
</div>

<style>
  .rename-suggestion {
    pointer-events: auto;
    margin: 0 var(--space-4) var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--surface-raised);
    border: 1px solid var(--border-default);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius-md);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }
  .rename-title {
    flex: 1 1 12rem;
    min-width: 0;
    margin: 0;
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
    overflow-wrap: anywhere;
  }
  .rename-error { flex-basis: 100%; order: 3; margin: 0; font-size: var(--text-xs); color: var(--error); }
  .rename-actions { display: flex; gap: var(--space-2); margin-left: auto; }
  .rename-actions button {
    min-height: 44px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-md);
    font-weight: 600;
    font-size: var(--text-sm);
    cursor: pointer;
  }
  .rename-dismiss { border: 1px solid var(--border-default); background: transparent; color: var(--text-secondary); }
  .rename-apply { border: none; background: var(--accent); color: #fff; }
  .rename-actions button:disabled { opacity: 0.6; cursor: default; }
  .rename-actions button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
