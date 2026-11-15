import { apiFetchForServer } from './api';
import { apiEnv, type EventSourceLike } from './apiEnv';
import { baseOf } from './rota';
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
  /** Passada que escreve o arquivo global de preferências; nasce desligada. */
  user_memory: boolean;
  /** Id deste servidor, para casar a escolha com a lista de servidores do app. */
  this_machine: string;
}
export type DreamConfigPatch = Partial<Omit<DreamConfig, 'this_machine'>>;
export type DreamMode = 'key' | 'account';
export interface DreamProject {
  /** `new_sessions` `null` = contagem desconhecida; o fato fica de fora. */
  id: string; name: string; machines: string[]; new_sessions: number | null; enabled: boolean; mode: DreamMode;
  last_dream_at: string | null; last_cost_usd: number; last_run_id: string | null; last_error: string | null; needs_confirm: boolean;
}
export interface DreamMonth { month: string; key_usd: number; account_usd: number }
export interface DreamSuggestion { text: string; conflicts_with: string }
export interface DreamStatus {
  projects: DreamProject[]; month: DreamMonth; running: string | null; progress?: DreamProgress | null; suggestions: DreamSuggestion[]; last_round_at: string | null;
}
export type DreamProjectStage = 'queued' | 'reading' | 'analyzing' | 'organizing' | 'writing' | 'done' | 'failed' | 'skipped' | 'stopped';
export interface DreamProjectProgress {
  /** `user` = a passada da memória do usuário. */
  id: string; name: string; status: DreamProjectStage; calls_total: number; calls_done: number; cost_usd: number;
  error: string | null; notes: { new: number; changed: number; removed: number } | null;
}
export interface DreamProgress {
  run_id: string; started_at: string; last_activity_at: string; spent_usd: number; stopping: boolean;
  current: number | null; projects: DreamProjectProgress[];
}
export interface DreamCallMeta {
  n: number; kind: 'analyzer' | 'organizer' | 'user'; label: string; model: string; status: 'running' | 'done' | 'failed';
  started_at: string; ended_at: string | null; cost_usd: number | null; error: string | null; steps: number;
}
export type DreamStep =
  | { kind: 'text'; text: string }
  | { kind: 'tool'; tool: string; target: string | null; detail: string | null };
export interface DreamCall { meta: DreamCallMeta; prompt: string; steps: DreamStep[] }
export const DREAM_USER_PROJECT = 'user';
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
export const stopDream = (server: Server) => apiFetchForServer<{ stopping: true }>(server, '/api/dream/stop', { method: 'POST' });
const callsPath = (runId: string, project: string, n?: number) =>
  `/api/dream/runs/${encodeURIComponent(runId)}/calls${n === undefined ? '' : `/${n}`}?project=${encodeURIComponent(project)}`;
export const getDreamCalls = (server: Server, runId: string, project: string) =>
  apiFetchForServer<{ calls: DreamCallMeta[] }>(server, callsPath(runId, project));
export const getDreamCall = (server: Server, runId: string, project: string, n: number) =>
  apiFetchForServer<DreamCall>(server, callsPath(runId, project, n));

/** Andamento ao vivo da máquina que sonha: um `progress` na abertura e a cada mudança (`null` = parada). */
export function openDreamEvents(server: Server, onProgress: (progress: DreamProgress | null) => void): EventSourceLike {
  const origin = apiEnv().origin;
  const base = baseOf(server);
  const sameOrigin = !!origin && (!base || base === origin);
  const url = `${base}/api/dream/events${sameOrigin ? '' : `?token=${encodeURIComponent(server.token)}`}`;
  const source = apiEnv().createEventSource(url, { withCredentials: sameOrigin });
  source.addEventListener('progress', (ev) => {
    try { onProgress(JSON.parse(ev.data) as DreamProgress | null); } catch { /* evento torto: espera o próximo */ }
  });
  return source;
}

export type DreamCallLabel =
  | { kind: 'conversation'; n: number; part: number | null }
  | { kind: 'notes'; machine: string }
  | { kind: 'organizer' } | { kind: 'user' } | { kind: 'other'; label: string };
/**
 * Nome da entrada que o servidor dá a cada analisador: `s{sessão}-p{parte}.txt` ou `memory-{máquina}-{n}.txt`.
 * `parts` = quantas partes cada sessão tem na lista; a parte só aparece quando a sessão tem mais de uma.
 */
export function dreamCallLabel(label: string, parts: Record<number, number> = {}): DreamCallLabel {
  if (label === 'organizer') return { kind: 'organizer' };
  if (label === 'user') return { kind: 'user' };
  const s = /^s(\d+)-p(\d+)\.txt$/.exec(label);
  if (s) { const n = Number(s[1]); return { kind: 'conversation', n: n + 1, part: (parts[n] ?? 1) > 1 ? Number(s[2]) + 1 : null }; }
  const notes = /^memory-(.+)-\d+\.txt$/.exec(label);
  return notes ? { kind: 'notes', machine: notes[1] } : { kind: 'other', label };
}
/** Partes por sessão, contadas nos rótulos da lista de chamadas. */
export function dreamCallParts(labels: string[]): Record<number, number> {
  const out: Record<number, number> = {};
  for (const l of labels) { const s = /^s(\d+)-p\d+\.txt$/.exec(l); if (s) out[Number(s[1])] = (out[Number(s[1])] ?? 0) + 1; }
  return out;
}
export function dreamCallTitle(label: DreamCallLabel, labelOf: (machine: string) => string = (x) => x): string {
  switch (label.kind) {
    case 'conversation': return label.part ? m.dream_call_conversation_part({ n: label.n, part: label.part }) : m.dream_call_conversation({ n: label.n });
    case 'notes': return m.dream_call_notes_from({ machine: labelOf(label.machine) });
    case 'organizer': return m.dream_call_organizer();
    case 'user': return m.dream_progress_user();
    default: return label.label;
  }
}

/** Início da rodada no horário local, `dd/MM HH:mm`, tirado do `run_id` (`YYYYMMDD-HHMMSS-xxxx`, em UTC). */
export function dreamRunWhen(runId: string): string {
  const p = /^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})(\d{2})/.exec(runId);
  if (!p) return runId;
  const at = new Date(Date.UTC(+p[1], +p[2] - 1, +p[3], +p[4], +p[5], +p[6]));
  const two = (n: number) => String(n).padStart(2, '0');
  return `${two(at.getDate())}/${two(at.getMonth() + 1)} ${two(at.getHours())}:${two(at.getMinutes())}`;
}

/** `claude-haiku-5-5` → `Haiku`. */
export const dreamModelName = (model: string) => {
  const family = /^claude-([a-z]+)/.exec(model)?.[1];
  return family ? family[0].toUpperCase() + family.slice(1) : model;
};

/** Duração curta (`8 s`, `11 min`, `1 h 05 min`) para o andamento. */
export function dreamSpan(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s} s`;
  const min = Math.floor(s / 60);
  if (min < 60) return `${min} min`;
  return `${Math.floor(min / 60)} h ${String(min % 60).padStart(2, '0')} min`;
}
export const dreamSince = (iso: string, now = Date.now()) => dreamSpan(now - Date.parse(iso));

/** O que mudou nas notas de um projeto, numa frase curta. */
export function dreamNotesText(notes: DreamProjectProgress['notes']): string {
  if (!notes) return '';
  const parts = [
    notes.new ? m.dream_notes_new({ n: notes.new }) : '',
    notes.changed ? m.dream_notes_changed({ n: notes.changed }) : '',
    notes.removed ? m.dream_notes_removed({ n: notes.removed }) : '',
  ].filter(Boolean);
  return parts.length ? parts.join(', ') : m.dream_notes_none();
}
const isActive = (s: DreamProjectStage) => s === 'reading' || s === 'analyzing' || s === 'organizing' || s === 'writing';
/** Projetos que contam na rodada (o pulado por falta de conversa nova sai do "N de M"). */
const counted = (p: DreamProgress) => p.projects.filter((x) => x.status !== 'skipped');
/** Linha de cima do bloco enquanto sonha: "projeto N de M" e o que o projeto atual está fazendo. */
export function dreamProgressLines(p: DreamProgress): { title: string; detail: string; fraction: number } {
  const list = counted(p);
  const current = p.current === null ? null : p.projects[p.current] ?? null;
  const at = current ? list.indexOf(current) : -1;
  const done = list.filter((x) => !isActive(x.status) && x.status !== 'queued').length;
  const name = current ? (current.id === DREAM_USER_PROJECT ? m.dream_progress_user() : current.name) : '';
  const detail = !current ? ''
    : current.status === 'reading' ? m.dream_progress_reading({ project: name })
    : current.status === 'organizing' ? m.dream_progress_organizing({ project: name })
    : current.status === 'writing' ? m.dream_progress_writing({ project: name })
    : m.dream_progress_analyzing({ project: name, done: Math.min(current.calls_done + 1, Math.max(current.calls_total - 1, 1)), total: Math.max(current.calls_total - 1, 1) });
  // Barra: projetos acabados mais a fração das chamadas do atual.
  const share = current && current.calls_total ? current.calls_done / current.calls_total : 0;
  return {
    title: m.dream_progress_running({ n: at < 0 ? Math.min(done + 1, Math.max(list.length, 1)) : at + 1, total: Math.max(list.length, 1) }),
    detail,
    fraction: list.length ? Math.min(1, (done + (current && isActive(current.status) ? share : 0)) / list.length) : 0,
  };
}
/** Linha de estado de um projeto na lista, enquanto a rodada está no ar; `null` = não está na rodada. */
export function dreamRowState(p: DreamProgress | null | undefined, id: string): { text: string; tone: 'muted' | 'active' | 'error' } | null {
  const row = p?.projects.find((x) => x.id === id);
  if (!row) return null;
  switch (row.status) {
    case 'queued': return { text: m.dream_row_queued(), tone: 'muted' };
    case 'reading': case 'organizing': case 'writing': case 'analyzing': {
      const total = Math.max(row.calls_total - 1, 1);
      return { text: m.dream_row_running({ done: Math.min(row.calls_done + 1, total), total }), tone: 'active' };
    }
    case 'done': return { text: m.dream_row_done({ notes: dreamNotesText(row.notes), cost: dreamUsd(row.cost_usd) }), tone: 'muted' };
    case 'failed': case 'stopped': return row.error ? { text: dreamErrorText(row.error), tone: 'error' } : null;
    default: return null;
  }
}

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
  dream_peer_failed: (detail) => m.dream_error_dream_peer_failed({ detail }),
  dream_stopped: () => m.dream_error_dream_stopped(),
  dream_timeout: () => m.dream_error_dream_timeout(),
  dream_budget_exhausted: () => m.dream_error_dream_budget_exhausted(),
  dream_locked: (detail) => m.dream_error_dream_locked({ detail }),
  dream_interrupted: () => m.dream_error_dream_interrupted(),
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

/** Uma linha por lugar que não voltou: o motivo do servidor quando ele manda, senão a frase antiga. */
export const dreamUndoFailures = (results: DreamUndoResult[], labelOf: (machine: string) => string) =>
  results.filter((r) => !r.ok).map((r) =>
    r.error ? `${labelOf(r.machine)}: ${dreamErrorText(r.error)}` : m.dream_undo_failed({ machine: labelOf(r.machine) }));

/** A primeira leitura que casa, sem esperar as lentas; `null` quando todas terminaram sem casar. Falha conta como não casar. */
export function dreamFirstMatch<T>(reads: Promise<T>[], match: (value: T) => boolean): Promise<T | null> {
  return new Promise((resolve) => {
    let left = reads.length;
    if (!left) resolve(null);
    const miss = () => { if (--left === 0) resolve(null); };
    for (const read of reads) read.then((value) => { if (match(value)) resolve(value); else miss(); }, miss);
  });
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
/** Máquinas e conversas novas do projeto; contagem desconhecida não aparece. */
export const dreamFactsText = (machines: string, newSessions: number | null) =>
  [machines, newSessions === null ? '' : m.dream_new_sessions({ n: newSessions })].filter(Boolean).join(' · ');
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
