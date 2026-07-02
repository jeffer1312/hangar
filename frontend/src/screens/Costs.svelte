<script lang="ts">
  import NavBar from '../components/NavBar.svelte';
  import { listServers } from '../lib/auth';
  import { fetchCostsForServer } from '../lib/api';
  import { mergeAccounts, addBuckets, sortDesc, type ServerResult, type MergedReport } from '../lib/costs';
  import { abbrevNum } from '../lib/format';
  import type { AccountCost, CostBucket } from '../lib/types';

  interface Props { onBack: () => void; }
  let { onBack }: Props = $props();

  let loading = $state(true);
  let merged = $state<MergedReport>({ accounts: [], partial: false });
  let selected = $state<string>('all'); // account_id ou 'all'
  let period = $state<'day' | 'week' | 'month'>('day');

  async function load() {
    loading = true;
    const servers = listServers();
    const results: ServerResult[] = await Promise.all(
      servers.map(async (s) => {
        try { return { report: await fetchCostsForServer(s) }; }
        catch { return { report: null }; }
      }),
    );
    merged = mergeAccounts(results);
    if (!merged.accounts.some((a) => a.account_id === selected)) selected = 'all';
    loading = false;
  }

  $effect(() => { load(); });

  // Soma uma lista de listas de buckets por key (usado no modo "Todas").
  function sumBuckets(lists: CostBucket[][]): CostBucket[] {
    const m = new Map<string, CostBucket>();
    for (const list of lists) addBuckets(m, list);
    return sortDesc(m);
  }

  const view = $derived.by<AccountCost | null>(() => {
    const accs = merged.accounts;
    if (accs.length === 0) return null;
    if (selected !== 'all') return accs.find((a) => a.account_id === selected) ?? null;
    // "Todas" = agrega todas as contas
    const sum = (f: (a: AccountCost) => number) => accs.reduce((t, a) => t + f(a), 0);
    const modelMap = new Map<string, { model: string; sessions: number; cost: number }>();
    for (const a of accs) for (const mb of a.by_model) {
      const cur = modelMap.get(mb.model);
      if (!cur) modelMap.set(mb.model, { ...mb });
      else { cur.sessions += mb.sessions; cur.cost += mb.cost; }
    }
    return {
      account_id: 'all', email: null, label: 'Todas',
      totals: {
        key: 'totals', sessions: sum((a) => a.totals.sessions),
        input: sum((a) => a.totals.input), output: sum((a) => a.totals.output),
        cache_read: sum((a) => a.totals.cache_read), cache_write: sum((a) => a.totals.cache_write),
        cost: sum((a) => a.totals.cost),
      },
      today: sum((a) => a.today), yesterday: sum((a) => a.yesterday),
      by_day: sumBuckets(accs.map((a) => a.by_day)),
      by_week: sumBuckets(accs.map((a) => a.by_week)),
      by_month: sumBuckets(accs.map((a) => a.by_month)),
      by_model: [...modelMap.values()].sort((a, b) => b.cost - a.cost),
    };
  });

  const rows = $derived(
    view ? (period === 'day' ? view.by_day : period === 'week' ? view.by_week : view.by_month) : [],
  );
  const peak = $derived(Math.max(1, ...rows.map((r) => r.cost)));
  const money = (n: number) => `$${n.toFixed(2)}`;
</script>

<NavBar title="Custos" showBack={true} onBack={onBack} />

<div class="costs">
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if !view}
    <p class="muted">Sem dados ainda. O custo aparece após a 1ª sessão parar.</p>
  {:else}
    {#if merged.partial}
      <p class="warn">⚠ Alguns servidores não responderam — total parcial.</p>
    {/if}

    <div class="tabs" role="tablist" aria-label="Conta">
      <button role="tab" aria-selected={selected === 'all'} class:on={selected === 'all'} onclick={() => (selected = 'all')}>Todas</button>
      {#each merged.accounts as a}
        <button role="tab" aria-selected={selected === a.account_id} class:on={selected === a.account_id} onclick={() => (selected = a.account_id)}>
          {a.email ?? a.label}
        </button>
      {/each}
    </div>

    <div class="chips">
      <span class="chip">Hoje <b>{money(view.today)}</b></span>
      <span class="chip">Ontem <b>{money(view.yesterday)}</b></span>
    </div>

    <!-- Stat-strip: um painel unico com tipografia contida (NAO o template hero de numero-gigante). -->
    <dl class="stats">
      <div><dt>custo total</dt><dd class="accent">{money(view.totals.cost)}</dd></div>
      <div><dt>sessões</dt><dd>{view.totals.sessions}</dd></div>
      <div><dt>input</dt><dd>{abbrevNum(view.totals.input)}</dd></div>
      <div><dt>output</dt><dd>{abbrevNum(view.totals.output)}</dd></div>
    </dl>

    <div class="tabs" role="tablist" aria-label="Período">
      <button role="tab" aria-selected={period === 'day'} class:on={period === 'day'} onclick={() => (period = 'day')}>Dia</button>
      <button role="tab" aria-selected={period === 'week'} class:on={period === 'week'} onclick={() => (period = 'week')}>Semana</button>
      <button role="tab" aria-selected={period === 'month'} class:on={period === 'month'} onclick={() => (period = 'month')}>Mês</button>
    </div>

    <!-- overflow-x: a tabela e larga (7 col) e nao pode empurrar o body no mobile (375px). -->
    <div class="twrap">
      <table>
        <thead><tr><th>período</th><th>sess</th><th>in</th><th>out</th><th>cache</th><th>custo</th><th></th></tr></thead>
        <tbody>
          {#each rows as r}
            <tr>
              <td class="k">{r.key}</td>
              <td class="n">{r.sessions}</td>
              <td class="n">{abbrevNum(r.input)}</td>
              <td class="n">{abbrevNum(r.output)}</td>
              <td class="n">{abbrevNum(r.cache_read)}</td>
              <td class="c">{money(r.cost)}</td>
              <td class="bar"><span style="width:{(r.cost / peak) * 100}%"></span></td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <h3>Por modelo</h3>
    <div class="twrap">
      <table>
        <tbody>
          {#each view.by_model as m}
            <tr><td class="k">{m.model}</td><td class="n">{m.sessions}</td><td class="c">{money(m.cost)}</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .costs { padding: var(--space-3) var(--space-4) var(--space-10); }
  .muted { color: var(--text-secondary); }
  .warn { color: var(--warning); font-size: var(--text-sm); }
  .tabs { display: flex; gap: var(--space-2); flex-wrap: wrap; margin: var(--space-3) 0; }
  .tabs button {
    background: var(--bg-surface); border: 1px solid var(--border-default);
    color: inherit; padding: var(--space-2) var(--space-4); border-radius: var(--radius-sm);
    font-size: var(--text-sm); min-height: 36px;
  }
  .tabs button.on { background: var(--accent); border-color: var(--accent); color: #fff; }
  .chips { display: flex; gap: var(--space-3); margin: var(--space-2) 0; }
  .chip {
    background: var(--bg-surface); border: 1px solid var(--border-default);
    border-radius: var(--radius-sm); padding: var(--space-2) var(--space-3); font-size: var(--text-sm);
  }
  /* Stat-strip: painel unico, tipografia contida — substitui o grid de 4 cards hero. */
  .stats {
    display: flex; flex-wrap: wrap; gap: var(--space-5) var(--space-6);
    background: var(--bg-surface); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); padding: var(--space-3) var(--space-4); margin: var(--space-3) 0;
  }
  .stats > div { display: flex; flex-direction: column; gap: 2px; }
  .stats dt { font-size: var(--text-xs); color: var(--text-muted); }
  .stats dd { font-size: var(--text-lg); font-weight: 600; font-variant-numeric: tabular-nums; }
  .stats dd.accent { color: var(--accent); }
  .twrap { overflow-x: auto; -webkit-overflow-scrolling: touch; }
  table { width: 100%; border-collapse: collapse; font-size: var(--text-sm); margin-top: var(--space-2); }
  th, td { padding: var(--space-2); text-align: left; border-bottom: 1px solid var(--border-subtle); white-space: nowrap; }
  th { color: var(--text-secondary); font-weight: 600; font-size: var(--text-xs); text-transform: uppercase; }
  .n, .c { text-align: right; font-variant-numeric: tabular-nums; }
  .c { font-weight: 700; color: var(--accent); }
  .k { white-space: nowrap; }
  .bar { width: 80px; }
  .bar span { display: block; height: 8px; border-radius: 4px; background: var(--accent); }
  h3 { margin: var(--space-5) 0 var(--space-1); font-size: var(--text-base); }
</style>
