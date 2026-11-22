import type { WhatsAppState } from '@hangar/core';

/** O que a página mostra, decidido só pela resposta do `GET /api/whatsapp` (mesma regra do nativo). */
export type WhatsAppView = 'off' | 'working' | 'stopped' | 'unreachable' | 'unpaired' | 'pairing' | 'connected' | 'outdated' | 'unknown';

/** As etapas do job, na ordem do `step` do servidor. */
export const STEPS = ['download', 'service', 'start'] as const;

/** Logo depois de pedir o código o estado ainda pode vir `unpaired`: a releitura continua por este tempo. */
export const PAIR_GRACE_MS = 10_000;
export const POLL_MS = 1_000;

/** Resposta fora do formato vira erro de leitura, não uma tela inventada. */
export function isWhatsAppState(v: unknown): v is WhatsAppState {
  if (!v || typeof v !== 'object') return false;
  const s = v as Partial<WhatsAppState>;
  return typeof s.installed === 'boolean' && (s.service === 'running' || s.service === 'stopped' || s.service === 'absent');
}

export function viewOf(state: WhatsAppState): WhatsAppView {
  if (state.job && !state.job.done) return 'working';
  if (!state.installed) return 'off';
  if (state.service !== 'running') return 'stopped';
  if (!state.status) return 'unreachable';
  switch (state.status.state) {
    case 'connected': return 'connected';
    case 'outdated': return 'outdated';
    case 'pairing': return 'pairing';
    case 'unpaired': case 'logged_out': return 'unpaired';
    default: return 'unknown';
  }
}

/** O código de um erro, venha como texto ou como `{code}`. */
export function codeOf(error: unknown): string | null {
  const code = typeof error === 'string' ? error
    : error && typeof error === 'object' && typeof (error as { code?: unknown }).code === 'string' ? (error as { code: string }).code : '';
  return code || null;
}

/** O job que terminou com erro: o tipo dele e o código. */
export function failedJob(state: WhatsAppState): { kind: string; code: string } | null {
  const job = state.job;
  if (!job?.done) return null;
  const code = codeOf(job.error);
  return code ? { kind: job.kind, code } : null;
}

/** A frase de um código, com ou sem o prefixo `whatsapp_`; sem frase própria, a genérica leva o código cru. */
export function phrase(table: Record<string, () => string>, code: string, unknown: (p: { code: string }) => string): string {
  const key = code.startsWith('whatsapp_') ? code.slice('whatsapp_'.length) : code;
  return Object.prototype.hasOwnProperty.call(table, key) ? table[key]() : unknown({ code });
}

export function shouldPoll(state: WhatsAppState, awaitingPair: boolean): boolean {
  const view = viewOf(state);
  return view === 'working' || view === 'pairing' || (awaitingPair && view === 'unpaired');
}

/** Índice da etapa em curso; etapa desconhecida conta como a primeira. */
export function stepIndex(step: string | undefined): number {
  const at = STEPS.indexOf(step as typeof STEPS[number]);
  return at < 0 ? 0 : at;
}

/** Só algarismos, de 10 a 15 (código do país, DDD e número); `+`, espaços, traços, parênteses e pontos somem. */
export function phoneDigits(text: string): string | null {
  if (/[^0-9+\s\-().]/.test(text)) return null;
  const digits = text.replace(/\D/g, '');
  return digits.length >= 10 && digits.length <= 15 ? digits : null;
}

/** Minutos que faltam até um ISO UTC futuro, arredondados para cima; passado ou ilegível = `null`. */
export function minutesLeft(at: string | null | undefined, now = Date.now()): number | null {
  if (!at) return null;
  const ms = Date.parse(at) - now;
  return Number.isNaN(ms) || ms <= 0 ? null : Math.ceil(ms / 60_000);
}
