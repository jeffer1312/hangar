import { describe, expect, it } from 'vitest';
import { mergeReports, type ComboRow } from '@hangar/core';
import { comparison, compactMoney, compactTokens, costView, filterCombos, monthForecast, outlierDays, priceLabel, setCostFilter, sumCombos } from './costs-model';

const combo = (patch: Partial<ComboRow> = {}): ComboRow => ({ dia: '2026-10-01', provider: 'a', source: 'claude', project: '/one', model: 'known', subagente: false, sessions: 1, session_ids: ['same-id'], input: 100, output: 20, cache_write: 0, cache_read: 10, cost: 2, cost_input: 1, cost_output: 1, cost_cache_write: 0, cost_cache_read: 0, ...patch });
const report = () => mergeReports([
  { id: 'one', report: { applied: { period: '30d' }, totals: { ...sumCombos([]), sessions: 2, cost: 6 }, combos: [combo(), combo({ dia: '2026-10-02' }), combo({ provider: 'b', source: 'codex', project: '/two', session_ids: ['other'], subagente: true })] } },
  { id: 'two', report: { applied: { period: '30d' }, totals: { ...sumCombos([]), sessions: 1, cost: 2 }, combos: [combo()] } },
], '30d').report;

describe('recortes de custos mobile', () => {
  it('cruza dimensões com OR interno e conta ids por máquina, não por dia', () => {
    const r = report();
    const rows = filterCombos(r.combos, { provider: ['a', 'b'], source: ['claude'], project: ['/one'], sub: false });
    expect(rows).toHaveLength(3);
    expect(sumCombos(rows)).toMatchObject({ sessions: 2, input: 300, cost: 6 });
    expect(costView(r, { source: ['codex'], sub: true }).total).toMatchObject({ sessions: 1, cost: 2 });
    expect(costView(r, { source: ['codex'], project: ['/one'] }).total.cost).toBe(0);
  });
  it('sem combos permite só um valor e mantém painéis no período inteiro', () => {
    const r = { ...report(), combos: [], by_provider: [{ ...sumCombos([]), key: 'a', cost: 3 }], by_day: [{ ...sumCombos([]), key: '2026-10-01', cost: 8 }] };
    const f = setCostFilter({ source: ['claude'], sub: true }, 'provider', ['a', 'b'], false);
    expect(f).toEqual({ provider: ['a'] });
    expect(costView(r, f)).toMatchObject({ total: { cost: 3 }, days: r.by_day, cross: false });
  });
  it('comparação ignora seu próprio filtro mas conserva os demais e não passa de quatro', () => {
    const r = report();
    const c = comparison(r, { provider: ['a'], source: ['codex'] }, 'provider', ['b', 'a', 'c', 'd', 'e']);
    expect(c).toHaveLength(4);
    expect(c[0].total.cost).toBe(2);
    expect(c[0].days[0].key).toBe('2026-10-01');
    expect(c[1].total.cost).toBe(0);
  });
  it('volume sem tarifa é desconhecido, não zero grátis', () => {
    const b = { ...sumCombos([]), input: 42 };
    expect(priceLabel(b, 'missing')).toBe('unknown');
    expect(priceLabel(b, 'kimi-free:high')).toBe('free');
    expect(priceLabel({ ...b, cost: 1 }, 'known')).toBe('priced');
  });
});

describe('compactTokens', () => {
  it('escreve igual ao tok() do nativo em português', () => {
    expect(compactTokens(1_934_283_576, 'pt-BR')).toBe('1,93 Bi');
    expect(compactTokens(68_608_504_332, 'pt-BR')).toBe('68,61 Bi');
    expect(compactTokens(246_920_986, 'pt-BR')).toBe('246,9 Mi');
    expect(compactTokens(12_400, 'pt-BR')).toBe('12 mil');
    expect(compactTokens(950, 'pt-BR')).toBe('950');
  });
  it('usa B, M e k em inglês', () => {
    expect(compactTokens(1_934_283_576, 'en-US')).toBe('1.93 B');
    expect(compactTokens(2_500_000, 'en-US')).toBe('2.5 M');
    expect(compactTokens(1_200, 'en-US')).toBe('1k');
  });
  it('sobe de unidade quando o arredondamento chega a mil', () => {
    expect(compactTokens(999_600, 'pt-BR')).toBe('1,0 Mi');
    expect(compactTokens(999_960_000, 'pt-BR')).toBe('1,00 Bi');
    expect(compactTokens(999.5, 'pt-BR')).toBe('1 mil');
  });
});

describe('dinheiro compacto', () => {
  it('escreve igual ao money() do nativo', () => {
    expect(compactMoney(13_654.4, 'pt-BR')).toBe('13,7 mil');
    expect(compactMoney(13_654.4, 'en-US')).toBe('13.7K');
    expect(compactMoney(2_000_000, 'pt-BR')).toBe('2 mi');
    expect(compactMoney(950, 'pt-BR')).toBe('950');
    expect(compactMoney(0.04, 'en-US')).toBe('0.04');
  });
  it('sobe de unidade quando o arredondamento chega a mil', () => {
    expect(compactMoney(999_960, 'pt-BR')).toBe('1 mi');
    expect(compactMoney(999_960, 'en-US')).toBe('1M');
  });
});

describe('previsão do mês', () => {
  const days = [{ key: '2026-10-31', cost: 50 }, ...Array.from({ length: 10 }, (_, i) => ({ key: `2026-11-${String(i + 1).padStart(2, '0')}`, cost: 10 }))];
  it('soma o mês até hoje e projeta pela média dos últimos 7 dias', () => {
    const f = monthForecast(days, '2026-11-10', false)!;
    expect(f.mtd).toBe(100);
    expect(f.projected).toBe(300);
    expect(f.daysInMonth).toBe(30);
    expect(f.cumulative).toHaveLength(10);
    expect(f.cumulative.at(-1)).toBe(100);
    expect(f.lastMonth).toBeNull();
  });
  it('mês passado só quando o período cobre o mês inteiro', () => {
    expect(monthForecast(days, '2026-11-10', true)!.lastMonth).toBeNull();
    expect(monthForecast([{ key: '2026-10-01', cost: 5 }, ...days], '2026-11-10', true)!.lastMonth).toBe(55);
  });
  it('dia sem dado conta como zero na média', () => {
    expect(monthForecast([{ key: '2026-11-10', cost: 70 }], '2026-11-10', false)!.projected).toBe(70 + 10 * 20);
  });
  it('sem dia do mês corrente não há previsão', () => {
    expect(monthForecast([{ key: '2026-10-31', cost: 5 }], '2026-11-10', false)).toBeNull();
  });
});

describe('dia fora da curva', () => {
  it('marca o dia acima de média + 2 desvios', () => {
    const values = [...Array.from({ length: 10 }, (_, i) => ({ key: `d${i}`, value: 10 })), { key: 'pico', value: 100 }];
    expect(outlierDays(values)).toEqual(['pico']);
  });
  it('com menos de 7 dias não marca nada', () => {
    expect(outlierDays([{ key: 'a', value: 1 }, { key: 'b', value: 100 }])).toEqual([]);
  });
});
