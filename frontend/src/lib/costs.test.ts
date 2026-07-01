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
    expect(merged.partial).toBe(false);
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
