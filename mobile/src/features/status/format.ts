import { intlLocale, type StatusAttention, type StatusExit, type StatusUpdateBlock } from '@hangar/core';
import * as m from '../../paraglide/messages';

const MIB = 1024 * 1024;
const GIB = MIB * 1024;

export const num = (value: number, digits = 0) => value.toLocaleString(intlLocale(), { maximumFractionDigits: digits, minimumFractionDigits: digits });

/** Memória e disco: GB com uma casa a partir de 1 GB, MB inteiro abaixo. */
export function bytes(n: number | undefined): string {
  if (n === undefined || !Number.isFinite(n)) return '—';
  return n >= GIB ? `${num(n / GIB, 1)} GB` : `${num(n / MIB)} MB`;
}

export const pct = (value: number | undefined) => value === undefined ? '—' : `${num(value, 1)} %`;

/** Tempo decorrido em minutos, horas ou dias com horas ("3 d 4 h"). */
export function duration(ms: number): string {
  const minutes = Math.max(0, Math.floor(ms / 60_000));
  if (minutes < 60) return m.status_duration_minutes({ n: String(minutes) });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return m.status_duration_hours({ n: String(hours) });
  return m.status_duration_days({ d: String(Math.floor(hours / 24)), h: String(hours % 24) });
}

export const since = (iso: string | undefined, now = Date.now()) => {
  const t = iso ? Date.parse(iso) : NaN;
  return Number.isFinite(t) ? duration(now - t) : '—';
};

/** Texto de um ponto de atenção; o código vem do servidor e só os conhecidos viram frase. */
export function attentionText(a: StatusAttention, now = Date.now()): string {
  const [a0 = '', a1 = '', a2 = ''] = a.args;
  switch (a.code) {
    case 'machine_down': return m.status_attention_machine_down({ name: a0, since: since(a1, now) });
    case 'exits_crash': return m.status_attention_exits_crash({ count: a0 });
    case 'machine_outdated': return m.status_attention_machine_outdated({ name: a0, version: a1 });
    case 'app_version_differs': return m.status_attention_app_version_differs({ host: a0, version: a1, server: a2 });
    case 'disk_full': return m.status_attention_disk_full({ pct: a0 });
    case 'tmp_full': return m.status_attention_tmp_full({ pct: a0 });
    case 'session_dead': return m.status_attention_session_dead({ name: a0 });
    case 'queue_stuck': return m.status_attention_queue_stuck({ minutes: a0 });
    case 'server_errors': return m.status_attention_server_errors({ count: a0 });
    case 'watchdog': return m.status_attention_watchdog({ count: a0 });
    case 'keyring_locked': return m.status_attention_keyring_locked();
    default: return [a.code, ...a.args].join(' · ');
  }
}

export function exitKindText(kind: StatusExit['kind']): string {
  return kind === 'crash' ? m.status_exit_crash() : kind === 'update' ? m.status_exit_update() : m.status_exit_requested();
}

/** Linha de baixo de uma saída: causa da queda ou as versões da atualização. */
export function exitDetail(e: StatusExit): string {
  if (e.from || e.to) return m.status_exit_versions({ from: e.from ?? '—', to: e.to ?? '—' });
  if (!e.cause) return '';
  switch (e.cause) {
    case 'out_of_memory': return m.status_exit_cause_out_of_memory();
    case 'exit_code': return m.status_exit_cause_exit_code();
    case 'signal': return m.status_exit_cause_signal();
    case 'core_dump': return m.status_exit_cause_core_dump();
    case 'watchdog': return m.status_exit_cause_watchdog();
    case 'timeout': return m.status_exit_cause_timeout();
    default: return m.status_exit_cause_other({ code: e.cause });
  }
}

export const when = (iso: string) => new Date(iso).toLocaleString(intlLocale(), { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });

export function updateLines(u: StatusUpdateBlock, now = Date.now()): { title: string; detail: string } {
  const title = u.available ? m.status_update_available({ version: u.available }) : m.status_update_latest();
  const channel = u.channel === 'stable' ? m.status_update_channel_stable() : u.channel === 'beta' ? m.status_update_channel_beta() : m.status_update_channel_other({ name: u.channel });
  const parts = [channel];
  if (u.last_check) parts.push(m.status_update_checked({ time: since(u.last_check, now) }));
  if (u.last_result) parts.push(u.last_result.ok ? m.status_update_last_ok() : u.last_result.step ? m.status_update_last_failed_step({ step: u.last_result.step }) : m.status_update_last_failed());
  return { title, detail: parts.join(' · ') };
}

export function processName(key: string): string {
  switch (key) {
    case 'claude': return m.status_process_claude();
    case 'codex_app_server': return m.status_process_codex_app_server();
    // O agy entra no mesmo grupo de processo do Pi, do omp e do Kimi.
    case 'pi_omp_kimi': return m.mobile_status_process_pi_omp_kimi_agy();
    case 'cano': return m.status_process_cano();
    case 'tmux': return m.status_process_tmux();
    default: return key;
  }
}

export function platformName(platform: string): string {
  switch (platform) {
    case 'linux': return 'Linux';
    case 'windows': return 'Windows';
    case 'macos': return 'macOS';
    default: return platform;
  }
}
