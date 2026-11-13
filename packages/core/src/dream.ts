import { apiFetchForServer } from './api';
import type { Server } from './servers';
import { fmtWhen } from './format';
import * as m from './paraglide/messages';

export interface DreamConfig {
  enabled: boolean;
  /** Mascarada; devolvida igual no PUT, mantém a chave gravada. */
  api_key: string;
  hour: string;
  project_cap_usd: number;
  month_cap_usd: number | null;
  /** Id da máquina que sonha, o mesmo em todas. */
  machine: string;
  /** Id deste servidor, para casar a escolha com a lista de servidores do app. */
  this_machine: string;
}
export type DreamConfigPatch = Partial<Omit<DreamConfig, 'this_machine'>>;
export type DreamMode = 'key' | 'account';
export interface DreamProject {
  id: string; name: string; machines: string[]; new_sessions: number; enabled: boolean; mode: DreamMode;
  last_dream_at: string | null; last_cost_usd: number; last_run_id: string | null; last_error: string | null; needs_confirm: boolean;
}
export interface DreamMonth { month: string; key_usd: number; account_usd: number }
export interface DreamSuggestion { text: string; conflicts_with: string }
export interface DreamStatus { projects: DreamProject[]; month: DreamMonth; running: string | null; suggestions: DreamSuggestion[]; last_round_at: string | null }
export interface DreamPlace {
  machine: string; root: string; dir: string | null;
  before: Record<string, string>; after: Record<string, string>; written: boolean; error: string | null;
}
export interface DreamRunProject { project: string; at: string; cost_usd: number; places: DreamPlace[] }
export interface DreamRun { run_id: string; projects: DreamRunProject[] }
export interface DreamUndoResult { project: string; machine: string; root: string; ok: boolean; error: string | null }
export interface DreamProjectPatch { enabled?: boolean; mode?: DreamMode; alias_of?: string }

const json = (method: string, body: unknown): RequestInit => ({ method, body: JSON.stringify(body) });
export const getDreamConfig = (server: Server) => apiFetchForServer<DreamConfig>(server, '/api/dream/config');
export const putDreamConfig = (server: Server, patch: DreamConfigPatch) => apiFetchForServer<DreamConfig>(server, '/api/dream/config', json('PUT', patch));
export const getDreamStatus = (server: Server) => apiFetchForServer<DreamStatus>(server, '/api/dream/status');
export const startDream = (server: Server, project?: string) => apiFetchForServer<{ run_id: string }>(server, '/api/dream/run', json('POST', project ? { project } : {}));
export const getDreamRun = (server: Server, runId: string) => apiFetchForServer<DreamRun>(server, `/api/dream/runs/${encodeURIComponent(runId)}`);
/** Com `project`, só aquele projeto volta: a rodada da noite cobre vários. */
export const undoDreamRun = (server: Server, runId: string, project?: string) => apiFetchForServer<{ results: DreamUndoResult[] }>(server,
  `/api/dream/runs/${encodeURIComponent(runId)}/undo${project ? `?project=${encodeURIComponent(project)}` : ''}`, { method: 'POST' });
export const putDreamProject = (server: Server, id: string, patch: DreamProjectPatch) => apiFetchForServer<{ ok: true }>(server, `/api/dream/project?id=${encodeURIComponent(id)}`, json('PUT', patch));
export const dismissDreamSuggestion = (server: Server, index: number) => apiFetchForServer<{ ok: true }>(server, '/api/dream/suggestions/dismiss', json('POST', { index }));

// Mapa explícito: acesso dinâmico a `m` perde o tipo gerado pelo Paraglide.
const KNOWN: Record<string, (detail: string) => string> = {
  dream_running: () => m.dream_error_dream_running(),
  dream_not_dreamer: () => m.dream_error_dream_not_dreamer(),
  dream_no_budget: () => m.dream_error_dream_no_budget(),
  dream_project_cap: () => m.dream_error_dream_project_cap(),
  dream_memory_changed: (detail) => m.dream_error_dream_memory_changed({ detail }),
  dream_rejected: () => m.dream_error_dream_rejected(),
  dream_no_credit: () => m.dream_error_dream_no_credit(),
  dream_account_limited: () => m.dream_error_dream_account_limited(),
};

/** Texto de um `last_error` ou mensagem do servidor no formato `código: detalhe`. */
export function dreamErrorText(raw: string): string {
  const i = raw.indexOf(':');
  const code = (i < 0 ? raw : raw.slice(0, i)).trim();
  const known = KNOWN[code];
  return known ? known(i < 0 ? '' : raw.slice(i + 1).trim()) : m.dream_error_generic({ detail: raw });
}

/** Texto do erro lançado por uma chamada do sonho (`apiFetchForServer`). */
export function dreamFailure(error: unknown): string {
  const e = (error ?? {}) as { code?: string; message?: string; envelope?: { msg?: unknown; params?: { field?: unknown } } };
  if (e.code === 'dream_invalid_config') return e.envelope?.params?.field === 'hour' ? m.dream_invalid_hour() : m.dream_invalid_cap();
  // O servidor manda `código: detalhe` no msg; sem ele, o código sozinho.
  if (e.code?.startsWith('dream_')) return dreamErrorText(typeof e.envelope?.msg === 'string' ? e.envelope.msg : e.code);
  return m.dream_error_generic({ detail: e.message || String(error) });
}

export type DreamFileChange = { file: string; kind: 'added' | 'removed' | 'changed' };
/** Só os arquivos que mudaram, em ordem de nome. */
export function dreamFileChanges(before: Record<string, string>, after: Record<string, string>): DreamFileChange[] {
  const files = [...new Set([...Object.keys(before), ...Object.keys(after)])].sort();
  const out: DreamFileChange[] = [];
  for (const file of files) {
    if (!(file in before)) out.push({ file, kind: 'added' });
    else if (!(file in after)) out.push({ file, kind: 'removed' });
    else if (before[file] !== after[file]) out.push({ file, kind: 'changed' });
  }
  return out;
}

export const dreamUsd = (value: number) => value.toFixed(2);
export const dreamLastText = (at: string | null, cost: number) =>
  at ? m.dream_last({ when: fmtWhen(Date.parse(at) / 1000), cost: dreamUsd(cost) }) : m.dream_never();
/** Gasto do mês; com teto, a chave mostra `gasto/teto`. */
export const dreamMonthText = (month: DreamMonth, cap: number | null) =>
  m.dream_month_value({ key: cap === null ? dreamUsd(month.key_usd) : `${dreamUsd(month.key_usd)}/${dreamUsd(cap)}`, account: dreamUsd(month.account_usd) });

export const validDreamHour = (hour: string) => /^([01]\d|2[0-3]):[0-5]\d$/.test(hour);
/** `null` = campo vazio; `undefined` = inválido. Aceita vírgula decimal do teclado brasileiro. */
export function parseDreamCap(text: string): number | null | undefined {
  const t = text.trim();
  if (!t) return null;
  const value = Number(t.replace(',', '.'));
  return Number.isFinite(value) && value > 0 ? value : undefined;
}

export type DreamDiffLine = { kind: ' ' | '-' | '+'; text: string };
/** Antes e depois de um arquivo, linha a linha. */
// shortcut: tabela LCS O(n·m), cabe nas notas de memória (centenas de linhas); trocar por Myers se vierem arquivos grandes.
export function dreamLineDiff(before: string, after: string): DreamDiffLine[] {
  const a = before ? before.split('\n') : [];
  const b = after ? after.split('\n') : [];
  const t = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) for (let j = b.length - 1; j >= 0; j--) t[i][j] = a[i] === b[j] ? t[i + 1][j + 1] + 1 : Math.max(t[i + 1][j], t[i][j + 1]);
  const out: DreamDiffLine[] = [];
  let i = 0, j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) { out.push({ kind: ' ', text: a[i] }); i++; j++; }
    else if (t[i + 1][j] >= t[i][j + 1]) out.push({ kind: '-', text: a[i++] });
    else out.push({ kind: '+', text: b[j++] });
  }
  while (i < a.length) out.push({ kind: '-', text: a[i++] });
  while (j < b.length) out.push({ kind: '+', text: b[j++] });
  return out;
}
