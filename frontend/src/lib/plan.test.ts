import { describe, expect, it } from 'vitest';
import { planBadge } from './plan';
import type { SessionInfo } from './types';

const base = { name: 's', state: 'idle' } as unknown as SessionInfo;

describe('planBadge', () => {
  it('devolve null sem plano', () => {
    expect(planBadge(base)).toBeNull();
    expect(planBadge(null)).toBeNull();
  });

  it('monta rótulo, pct e title', () => {
    const b = planBadge({ ...base, plan_name: 'git-stash-manager', plan_task: 2,
      plan_task_total: 3, plan_done: 9, plan_total: 17, plan_complete: false })!;
    // O NOME vai no rotulo, nao so no title: no celular nao ha hover, entao o title nunca e lido e
    // nada dizia QUAL plano estava rodando.
    expect(b.label).toBe('📋 git-stash-manager · Task 2/3');
    expect(Math.round(b.pct)).toBe(53);
    expect(b.title).toBe('git-stash-manager · Task 2/3 · 9/17 steps');
    expect(b.complete).toBe(false);
  });

  it('total 0 não divide por zero', () => {
    expect(planBadge({ ...base, plan_name: 'x', plan_done: 0, plan_total: 0 })).toBeNull();
  });

  it('plano concluído marca complete e 100%', () => {
    const b = planBadge({ ...base, plan_name: 'x', plan_task: 3, plan_task_total: 3,
      plan_done: 17, plan_total: 17, plan_complete: true })!;
    expect(b.pct).toBe(100);
    expect(b.complete).toBe(true);
    expect(b.label).toBe('📋 x · concluído');
  });

  it('sem task_total cai no rótulo de steps e não duplica no title', () => {
    const b = planBadge({ ...base, plan_name: 'x', plan_done: 3, plan_total: 10 })!;
    expect(b.label).toBe('📋 x · 3/10');
    expect(b.title).toBe('x · 3/10 steps');
  });
});
