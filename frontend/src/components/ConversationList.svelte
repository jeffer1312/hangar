<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
  import { getArchiveRecentForServer, relativeTime, mergeConversations, rememberEntry, type ArchiveEntry } from '@hangar/core';
  import { listOwnServers } from '../lib/auth';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { negadaAoConvidado } from '../lib/papel.svelte';

  interface Props {
    onNavigateToChat: (serverId: string, name: string) => void;
  }
  let { onNavigateToChat }: Props = $props();

  let closedByServer = $state(new Map<string, ArchiveEntry[]>());
  let failed = $state<string[]>([]);
  let loaded = $state(false);

  const ownIds = $derived(new Set(listOwnServers().map((s) => s.id)));
  const live = $derived(sessionsStore.rows.filter((s) => ownIds.has(s.serverId)));
  const rows = $derived(mergeConversations(live, closedByServer));

  let seq = 0;
  async function load() {
    const mine = ++seq;
    // Servidor em espera/offline não é consultado: cada tentativa custaria o teto de 8s da lista inteira.
    const down = new Set(sessionsStore.byServer.filter((b) => b.error).map((b) => b.server.id));
    const own = listOwnServers();
    const tried = own.filter((s) => !down.has(s.id));
    const results = await Promise.allSettled(tried.map((s) => getArchiveRecentForServer(s)));
    if (mine !== seq) return;
    const next = new Map<string, ArchiveEntry[]>();
    const bad: string[] = [];
    // Servidor fora do ar entra na linha de falha e mantém a última lista boa.
    for (const s of own) {
      if (!down.has(s.id)) continue;
      bad.push(s.label);
      const old = closedByServer.get(s.id);
      if (old) next.set(s.id, old);
    }
    results.forEach((r, i) => {
      if (r.status === 'fulfilled') next.set(tried[i].id, r.value);
      // Convidado não tem Arquivo: a máquina respondeu, só não há o que listar.
      else if (negadaAoConvidado(r.reason)) next.set(tried[i].id, []);
      else {
        bad.push(tried[i].label);
        // Mantém a última lista boa do servidor que falhou agora.
        const old = closedByServer.get(tried[i].id);
        if (old) next.set(tried[i].id, old);
      }
    });
    closedByServer = next;
    failed = bad;
    loaded = true;
  }

  onMount(() => {
    void load();
    const onVisible = () => { if (document.visibilityState === 'visible') void load(); };
    document.addEventListener('visibilitychange', onVisible);
    return () => document.removeEventListener('visibilitychange', onVisible);
  });

  // Sessão que fechou passa a existir só no arquivo: relê pra ela aparecer como fechada.
  let prevLive = new Set<string>();
  $effect(() => {
    const now = new Set(live.map((s) => `${s.serverId}::${s.name}`));
    const shrank = [...prevLive].some((k) => !now.has(k));
    prevLive = now;
    if (shrank) untrack(() => void load());
  });

  // Servidor que voltou ao ar: relê os recentes dele.
  let prevDown = new Set<string>();
  $effect(() => {
    const now = new Set(sessionsStore.byServer.filter((b) => b.error).map((b) => b.server.id));
    const recovered = [...prevDown].some((id) => !now.has(id));
    prevDown = now;
    if (recovered) untrack(() => void load());
  });

  function open(row: (typeof rows)[number]) {
    if (row.kind === 'live') {
      onNavigateToChat(row.serverId, row.name);
      return;
    }
    rememberEntry(row.serverId, row.entry);
    window.location.hash = `#/archive/${encodeURIComponent(row.serverId)}/${encodeURIComponent(row.entry.project)}/${encodeURIComponent(row.entry.session_id)}`;
  }

  const providerOf = (name: string, serverId: string) =>
    live.find((s) => s.serverId === serverId && s.name === name)?.provider;
</script>

<div class="conv">
  <button type="button" class="new" onclick={() => { window.location.hash = '#/'; }}>
    <span class="plus" aria-hidden="true">+</span>{m.conversas_nova()}
  </button>
  <h2 class="section">{m.conversas_recentes()}</h2>

  {#if !loaded && rows.length === 0}
    <div class="skeleton" aria-label={m.comum_carregando()}>
      {#each Array(6) as _, i (i)}<div class="sk-row"><span class="sk-bar"></span></div>{/each}
    </div>
  {:else if rows.length === 0 && failed.length === 0}
    <p class="empty">{m.conversas_vazio()}</p>
  {:else}
    <ul>
      {#each rows as row (row.kind === 'live' ? `l:${row.serverId}:${row.name}` : `c:${row.serverId}:${row.entry.project}:${row.entry.session_id}`)}
        <li>
          <button type="button" class="row" onclick={() => open(row)}>
            <span class="icon" aria-hidden="true">
              {#if row.kind === 'live'}
                <ProviderGlyph provider={providerOf(row.name, row.serverId)} size={18} />
                <span class="dot {row.state}"></span>
              {:else}
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12z"/></svg>
              {/if}
            </span>
            <span class="title">{row.title}</span>
            <time class="when">{relativeTime(row.at)}</time>
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  {#each failed as servidor (servidor)}
    <p class="failed" role="status">{m.conversas_servidor_falhou({ servidor })}</p>
  {/each}
</div>

<style>
  .conv { background: transparent; padding: 4px 0 24px; }
  .new {
    display: flex; align-items: center; gap: 10px; width: 100%; min-height: 48px; padding: 0 16px;
    background: transparent; border: 0; color: var(--text); font: inherit; font-weight: 600; text-align: left;
  }
  .plus { width: 18px; text-align: center; font-size: 20px; line-height: 1; color: var(--accent); }
  .section {
    margin: 8px 0 0; padding: 8px 16px; font-size: 12px; font-weight: 600; letter-spacing: .02em;
    color: var(--text-muted); text-transform: none;
  }
  ul { list-style: none; margin: 0; padding: 0; }
  .row {
    display: flex; align-items: center; gap: 12px; width: 100%; height: 48px; padding: 0 16px;
    background: transparent; border: 0; color: var(--text); font: inherit; text-align: left;
  }
  .row:active { background: var(--surface-inset, rgba(127, 127, 127, .12)); }
  .icon { position: relative; flex: none; width: 20px; display: grid; place-items: center; color: var(--text-muted); }
  .dot { position: absolute; right: -3px; bottom: -3px; width: 8px; height: 8px; border-radius: 50%; background: var(--text-muted); }
  .dot.working { background: var(--accent); }
  .dot.awaiting_input { background: var(--warning); }
  .title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .when { flex: none; font-size: 12px; color: var(--text-muted); }
  .empty, .failed { margin: 0; padding: 12px 16px; font-size: 13px; color: var(--text-muted); }
  .failed { color: var(--warning); }
  .sk-row { height: 48px; display: flex; align-items: center; padding: 0 16px; }
  .sk-bar { height: 12px; width: 60%; border-radius: 6px; background: var(--surface-inset, rgba(127, 127, 127, .15)); opacity: .6; }
</style>
