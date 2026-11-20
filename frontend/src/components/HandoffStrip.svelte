<script lang="ts">
  // Faixa da troca de agente pedida pelo seletor de modelos (no app de PC): esperando o turno acabar, em curso
  // ou que falhou. O que for enviado enquanto ela espera vai ao modelo novo.
  import * as m from '../paraglide/messages';
  import { cancelHandoffForServer, handoffTitle, modelDisplay, type Server, type SessionInfo } from '@hangar/core';

  let { server, session }: { server: Server; session: SessionInfo } = $props();

  let busy = $state(false);
  let error = $state<string | null>(null);
  const handoff = $derived(session.handoff);
  const from = $derived(modelDisplay(session.provider, session.model));
  const to = $derived(modelDisplay(handoff?.provider, handoff?.model));
  const title = $derived(handoff ? handoffTitle(handoff, session.state) : '');

  async function cancel() {
    if (busy) return;
    busy = true;
    error = null;
    try { await cancelHandoffForServer(server, session.name); }
    catch (e) { error = e instanceof Error ? e.message : String(e); }
    finally { busy = false; }
  }
</script>

{#if handoff}
  <div class="handoff-strip" class:failed={handoff.state === 'failed'} role="status" aria-live="polite">
    <div class="head">
      <p class="title">{title}</p>
      <p class="route">{from} → {to}</p>
    </div>
    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}
    {#if handoff.state === 'waiting'}
      <button type="button" onclick={cancel} disabled={busy}>{m.handoff_cancel()}</button>
    {/if}
  </div>
{/if}

<style>
  .handoff-strip {
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
  .head { flex: 1 1 12rem; min-width: 0; }
  .title { margin: 0; font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .failed .title { color: var(--warning); }
  .route { margin: 0; font-size: var(--text-xs); color: var(--text-muted); overflow-wrap: anywhere; }
  .error { flex-basis: 100%; order: 3; margin: 0; font-size: var(--text-xs); color: var(--error); }
  button {
    min-height: 44px;
    margin-left: auto;
    padding: 0 var(--space-3);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text-secondary);
    font-weight: 600;
    font-size: var(--text-sm);
    cursor: pointer;
  }
  button:disabled { opacity: 0.6; cursor: default; }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
