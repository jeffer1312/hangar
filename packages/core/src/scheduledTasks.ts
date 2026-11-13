import { apiFetchForServer, type CreateSessionBody } from './api';
import type { Server } from './servers';

export type TaskSchedule = { kind: 'once'; at: string } | { kind: 'weekly'; time: string; weekdays: number[] } | { kind: 'interval'; minutes: number };
export type TaskLaunch = Omit<CreateSessionBody, 'name' | 'prewarm' | 'remember_provider' | 'branch' | 'new_branch' | 'base'>;
export type TaskTarget = { kind: 'new'; launch: TaskLaunch; worktree: boolean; base?: string } | { kind: 'session'; name: string; session_key?: string; conversation?: string };
export interface ScheduledTaskInput { name: string; prompt: string; enabled: boolean; schedule: TaskSchedule; target: TaskTarget }
export interface ScheduledTask extends ScheduledTaskInput {
  id: string; revision: number; next_run_at: string | null; last_run_at: string | null;
  last_status: 'never' | 'dispatching' | 'sent' | 'failed' | 'uncertain';
  last_error: string | null; last_session: string | null;
}
export interface ScheduledTasksList { tasks: ScheduledTask[]; server_time: string; timezone: string }
const root = '/api/scheduled-tasks';
const taskPath = (id: string) => `${root}/${encodeURIComponent(id)}`;
export const getScheduledTasks = (server: Server) => apiFetchForServer<ScheduledTasksList>(server, root);
/** `request_id` repetido devolve a tarefa já criada: a nova tentativa após resposta perdida não duplica. */
export const createScheduledTask = (server: Server, input: ScheduledTaskInput & { request_id?: string }) => apiFetchForServer<ScheduledTask>(server, root, { method: 'POST', body: JSON.stringify(input) });
export const updateScheduledTask = (server: Server, id: string, revision: number, input: ScheduledTaskInput) => apiFetchForServer<ScheduledTask>(server, taskPath(id), { method: 'PUT', body: JSON.stringify({ ...input, revision }) });
export const setScheduledTaskEnabled = (server: Server, id: string, revision: number, enabled: boolean) => apiFetchForServer<ScheduledTask>(server, `${taskPath(id)}/enabled`, { method: 'PATCH', body: JSON.stringify({ enabled, revision }) });
export const runScheduledTask = (server: Server, id: string) => apiFetchForServer<{ ok: true }>(server, `${taskPath(id)}/run`, { method: 'POST' });
export const deleteScheduledTask = (server: Server, id: string, revision: number) => apiFetchForServer<void>(server, `${taskPath(id)}?revision=${revision}`, { method: 'DELETE' });

// Cada leitura traz a lista inteira: sem reaproveitar o que não mudou, toda linha ganha objeto novo e o memo não segura nada.
export function reuseUnchangedTasks(prev: readonly ScheduledTask[] | undefined, next: ScheduledTask[]): ScheduledTask[] {
  const old = new Map(prev?.map((task) => [task.id, task]));
  return next.map((task) => {
    const kept = old.get(task.id);
    return kept && JSON.stringify(kept) === JSON.stringify(task) ? kept : task;
  });
}

export type TaskValidation = 'name' | 'prompt' | 'days' | 'time' | 'interval' | 'date' | 'folder' | 'session';
const validTime = (time: string) => /^([01]\d|2[0-3]):[0-5]\d$/.test(time);
function validInstant(at: string): boolean {
  const match = /^(\d{4})-(\d{2})-(\d{2})T([01]\d|2[0-3]):([0-5]\d):([0-5]\d)(?:\.\d+)?(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$/.exec(at);
  if (!match || !Number.isFinite(Date.parse(at))) return false;
  const day = new Date(Date.UTC(+match[1], +match[2] - 1, +match[3]));
  return day.getUTCMonth() === +match[2] - 1 && day.getUTCDate() === +match[3];
}
export function validateScheduledTask(task: ScheduledTaskInput, now = Date.now(), original?: ScheduledTaskInput): TaskValidation | null {
  if (!task.name.trim()) return 'name';
  if (!task.prompt.trim()) return 'prompt';
  const s = task.schedule;
  if (s.kind === 'weekly') {
    if (!s.weekdays.length || s.weekdays.some((d) => !Number.isInteger(d) || d < 0 || d > 6)) return 'days';
    if (!validTime(s.time)) return 'time';
  } else if (s.kind === 'interval') {
    if (!Number.isSafeInteger(s.minutes) || s.minutes < 1) return 'interval';
  } else {
    const unchanged = original?.schedule.kind === 'once' && original.schedule.at === s.at;
    if (!validInstant(s.at) || (!unchanged && Date.parse(s.at) <= now)) return 'date';
  }
  if (task.target.kind === 'new' && !task.target.launch.cwd?.trim()) return 'folder';
  if (task.target.kind === 'session' && !task.target.name.trim()) return 'session';
  return null;
}

/** A data pertence à máquina escolhida, mesmo quando o celular está em outro fuso. */
export function taskDateParts(instant: string, timezone: string): { date: string; time: string } {
  const parts = new Intl.DateTimeFormat('en-GB', { timeZone: timezone, year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(new Date(instant));
  const part = (kind: Intl.DateTimeFormatPartTypes) => parts.find((p) => p.type === kind)!.value;
  return { date: `${part('day')}/${part('month')}/${part('year')}`, time: `${part('hour')}:${part('minute')}` };
}

export function taskDateToInstant(date: string, time: string, timezone: string): string | null {
  const match = /^(\d{2})\/(\d{2})\/(\d{4})$/.exec(date);
  if (!match || !validTime(time)) return null;
  const [, day, month, year] = match;
  const wall = Date.UTC(+year, +month - 1, +day, +time.slice(0, 2), +time.slice(3));
  if (!validInstant(`${year}-${month}-${day}T${time}:00Z`)) return null;
  try {
    const offsets = new Set<number>();
    // Os dois lados da transição precisam participar, inclusive quando a mudança não é de uma hora.
    for (let hours = -48; hours <= 48; hours += 6) {
      const sample = wall + hours * 3600000;
      const shown = taskDateParts(new Date(sample).toISOString(), timezone);
      const [d, mo, y] = shown.date.split('/').map(Number);
      const observed = Date.UTC(y, mo - 1, d, +shown.time.slice(0, 2), +shown.time.slice(3));
      offsets.add(observed - sample);
    }
    const candidates = [...offsets].map((offset) => new Date(wall - offset).toISOString()).filter((instant) => {
      const shown = taskDateParts(instant, timezone);
      return shown.date === date && shown.time === time;
    });
    return candidates.length === 1 ? candidates[0] : null;
  } catch { return null; }
}
