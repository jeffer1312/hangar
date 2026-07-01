import type { AccountCost, CostBucket, CostModelBucket, CostReport } from './types';

export interface ServerResult {
  report: CostReport | null; // null = servidor falhou/offline
}

export interface MergedReport {
  accounts: AccountCost[];
  partial: boolean; // algum servidor nao respondeu
}

export function addBuckets(into: Map<string, CostBucket>, list: CostBucket[]): void {
  for (const b of list) {
    const cur = into.get(b.key);
    if (!cur) {
      into.set(b.key, { ...b });
    } else {
      cur.sessions += b.sessions;
      cur.input += b.input;
      cur.output += b.output;
      cur.cache_read += b.cache_read;
      cur.cache_write += b.cache_write;
      cur.cost += b.cost;
    }
  }
}

function addModels(into: Map<string, CostModelBucket>, list: CostModelBucket[]): void {
  for (const m of list) {
    const cur = into.get(m.model);
    if (!cur) into.set(m.model, { ...m });
    else { cur.sessions += m.sessions; cur.cost += m.cost; }
  }
}

interface Acc {
  account_id: string;
  email: string | null;
  label: string;
  totals: CostBucket;
  today: number;
  yesterday: number;
  day: Map<string, CostBucket>;
  week: Map<string, CostBucket>;
  month: Map<string, CostBucket>;
  model: Map<string, CostModelBucket>;
}

function emptyTotals(): CostBucket {
  return { key: 'totals', sessions: 0, input: 0, output: 0, cache_read: 0, cache_write: 0, cost: 0 };
}

// Ordena buckets de periodo por key desc (data mais recente primeiro).
export function sortDesc(m: Map<string, CostBucket>): CostBucket[] {
  return [...m.values()].sort((a, b) => (a.key < b.key ? 1 : a.key > b.key ? -1 : 0));
}

export function mergeAccounts(results: ServerResult[]): MergedReport {
  const byId = new Map<string, Acc>();
  let partial = false;

  for (const r of results) {
    if (!r.report) { partial = true; continue; }
    for (const a of r.report.accounts) {
      let acc = byId.get(a.account_id);
      if (!acc) {
        acc = {
          account_id: a.account_id, email: a.email, label: a.label,
          totals: emptyTotals(), today: 0, yesterday: 0,
          day: new Map(), week: new Map(), month: new Map(), model: new Map(),
        };
        byId.set(a.account_id, acc);
      }
      acc.totals.sessions += a.totals.sessions;
      acc.totals.input += a.totals.input;
      acc.totals.output += a.totals.output;
      acc.totals.cache_read += a.totals.cache_read;
      acc.totals.cache_write += a.totals.cache_write;
      acc.totals.cost += a.totals.cost;
      acc.today += a.today;
      acc.yesterday += a.yesterday;
      addBuckets(acc.day, a.by_day);
      addBuckets(acc.week, a.by_week);
      addBuckets(acc.month, a.by_month);
      addModels(acc.model, a.by_model);
    }
  }

  const accounts: AccountCost[] = [...byId.values()].map((acc) => ({
    account_id: acc.account_id,
    email: acc.email,
    label: acc.label,
    totals: acc.totals,
    today: acc.today,
    yesterday: acc.yesterday,
    by_day: sortDesc(acc.day),
    by_week: sortDesc(acc.week),
    by_month: sortDesc(acc.month),
    by_model: [...acc.model.values()].sort((a, b) => b.cost - a.cost),
  }));

  return { accounts, partial };
}
