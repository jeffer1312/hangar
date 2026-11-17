// Retrato da tela Status: espelho de `crates/hangar-api/src/status.rs`. Campo com `?` é bloco ou
// valor sem fonte no servidor (a tela esconde ou mostra "—", nunca um zero inventado).
import { apiFetchForServer } from './api';
import { apiEnv, type EventSourceLike } from './apiEnv';
import { novoReq } from './diag';
import { baseOf, UNCONFIRMED_BASE } from './rota';
import type { Server } from './servers';
import * as m from './paraglide/messages';

export interface StatusPoint { ts: string; v: number }

export interface StatusServerBlock {
  version: string;
  commit?: string;
  pid: number;
  started_at: string;
  rss_bytes: number;
  /** Só existe a partir da segunda amostra. */
  cpu_pct?: number;
  rss_series: StatusPoint[];
  cpu_series: StatusPoint[];
  rss_avg_bytes?: number;
  cpu_avg_pct?: number;
}

export type StatusExitKind = 'crash' | 'update' | 'requested';

export interface StatusExit {
  ts: string;
  kind: StatusExitKind;
  /** Código; o texto sai de `status_exit_cause_<cause>`. */
  cause?: string;
  from?: string;
  to?: string;
  error_lines: string[];
}

export interface StatusExitsBlock { items: StatusExit[] }

export interface StatusUpdateResult {
  ts: string;
  ok: boolean;
  step?: string;
}

export interface StatusUpdateBlock {
  channel: string;
  installed: string;
  available?: string;
  last_check?: string;
  last_result?: StatusUpdateResult;
}

export type StatusMachineState = 'ok' | 'down' | 'unknown';

export interface StatusAppReport {
  version: string;
  rss_bytes: number;
  cpu_pct: number;
  platform: string;
  host: string;
}

export interface StatusMachine {
  id: string;
  label: string;
  this_machine: boolean;
  state: StatusMachineState;
  down_since?: string;
  latency_ms?: number;
  server_version?: string;
  outdated: boolean;
  lan: boolean;
  app?: StatusAppReport;
}

export interface StatusUsage {
  key: string;
  count: number;
  rss_bytes?: number;
  cpu_pct?: number;
}

export interface StatusDeadSession { name: string; cause: string }

export interface StatusSessionsBlock {
  open: number;
  by_provider: StatusUsage[];
  by_process: StatusUsage[];
  dead: StatusDeadSession[];
  asleep: number;
  queued: number;
  oldest_queued_secs?: number;
}

export interface StatusHourBucket { hour_start: string; slow: number; errors: number }

export interface StatusRouteStat {
  route: string;
  count: number;
  worst_ms?: number;
  last_code: number;
}

export type StatusWatchdogKind = 'late' | 'withheld';

export interface StatusWatchdogEvent {
  ts: string;
  kind: StatusWatchdogKind;
  ms?: number;
}

export interface StatusSlowBlock {
  hours: StatusHourBucket[];
  top_routes: StatusRouteStat[];
  watchdog: StatusWatchdogEvent[];
}

export interface StatusDiskUse { used: number; total: number }

/** `bytes` é o último valor conhecido; `computing` diz que há uma conta em andamento. */
export interface StatusDirSize { bytes?: number; computing: boolean }

export interface StatusHostBlock {
  mem_total: number;
  mem_available: number;
  swap_total: number;
  swap_used: number;
  /** Ausente no Windows. */
  load?: [number, number, number];
  cpus: number;
  disk_home: StatusDiskUse;
  disk_tmp?: StatusDiskUse;
  dir_hangar: StatusDirSize;
  dir_build_cache: StatusDirSize;
}

export interface StatusConnectState { enabled: boolean; running: boolean }

export interface StatusOutsideBlock {
  connect?: StatusConnectState;
  funnel?: boolean;
  keyring_locked?: boolean;
}

export interface StatusLeftoversBlock {
  tmux_orphans?: string[];
  cano_orphans?: number;
}

export type StatusSeverity = 'bad' | 'warn';
export type StatusAttentionTarget = 'server' | 'exits' | 'machines' | 'sessions' | 'slow' | 'host' | 'outside';

/** O texto sai de `status_attention_<code>` com `args`, no idioma do aparelho. */
export interface StatusAttention {
  severity: StatusSeverity;
  code: string;
  target: StatusAttentionTarget;
  args: string[];
}

export interface StatusSnapshot {
  generated_at: string;
  server?: StatusServerBlock;
  exits?: StatusExitsBlock;
  update?: StatusUpdateBlock;
  machines?: StatusMachine[];
  sessions?: StatusSessionsBlock;
  slow?: StatusSlowBlock;
  host?: StatusHostBlock;
  outside?: StatusOutsideBlock;
  leftovers?: StatusLeftoversBlock;
  attention: StatusAttention[];
}

/** Nome do evento do SSE que carrega cada retrato. */
export const STATUS_EVENT = 'status';

export function getStatus(s: Server): Promise<StatusSnapshot> {
  return apiFetchForServer<StatusSnapshot>(s, '/api/status');
}

/** Dado de um evento `status`; `null` quando não é um retrato (JSON torto ou sem os campos da raiz). */
export function parseStatusEvent(data: string): StatusSnapshot | null {
  let raw: unknown;
  try { raw = JSON.parse(data); } catch { return null; }
  if (!raw || typeof raw !== 'object') return null;
  const snap = raw as Partial<StatusSnapshot>;
  if (typeof snap.generated_at !== 'string' || !Array.isArray(snap.attention)) return null;
  return snap as StatusSnapshot;
}

// Mesma convenção de `openSessionsStream`: ?token só fora da origem (EventSource não manda
// cabeçalho e cross-origin não leva cookie), cookie na mesma origem.
export function openStatusStream(s: Server, req = novoReq()): EventSourceLike {
  const o = apiEnv().origin;
  const base = baseOf(s);
  if (base === UNCONFIRMED_BASE) {
    throw Object.assign(new Error(m.mobile_machines_identity_unconfirmed()), { status: 409, code: 'server_identity_unconfirmed' });
  }
  const isSameOrigin = !!o && (!base || base === o);
  const params = new URLSearchParams();
  if (!isSameOrigin) params.set('token', s.token);
  if (req) params.set('diag_req', req);
  const qs = params.toString();
  return apiEnv().createEventSource(`${base}/api/status/events${qs ? `?${qs}` : ''}`, { withCredentials: isSameOrigin });
}
