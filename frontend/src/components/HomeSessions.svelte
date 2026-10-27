<script lang="ts">
  import { onMount } from 'svelte';
  import { stateColors } from '@hangar/core';
  import SessionCard from './SessionCard.svelte';
  import ConfirmSheet from './ConfirmSheet.svelte';
  import KeyringNotice from './KeyringNotice.svelte';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { createSessionListModel } from '../lib/sessionListModel.svelte';
  import { groupHomeSessions, homeSessionsView, type HomeState } from '../lib/homeSessions';
  import * as m from '../paraglide/messages';

  // Sessões abertas de todas as máquinas na tela inicial do celular, agrupadas pelo que pedem de você.
  let { onOpenChat }: { onOpenChat: (name: string) => void } = $props();

  const model = createSessionListModel({ variant: 'mobile', onOpen: (n) => onOpenChat(n), onCompare: () => {} });
  onMount(() => model.mount());

  let readyExpanded = $state(homeSessionsView.readyExpanded);
  let deleteError = $state('');
  const groups = $derived(groupHomeSessions(sessionsStore.rows, readyExpanded));
  const machines = $derived(sessionsStore.byServer.filter((b) => b.error || !b.loaded));
  const manyServers = $derived(sessionsStore.byServer.length > 1);

  const LABEL: Record<HomeState, () => string> = {
    awaiting_input: m.home_sessions_awaiting,
    working: m.home_sessions_working,
    idle: m.home_sessions_ready,
  };

  async function doDelete() {
    const alvo = model.confirmDel;
    const r = await model.doDelete();
    deleteError = r.erro && alvo ? m.lista_flash_excluir({ nome: alvo.name, erro: r.erro }) : '';
  }
</script>

<div class="home-sessions">
  {#each machines as b (b.server.id)}
    {#if b.error}
      <p class="machine error" role="alert">
        {b.error === 'offline' ? m.lista_servidor_offline({ label: b.server.label }) : `${b.server.label}: ${b.error}`}
      </p>
    {:else if manyServers}
      <p class="machine">{b.server.label}: {m.lista_carregando()}</p>
    {/if}
  {/each}
  {#each sessionsStore.byServer.filter((b) => b.keyringLocked && !b.server.invite) as b (b.server.id)}
    <KeyringNotice server={b.server} />
  {/each}
  {#if deleteError}<p class="machine error" role="alert">{deleteError}</p>{/if}

  {#if groups.length === 0}
    {#if sessionsStore.loading || machines.some((b) => !b.loaded && !b.error)}
      <p class="empty" role="status">{m.lista_carregando()}</p>
    {:else if machines.length === 0}
      <p class="empty">{m.lista_nenhuma_ativa()}</p>
    {/if}
  {:else}
    {#each groups as g (g.state)}
      <section class="group" aria-label={LABEL[g.state]()}>
        <h2 class="group-head">
          <span class="dot" style="background: {stateColors[g.state]};" aria-hidden="true"></span>
          <span>{LABEL[g.state]()}</span>
          <span class="count">{g.total}</span>
        </h2>
        {#each g.servers as server (server.id)}
          <section class="server-group" aria-label={server.label}>
            <h3 class="server-head">
              <span class="dot" style:background={server.color} aria-hidden="true"></span>
              <span>{server.label}</span>
            </h3>
            {#each server.sessions as session (session.name)}
              <SessionCard {session}
                onClick={() => { deleteError = ''; model.open(session); }}
                onDelete={() => { deleteError = ''; model.requestDelete(session.name, session.serverId, session.state); }} />
            {/each}
          </section>
        {/each}
        {#if g.state === 'idle' && g.total > 1}
          <button type="button" class="more" aria-expanded={readyExpanded} onclick={() => (homeSessionsView.readyExpanded = readyExpanded = !readyExpanded)}>
            {readyExpanded ? m.sessao_recolher() : m.home_sessions_more_ready({ n: g.total - 1 })}
          </button>
        {/if}
      </section>
    {/each}
  {/if}
</div>

<ConfirmSheet
  open={model.confirmDel !== null}
  title={m.sessao_excluir()}
  message={model.confirmDel
    ? model.confirmDel.state === 'working' ? m.lista_excluir_executando({ n: model.confirmDel.name }) : model.confirmDel.name
    : null}
  confirmLabel={m.sessao_excluir_curto()}
  danger
  onConfirm={doDelete}
  onClose={() => (model.confirmDel = null)}
/>

<style>
  .home-sessions { display: flex; flex-direction: column; padding-bottom: var(--space-2); }
  .group { display: flex; flex-direction: column; }
  .group-head {
    display: flex; align-items: center; gap: 8px; margin: 0; padding: var(--space-3) var(--space-4) var(--space-1);
    font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary);
  }
  .server-head {
    display: flex; align-items: center; gap: var(--space-2); margin: 0; padding: var(--space-2) var(--space-4);
    font-size: var(--text-sm); font-weight: 600; color: var(--text-primary);
  }
  .dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  .count { font-weight: 500; color: var(--text-muted); font-variant-numeric: tabular-nums; }
  .more {
    align-self: stretch; min-height: 44px; display: flex; align-items: center; justify-content: flex-start; padding: 0 var(--space-4) 0 46px; border: 0; background: transparent;
    color: var(--accent); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .machine { margin: 0; padding: 6px var(--space-4); font-size: var(--text-sm); color: var(--text-muted); }
  .machine.error { color: var(--error); }
  .empty { margin: 0; padding: var(--space-6) var(--space-4); text-align: center; color: var(--text-muted); font-size: var(--text-sm); }
</style>
