import { describe, it, expect } from 'vitest';
import { mergeAccounts, type ServerResult } from './costs';
import type { AccountCost, CostBucket } from './types';

function bucket(key: string, cost: number): CostBucket {
  return { key, sessions: 1, input: 0, output: 0, cache_read: 0, cache_write: 0, cost };
}

function acc(id: string, cost: number, days: CostBucket[]): AccountCost {
  return {
    account_id: id, email: `${id}@x.com`, label: id,
    totals: { key: 'totals', sessions: 1, input: 0, output: 0, cache_read: 0, cache_write: 0, cost },
    today: 0, yesterday: 0,
    by_day: days, by_week: [], by_month: [],
    by_model: [{ model: 'opus', sessions: 1, cost }],
  };
}

describe('mergeAccounts', () => {
  it('sums the same account across servers by day key', () => {
    const a: ServerResult = { report: { accounts: [acc('u1', 5, [bucket('2026-07-01', 5)])] } };
    const b: ServerResult = { report: { accounts: [acc('u1', 3, [bucket('2026-07-01', 3)])] } };
    const merged = mergeAccounts([a, b]);
    expect(merged.accounts).toHaveLength(1);
    expect(merged.accounts[0].totals.cost).toBe(8);
    expect(merged.accounts[0].by_day).toHaveLength(1);
    expect(merged.accounts[0].by_day[0].cost).toBe(8);
    // Tokens entram zerados mesmo quando a origem não os mandou: addModels NORMALIZA na entrada,
    // pra um `+=` posterior de servidor novo não bater em undefined e virar NaN.
    expect(merged.accounts[0].by_model).toEqual([
      { model: 'opus', sessions: 2, cost: 8, input: 0, output: 0, cache_read: 0, cache_write: 0 },
    ]);
    expect(merged.partial).toBe(false);
  });

  it('sorts by_day desc by key and by_model desc by cost', () => {
    const account: AccountCost = {
      account_id: 'u1', email: null, label: 'u1',
      totals: { key: 'totals', sessions: 2, input: 0, output: 0, cache_read: 0, cache_write: 0, cost: 9 },
      today: 0, yesterday: 0,
      by_day: [bucket('2026-06-30', 4), bucket('2026-07-02', 5)], // fora de ordem
      by_week: [], by_month: [],
      by_model: [{ model: 'sonnet', sessions: 1, cost: 3 }, { model: 'opus', sessions: 1, cost: 6 }], // fora de ordem
    };
    const merged = mergeAccounts([{ report: { accounts: [account] } }]);
    const a = merged.accounts[0];
    expect(a.by_day.map((b) => b.key)).toEqual(['2026-07-02', '2026-06-30']); // key desc
    expect(a.by_model.map((m) => m.model)).toEqual(['opus', 'sonnet']); // cost desc
  });

  it('keeps different accounts separate', () => {
    const a: ServerResult = { report: { accounts: [acc('u1', 5, [])] } };
    const b: ServerResult = { report: { accounts: [acc('u2', 3, [])] } };
    const merged = mergeAccounts([a, b]);
    expect(merged.accounts).toHaveLength(2);
  });

  it('marks partial when a server failed', () => {
    const a: ServerResult = { report: { accounts: [acc('u1', 5, [])] } };
    const failed: ServerResult = { report: null };
    const merged = mergeAccounts([a, failed]);
    expect(merged.partial).toBe(true);
    expect(merged.accounts).toHaveLength(1);
  });
});

describe('addModels (via mergeAccounts) — tokens por modelo', () => {
  // Fábrica local: o acc() do topo fixa by_model sem tokens, que é justamente o caso legado.
  function comModelo(id: string, m: Partial<AccountCost['by_model'][0]> & { model: string }): AccountCost {
    return {
      account_id: id, email: `${id}@x.com`, label: id,
      totals: { key: 'totals', sessions: 1, input: 0, output: 0, cache_read: 0, cache_write: 0, cost: 0 },
      today: 0, yesterday: 0, by_day: [], by_week: [], by_month: [],
      by_model: [{ sessions: 1, cost: 1, ...m }],
    };
  }

  it('soma os tokens da MESMA conta espalhada em servidores diferentes', () => {
    // O bug: o ramo de merge só acumulava sessions/cost, então os tokens do 2º servidor em diante
    // eram descartados calados — e só no caminho "conta específica", nunca no modo "Todas".
    const s1 = { report: { accounts: [comModelo('u1', { model: 'opus', input: 100, output: 10, cache_write: 5, cache_read: 1000 })] } };
    const s2 = { report: { accounts: [comModelo('u1', { model: 'opus', input: 200, output: 20, cache_write: 7, cache_read: 3000 })] } };

    const [m] = mergeAccounts([s1, s2]).accounts[0].by_model;
    expect(m.input).toBe(300);
    expect(m.output).toBe(30);
    expect(m.cache_write).toBe(12);
    expect(m.cache_read).toBe(4000);
    expect(m.sessions).toBe(2);
  });

  it('servidor legado (sem tokens) não zera nem NaN-ifica o que o novo mandou', () => {
    // Ordem legado-primeiro é a pior: guardar o undefined dele congelava o campo pra sempre.
    const legado = { report: { accounts: [comModelo('u1', { model: 'opus' })] } };
    const novo = { report: { accounts: [comModelo('u1', { model: 'opus', input: 500, cache_read: 42 })] } };

    const [m] = mergeAccounts([legado, novo]).accounts[0].by_model;
    expect(m.input).toBe(500);
    expect(m.cache_read).toBe(42);
    expect(Number.isNaN(m.input)).toBe(false);
    expect(Number.isNaN(m.output)).toBe(false);
  });
});
