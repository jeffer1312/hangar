// Helpers puros do progresso do plano (app/planprog.py no backend). Espelha lib/loop.ts: o rótulo e
// a porcentagem são montados aqui, os componentes só renderizam.

import type { SessionInfo } from './types';

export interface PlanBadge {
  label: string;
  pct: number;        // 0..100
  title: string;      // tooltip: plano · Task N/M · done/total steps
  complete: boolean;
}

export type PlanCarrier = Pick<SessionInfo,
  'plan_name' | 'plan_task' | 'plan_task_total' | 'plan_done' | 'plan_total' | 'plan_complete'>;

// null = sem plano (ou payload incoerente) — o chamador esconde chip e barra. O guard de total <= 0
// existe porque done/total vira NaN no width do CSS, e um NaN% não erra visivelmente: some.
export function planBadge(s: PlanCarrier | null | undefined): PlanBadge | null {
  if (!s || !s.plan_name) return null;
  const total = s.plan_total ?? 0;
  const done = s.plan_done ?? 0;
  if (total <= 0) return null;
  const pct = Math.max(0, Math.min(100, (done / total) * 100));
  const complete = s.plan_complete === true || done >= total;
  const hasTask = s.plan_task != null && s.plan_task_total != null;
  const task = hasTask ? `Task ${s.plan_task}/${s.plan_task_total}` : `${done}/${total}`;
  return {
    label: complete ? '📋 concluído' : `📋 ${task}`,
    pct,
    // sem task, o rótulo já É done/total — repetir daria "x · 3/10 · 3/10 steps"
    title: hasTask ? `${s.plan_name} · ${task} · ${done}/${total} steps`
                   : `${s.plan_name} · ${done}/${total} steps`,
    complete,
  };
}
