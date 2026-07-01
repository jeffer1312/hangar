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
    expect(merged.accounts[0].by_model).toEqual([{ model: 'opus', sessions: 2, cost: 8 }]);
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
