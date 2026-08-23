// Helpers puros do painel do plano — o que dá pra testar sem render.
// Espelha a lógica de PlanPanel.svelte: current = detail.task - 1

import type { PlanDetail } from '@hangar/core';

export function currentIndex(detail: PlanDetail | null | undefined): number {
  return detail ? detail.task - 1 : -1;
}

export function taskMark(done: number, total: number, isCurrent: boolean): string {
  if (total > 0 && done >= total) return '✓';
  if (isCurrent) return '◐';
  return '○';
}

export function stepMark(done: boolean): string {
  return done ? '✓' : '○';
}

// rótulo do chip/barra: "Task N/M" ou "done/total" quando não há task
export function progressoRotulo(detail: PlanDetail | null): string | null {
  if (!detail) return null;
  if (detail.task_total > 0) return `Task ${detail.task}/${detail.task_total}`;
  return `${detail.done}/${detail.total}`;
}
