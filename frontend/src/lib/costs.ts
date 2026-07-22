import type { AccountCost, CostBucket, CostModelBucket, CostReport } from './types';

export interface ServerResult {
  report: CostReport | null; // null = servidor falhou/offline
}

export interface MergedReport {
  accounts: AccountCost[];
  partial: boolean; // algum servidor nao respondeu
  usdBrl: number | null; // primeira cotação não-nula entre os servidores
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
    if (!cur) {
      // Normaliza os tokens JÁ NA ENTRADA: servidor da malha em versão antiga manda by_model sem
      // eles, e guardar `undefined` aqui congelava o campo — nem o `+=` de um servidor novo
      // depois recuperava (undefined + n = NaN), e a coluna aparecia zerada via `?? 0`.
      into.set(m.model, {
        ...m,
        input: m.input ?? 0,
        output: m.output ?? 0,
        cache_read: m.cache_read ?? 0,
        cache_write: m.cache_write ?? 0,
      });
    } else {
      cur.sessions += m.sessions;
      cur.cost += m.cost;
      // Somar os tokens era o que faltava: o `else` antigo só acumulava sessions/cost, então a
      // MESMA conta espalhada em mais de um servidor perdia os tokens de todos menos o primeiro.
      // Silencioso e só no caminho "conta específica" — o modo "Todas" agrega noutro lugar.
      cur.input = (cur.input ?? 0) + (m.input ?? 0);
      cur.output = (cur.output ?? 0) + (m.output ?? 0);
      cur.cache_read = (cur.cache_read ?? 0) + (m.cache_read ?? 0);
      cur.cache_write = (cur.cache_write ?? 0) + (m.cache_write ?? 0);
    }
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
  let usdBrl: number | null = null;

  for (const r of results) {
    if (!r.report) { partial = true; continue; }
    usdBrl ??= r.report.usd_brl ?? null;
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

  return { accounts, partial, usdBrl };
}

// Preenche buracos de data na lista de buckets diários (desc) com dias zerados, pra série
// visual ficar contínua. Só faz sentido no período "dia" — semana/mês ficam como estão.
export function fillDayGaps(list: CostBucket[]): CostBucket[] {
  if (list.length < 2) return list;
  const zero = (key: string): CostBucket => ({
    key, sessions: 0, input: 0, output: 0, cache_read: 0, cache_write: 0, cost: 0,
  });
  const fmt = (d: Date) =>
    `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
  const out: CostBucket[] = [];
  for (let i = 0; i < list.length; i++) {
    out.push(list[i]);
    const next = list[i + 1];
    if (!next) break;
    const d = new Date(`${list[i].key}T00:00:00`);
    // guarda de sanidade: key malformada não pode virar loop infinito
    for (let g = 0; g < 366; g++) {
      d.setDate(d.getDate() - 1);
      const k = fmt(d);
      if (k <= next.key) break;
      out.push(zero(k));
    }
  }
  return out;
}
