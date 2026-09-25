<script lang="ts">
  import * as m from '../paraglide/messages';
  import type { OrqConductor } from '@hangar/core';
  import Spinner from './Spinner.svelte';
  import OrqConductorChip from './OrqConductorChip.svelte';
  import { FEED_FILTERS, feedTasks, feedTime, filterFeed, fmtP, type FeedFilter } from '../lib/orqConductor';

  interface Props {
    conductor: OrqConductor | null;   // null = ainda carregando
    error: string;
  }
  let { conductor, error }: Props = $props();

  // Filtro e itens abertos vivem aqui e sobrevivem à revalidação: o pai troca `conductor` a cada
  // 20 s sem desmontar este componente.
  let kind = $state<FeedFilter>('all');
  let task = $state<number | null>(null);
  let open = $state<Set<string>>(new Set());

  const tasks = $derived(conductor ? feedTasks(conductor.feed) : []);
  const visible = $derived(conductor ? filterFeed(conductor.feed, kind, task) : []);
  // Eventos sempre existem numa execução listada; "vazio" é não ter passado recado nenhum.
  const noMessages = $derived(conductor ? !conductor.feed.some((i) => i.kind !== 'event') : false);

  function label(k: FeedFilter): string {
    switch (k) {
      case 'all': return m.orq_conductor_filter_all();
      case 'woke': return m.orq_conductor_kind_woke();
      case 'dropped': return m.orq_conductor_kind_dropped();
      case 'notice': return m.orq_conductor_kind_notice();
      case 'alarm': return m.orq_conductor_kind_alarm();
      case 'event': return m.orq_conductor_kind_event();
    }
  }

  function toggle(id: string) {
    const s = new Set(open);
    if (s.has(id)) s.delete(id); else s.add(id);
    open = s;
  }

  const time = (iso: string | null) => (iso ? feedTime(iso) || '—' : '—');
</script>

<section class="conductor">
  <h2>{m.orq_conductor_title()}</h2>
  {#if error}
    <p class="warn">{m.orq_conductor_error({ error })}</p>
  {:else if !conductor}
    <Spinner label={m.orq_conductor_loading()} />
  {:else}
    {@const w = conductor.watchdog}
    <div class="head">
      <OrqConductorChip watchdog={w} />
      <dl class="fields">
        <div><dt>{m.orq_conductor_last_cycle()}</dt><dd>{time(w.last_cycle)}</dd></div>
        <div><dt>{m.orq_conductor_since()}</dt><dd>{time(w.since)}</dd></div>
        <div><dt>{m.orq_conductor_restarts()}</dt><dd>{w.restarts ?? '—'}</dd></div>
        <div><dt>{m.orq_conductor_unit()}</dt><dd>{w.unit ?? '—'}{w.unit_state ? ` · ${w.unit_state}` : ''}</dd></div>
        <div><dt>{m.orq_conductor_arbiter()}</dt><dd>{w.arbiter ?? '—'}</dd></div>
        <div><dt>{m.orq_conductor_watching()}</dt><dd>{w.watching.length ? w.watching.join(', ') : '—'}</dd></div>
      </dl>
    </div>

    {#if noMessages}
      <p class="empty">{m.orq_conductor_empty()}</p>
    {/if}
    {#if conductor.feed.length}
      <div class="filters">
        <div class="kinds" role="group" aria-label={m.orq_conductor_filter_kind()}>
          {#each FEED_FILTERS as k (k)}
            <button class="kind" data-kind={k} aria-pressed={kind === k} onclick={() => (kind = k)}>{label(k)}</button>
          {/each}
        </div>
        {#if tasks.length}
          <select
            value={task === null ? '' : String(task)}
            onchange={(e) => { const v = e.currentTarget.value; task = v === '' ? null : Number(v); }}
            aria-label={m.orq_conductor_filter_task()}
          >
            <option value="">{m.orq_conductor_all_tasks()}</option>
            {#each tasks as n (n)}<option value={String(n)}>T{n}</option>{/each}
          </select>
        {/if}
      </div>

      {#if visible.length === 0}
        <p class="note">{m.orq_conductor_filter_empty()}</p>
      {/if}
      <ul class="feed">
        {#each visible as item (item.id)}
          <li class="item k-{item.kind}">
            <div class="meta">
              <span class="time">{time(item.ts)}</span>
              <span>{label(item.kind)}</span>
              {#if item.task !== null}<span class="task">T{item.task}</span>{/if}
            </div>
            <!-- Texto puro: o registro não é markdown, e crase ali é crase. -->
            <p class="text">{item.text}</p>
            {#if item.jev}
              {@const j = item.jev}
              <button class="jev-toggle" aria-expanded={open.has(item.id)} onclick={() => toggle(item.id)}>{m.orq_conductor_jev()}</button>
              {#if open.has(item.id)}
                <dl class="jev">
                  {#if j.error}
                    <div><dt>{m.orq_conductor_jev_error()}</dt><dd>{j.error}</dd></div>
                  {:else}
                    <div><dt>{m.orq_conductor_jev_choice()}</dt><dd>{j.choice ?? '—'}</dd></div>
                    <div><dt>{m.orq_conductor_jev_certainty()}</dt><dd>{fmtP(j.p)}</dd></div>
                    <div>
                      <dt>{m.orq_conductor_jev_vetoes()}</dt>
                      <dd>
                        <ul class="vetoes">
                          {#each Object.entries(j.veto) as [name, v] (name)}
                            <li class:held={j.held.includes(name)}>{name} {fmtP(v)}</li>
                          {/each}
                        </ul>
                      </dd>
                    </div>
                  {/if}
                </dl>
                {#if !j.error && j.would_drop}<p class="note">{m.orq_conductor_jev_would_drop()}</p>{/if}
              {/if}
            {/if}
          </li>
        {/each}
      </ul>
      {#if conductor.truncated}<p class="note">{m.orq_conductor_truncated()}</p>{/if}
    {/if}
    {#if conductor.skipped}<p class="note">{m.orq_conductor_skipped({ n: conductor.skipped })}</p>{/if}
  {/if}
</section>

<style>
  .conductor { display: flex; flex-direction: column; gap: var(--space-2); }
  h2 { margin-top: var(--space-2); color: var(--text-muted); font-size: var(--text-xs); font-weight: 600; text-transform: uppercase; letter-spacing: 0.07em; }
  .warn { color: var(--warning); font-size: var(--text-sm); }
  .empty, .note { color: var(--text-muted); font-size: var(--text-xs); }

  .head {
    display: flex; flex-direction: column; align-items: flex-start; gap: var(--space-2);
    padding: var(--space-3);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-lg);
    background: var(--surface-card);
  }
  .fields { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: var(--space-2); width: 100%; margin: 0; }
  /* Quem aperta é a largura do painel `.orq` (o container da tela), não a janela. */
  @container (max-width: 560px) { .fields { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
  .fields dt, .jev dt { color: var(--text-muted); font-size: var(--text-xs); }
  .fields dd, .jev dd { margin: 0; font-family: var(--font-mono); font-size: var(--text-xs); overflow-wrap: anywhere; }

  .filters { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); }
  .kinds { display: flex; flex-wrap: wrap; gap: var(--space-1); }
  .kind {
    min-height: 0; padding: 2px 10px;
    border: 1px solid var(--border-subtle); border-radius: 999px;
    background: transparent; color: var(--text-secondary); font-size: var(--text-xs);
  }
  .kind[aria-pressed='true'] { border-color: transparent; background: var(--accent-dim); color: var(--accent); }
  select { font-size: var(--text-xs); }

  .feed { display: flex; flex-direction: column; gap: var(--space-2); margin: 0; padding: 0; list-style: none; }
  .item { padding: var(--space-1) var(--space-3); border-left: 2px solid var(--border-default); background: transparent; }
  .item.k-woke { border-left-color: var(--accent); }
  .item.k-dropped { border-left-color: var(--success); }
  .item.k-alarm { border-left-color: var(--warning); }
  .meta { display: flex; flex-wrap: wrap; gap: var(--space-2); color: var(--text-muted); font-size: var(--text-xs); }
  .time, .task { font-family: var(--font-mono); }
  .text { margin: 2px 0 0; font-size: var(--text-sm); white-space: pre-wrap; overflow-wrap: anywhere; }
  .jev-toggle { min-height: 0; margin-top: var(--space-1); padding: 0; background: transparent; color: var(--accent); font-size: var(--text-xs); }
  .jev {
    display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: var(--space-2);
    margin: var(--space-1) 0 0; padding: var(--space-2);
    border-radius: var(--radius-md); background: var(--surface-inset);
  }
  @container (max-width: 560px) { .jev { grid-template-columns: minmax(0, 1fr); } }
  .vetoes { margin: 0; padding: 0; list-style: none; }
  .vetoes .held { color: var(--warning); font-weight: 600; }
</style>
