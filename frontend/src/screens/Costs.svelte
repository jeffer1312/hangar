<script lang="ts">
  import NavBar from '../components/NavBar.svelte';
  import { listServers } from '../lib/auth';
  import { fetchCostsForServer } from '../lib/api';
  import { mergeAccounts, type ServerResult, type MergedReport } from '../lib/costs';
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
    for (const list of lists) for (const b of list) {
      const cur = m.get(b.key);
      if (!cur) m.set(b.key, { ...b });
      else {
        cur.sessions += b.sessions; cur.input += b.input; cur.output += b.output;
        cur.cache_read += b.cache_read; cur.cache_write += b.cache_write; cur.cost += b.cost;
      }
    }
    return [...m.values()].sort((a, b) => (a.key < b.key ? 1 : -1));
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
      <button class:on={selected === 'all'} onclick={() => (selected = 'all')}>Todas</button>
      {#each merged.accounts as a}
        <button class:on={selected === a.account_id} onclick={() => (selected = a.account_id)}>
          {a.email ?? a.label}
        </button>
      {/each}
    </div>

    <div class="chips">
      <span class="chip">Hoje <b>{money(view.today)}</b></span>
      <span class="chip">Ontem <b>{money(view.yesterday)}</b></span>
    </div>

    <div class="cards">
      <div class="card"><div class="v">{money(view.totals.cost)}</div><div class="l">custo total</div></div>
      <div class="card"><div class="v">{view.totals.sessions}</div><div class="l">sessões</div></div>
      <div class="card"><div class="v">{abbrevNum(view.totals.input)}</div><div class="l">input</div></div>
      <div class="card"><div class="v">{abbrevNum(view.totals.output)}</div><div class="l">output</div></div>
    </div>

    <div class="tabs" role="tablist" aria-label="Período">
      <button class:on={period === 'day'} onclick={() => (period = 'day')}>Dia</button>
      <button class:on={period === 'week'} onclick={() => (period = 'week')}>Semana</button>
      <button class:on={period === 'month'} onclick={() => (period = 'month')}>Mês</button>
    </div>

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

    <h3>Por modelo</h3>
    <table>
      <tbody>
        {#each view.by_model as m}
          <tr><td class="k">{m.model}</td><td class="n">{m.sessions}</td><td class="c">{money(m.cost)}</td></tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .costs { padding: 12px 16px 40px; }
  .muted { color: var(--text-dim, #8b949e); }
  .warn { color: #d29922; font-size: 13px; }
  .tabs { display: flex; gap: 8px; flex-wrap: wrap; margin: 12px 0; }
  .tabs button {
    background: var(--surface, #161b22); border: 1px solid var(--border, #30363d);
    color: inherit; padding: 6px 14px; border-radius: 8px; font-size: 14px;
  }
  .tabs button.on { background: var(--accent, #238636); border-color: var(--accent, #3fb950); }
  .chips { display: flex; gap: 10px; margin: 8px 0; }
  .chip { background: var(--surface, #161b22); border: 1px solid var(--border, #30363d); border-radius: 8px; padding: 6px 12px; font-size: 13px; }
  .cards { display: flex; gap: 10px; flex-wrap: wrap; margin: 12px 0; }
  .card { background: var(--surface, #161b22); border: 1px solid var(--border, #30363d); border-radius: 10px; padding: 12px 16px; min-width: 92px; }
  .card .v { font-size: 20px; font-weight: 700; }
  .card .l { font-size: 11px; color: var(--text-dim, #8b949e); text-transform: uppercase; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; margin-top: 8px; }
  th, td { padding: 6px 8px; text-align: left; border-bottom: 1px solid var(--border, #30363d); }
  th { color: var(--text-dim, #8b949e); font-weight: 600; font-size: 11px; text-transform: uppercase; }
  .n, .c { text-align: right; font-variant-numeric: tabular-nums; }
  .c { font-weight: 700; color: var(--accent, #3fb950); }
  .k { white-space: nowrap; }
  .bar { width: 80px; }
  .bar span { display: block; height: 8px; border-radius: 4px; background: var(--accent, #3fb950); }
  h3 { margin: 20px 0 4px; font-size: 15px; }
</style>
