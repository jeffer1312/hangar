import { describe, it, expect } from 'vitest';
import { currentIndex, taskMark, stepMark } from './planPanel';
import type { PlanDetail } from '@hangar/core';

function fakeDetail(task: number, task_total: number): PlanDetail {
  return {
    name: 'p',
    path: '/tmp/p.md',
    task,
    task_total,
    done: 0,
    total: 10,
    complete: false,
    tasks: [],
    markdown: '',
  } as unknown as PlanDetail;
}

describe('planPanel', () => {
  it('current = detail.task - 1', () => {
    expect(currentIndex(fakeDetail(1, 3))).toBe(0);
    expect(currentIndex(fakeDetail(3, 3))).toBe(2);
    expect(currentIndex(null)).toBe(-1);
  });

  it('taskMark usa ✓ para concluída e ◐ para atual', () => {
    expect(taskMark(5, 5, false)).toBe('✓');
    expect(taskMark(0, 5, true)).toBe('◐');
    expect(taskMark(0, 5, false)).toBe('○');
  });

  it('stepMark distingue concluída', () => {
    expect(stepMark(true)).toBe('✓');
    expect(stepMark(false)).toBe('○');
  });
});
