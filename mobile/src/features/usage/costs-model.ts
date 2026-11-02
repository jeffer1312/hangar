import { custoDesconhecido, isFree, type ComboLocal, type DimBucket, type RelatorioMesclado } from '@hangar/core';

export const dimensions = ['provider', 'source', 'project', 'model', 'servidor'] as const;
export type Dimension = typeof dimensions[number];
export type CostFilter = Partial<Record<Dimension, string[]>> & { sub?: boolean };
// À mão, na regra do `tok()` do nativo: o Intl do Hermes no iOS ignora `notation: 'compact'`.
// Os cortes olham o valor já arredondado ("999,6 mil" sobe para "1,0 Mi").
export function compactTokens(value: number, locale: string): string {
  const en = locale.startsWith('en');
  const fixed = (n: number, digits: number) => n.toLocaleString(locale, { minimumFractionDigits: digits, maximumFractionDigits: digits });
  if (value >= 999.95e6) return `${fixed(value / 1e9, 2)} ${en ? 'B' : 'Bi'}`;
  if (value >= 999.5e3) return `${fixed(value / 1e6, 1)} ${en ? 'M' : 'Mi'}`;
  if (value >= 999.5) return `${fixed(value / 1e3, 0)}${en ? 'k' : ' mil'}`;
  return fixed(value, 0);
}
// Grandeza de dinheiro na regra do `money()` do nativo: até centavos abaixo de mil (US$ 0,04 não vira 0), depois uma casa.
export function compactMoney(value: number, locale: string): string {
  const en = locale.startsWith('en');
  const abs = Math.abs(value);
  if (abs < 999.95) return value.toLocaleString(locale, { maximumFractionDigits: 2 });
  // Cortes no valor já arredondado a uma casa: 999.960 é "1 mi", não "1.000 mil".
  const [size, unit] = abs >= 999.95e6 ? [1e9, en ? 'B' : ' bi'] : abs >= 999.95e3 ? [1e6, en ? 'M' : ' mi'] : abs >= 999.95 ? [1e3, en ? 'K' : ' mil'] : [1, ''];
  return `${(value / size).toLocaleString(locale, { maximumFractionDigits: 1 })}${unit}`;
}
const dayKey = (date: Date) => date.toISOString().slice(0, 10);
// Projeção = gasto do mês até hoje + média dos últimos 7 dias × dias que faltam (dia sem dado vale zero).
export function monthForecast(days: { key: string; cost: number }[], today: string, coversPrevMonth: boolean) {
  const month = today.slice(0, 7);
  const byKey = new Map(days.map((d) => [d.key, d.cost]));
  if (!days.some((d) => d.key.startsWith(month) && d.key <= today)) return null;
  const now = new Date(`${today}T12:00:00Z`);
  const day = now.getUTCDate();
  const daysInMonth = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth() + 1, 0)).getUTCDate();
  const cumulative: number[] = [];
  for (let d = 1; d <= day; d++) cumulative.push((cumulative.at(-1) ?? 0) + (byKey.get(`${month}-${String(d).padStart(2, '0')}`) ?? 0));
  let week = 0;
  for (let i = 0; i < 7; i++) week += byKey.get(dayKey(new Date(now.getTime() - i * 86_400_000))) ?? 0;
  const mtd = cumulative.at(-1)!;
  const prev = dayKey(new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth() - 1, 1))).slice(0, 7);
  // Histórico que começa depois do dia 1 do mês passado faria parecer um mês barato (ou de custo zero).
  const sinceStart = days.some((d) => d.key <= `${prev}-01`);
  const lastMonth = coversPrevMonth && sinceStart ? days.filter((d) => d.key.startsWith(prev)).reduce((sum, d) => sum + d.cost, 0) : null;
  return { mtd, projected: mtd + week / 7 * (daysInMonth - day), daysInMonth, cumulative, lastMonth };
}
export function outlierDays(values: { key: string; value: number }[]): string[] {
  if (values.length < 7) return [];
  const mean = values.reduce((sum, v) => sum + v.value, 0) / values.length;
  const sd = Math.sqrt(values.reduce((sum, v) => sum + (v.value - mean) ** 2, 0) / values.length);
  return sd > 0 ? values.filter((v) => v.value > mean + 2 * sd).map((v) => v.key) : [];
}
export const rawTokens = (b: { input: number; output: number; cache_write: number; cache_read: number }) => b.input + b.output + b.cache_write + b.cache_read;
const fields = ['input', 'output', 'cache_write', 'cache_read', 'cost', 'cost_input', 'cost_output', 'cost_cache_write', 'cost_cache_read', 'cache_write_1h', 'regravado', 'custo_regravado'] as const;
const zero = (key: string): DimBucket => ({ key, sessions: 0, input: 0, output: 0, cache_write: 0, cache_read: 0, cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0 });

// Sem combinações, os totais por dimensão não permitem cruzamentos.
export function setCostFilter(filter: CostFilter, dim: Dimension, values: string[], cross: boolean): CostFilter {
  const next = cross ? { ...filter } : {};
  if (values.length) next[dim] = cross ? values : values.slice(0, 1);
  else delete next[dim];
  return next;
}
export function filterCombos(rows: ComboLocal[], filter: CostFilter): ComboLocal[] {
  return rows.filter((row) => dimensions.every((dim) => !filter[dim]?.length || filter[dim]!.includes(row[dim]))
    && (filter.sub === undefined || row.subagente === filter.sub));
}
export function sumCombos(rows: ComboLocal[]): DimBucket {
  const total = zero('totals');
  const seen = new Set<string>();
  for (const row of rows) {
    if (row.session_ids) {
      for (const id of row.session_ids) {
        const key = JSON.stringify([row.servidor, id]);
        if (!seen.has(key)) { seen.add(key); total.sessions++; }
      }
    } else total.sessions += row.sessions;
    for (const field of fields) total[field] = (total[field] ?? 0) + (row[field] ?? 0);
  }
  return total;
}
export function groupCombos(rows: ComboLocal[], field: Dimension | 'dia'): DimBucket[] {
  const groups = new Map<string, ComboLocal[]>();
  for (const row of rows) {
    const list = groups.get(row[field]);
    if (list) list.push(row); else groups.set(row[field], [row]);
  }
  return [...groups].map(([key, list]) => ({ ...sumCombos(list), key }))
    .sort((a, b) => b.cost - a.cost || a.key.localeCompare(b.key));
}
export function dimensionBuckets(report: RelatorioMesclado, dim: Dimension): DimBucket[] {
  return { provider: report.by_provider, source: report.by_source, project: report.by_project, model: report.by_model, servidor: report.by_servidor }[dim];
}
export function costView(report: RelatorioMesclado, filter: CostFilter) {
  const cross = report.combos.length > 0;
  const rows = filterCombos(report.combos, filter);
  const dim = dimensions.find((d) => filter[d]?.length);
  const total = cross ? sumCombos(rows) : dim ? dimensionBuckets(report, dim).find((b) => b.key === filter[dim]![0]) ?? zero('totals') : report.totals;
  const groups = Object.fromEntries(dimensions.map((d) => [d, cross ? groupCombos(rows, d) : dimensionBuckets(report, d)])) as Record<Dimension, DimBucket[]>;
  return { total, groups, rows, cross, days: cross ? groupCombos(rows, 'dia').sort((a, b) => b.key.localeCompare(a.key)) : report.by_day };
}
export function comparison(report: RelatorioMesclado, filter: CostFilter, dim: Dimension, keys: string[]) {
  const rest = { ...filter };
  delete rest[dim];
  return [...new Set(keys)].slice(0, 4).map((key) => {
    const view = costView(report, { ...rest, [dim]: [key] });
    return { key, total: report.combos.length ? view.total : dimensionBuckets(report, dim).find((b) => b.key === key) ?? zero(key), days: report.combos.length ? view.days : [] };
  });
}
export function priceLabel(bucket: Pick<DimBucket, 'input' | 'output' | 'cache_write' | 'cache_read' | 'cost'>, model?: string): 'unknown' | 'free' | 'priced' {
  return custoDesconhecido({ ...zero(''), ...bucket }) ? model && isFree(model) ? 'free' : 'unknown' : 'priced';
}
