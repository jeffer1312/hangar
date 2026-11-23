import type { WhatsAppState } from '@hangar/core';

/** O que a página mostra, decidido só pela resposta do `GET /api/whatsapp` (mesma regra do nativo). */
export type WhatsAppView = 'off' | 'working' | 'stopped' | 'schema_newer' | 'schema_newer_latest' | 'unreachable' | 'unpaired' | 'pairing' | 'connected' | 'outdated' | 'unknown';

/** As etapas do job, na ordem do `step` do servidor. */
export const STEPS = ['download', 'service', 'start'] as const;

/** Logo depois de pedir o código o estado ainda pode vir `unpaired`: a releitura continua por este tempo. */
export const PAIR_GRACE_MS = 10_000;
export const POLL_MS = 1_000;

type Fields = Record<string, unknown>;
const isFields = (v: unknown): v is Fields => !!v && typeof v === 'object' && !Array.isArray(v);
/** Ausente ou `null` também vale: os campos opcionais que a tela lê. */
const optText = (v: unknown) => v === undefined || v === null || typeof v === 'string';
const optCode = (v: unknown) => optText(v) || (isFields(v) && typeof v.code === 'string');

function isJob(v: unknown): boolean {
  if (v === undefined || v === null) return true;
  return isFields(v) && typeof v.done === 'boolean' && typeof v.kind === 'string' && optText(v.step) && optCode(v.error);
}

function isStatus(v: unknown): boolean {
  if (v === undefined || v === null) return true;
  if (!isFields(v) || typeof v.state !== 'string' || !optCode(v.error)) return false;
  const texts = ['device', 'device_name', 'pair_code', 'pair_code_expires_at', 'qr_expires_at', 'since', 'last_message_at'];
  if (!texts.every((key) => optText(v[key]))) return false;
  const totals = v.totals;
  return totals === undefined || totals === null
    || (isFields(totals) && Object.values(totals).every((n) => n === undefined || n === null || typeof n === 'number'));
}

/** Resposta fora do formato vira erro de leitura, não uma tela inventada nem exceção no desenho. */
export function isWhatsAppState(v: unknown): v is WhatsAppState {
  if (!isFields(v)) return false;
  return typeof v.installed === 'boolean' && (v.service === 'running' || v.service === 'stopped' || v.service === 'absent')
    && optText(v.accepted_at) && optText(v.qr_svg) && optText(v.status_error) && isJob(v.job) && isStatus(v.status);
}

export function viewOf(state: WhatsAppState): WhatsAppView {
  if (state.job && !state.job.done) return 'working';
  if (!state.installed) return 'off';
  // Parado porque os dados são de um binário mais novo: religar não adianta, só atualizar; e atualizar que
  // não achou outro também não.
  if (state.service !== 'running') {
    if (state.status_error !== 'whatsapp_schema_newer') return 'stopped';
    return alreadyLatest(state) ? 'schema_newer_latest' : 'schema_newer';
  }
  if (!state.status) return 'unreachable';
  switch (state.status.state) {
    case 'connected': return 'connected';
    case 'outdated': return 'outdated';
    case 'pairing': return 'pairing';
    case 'unpaired': case 'logged_out': return 'unpaired';
    default: return 'unknown';
  }
}

/** A última atualização terminou sem achar release mais nova que a instalada. */
function alreadyLatest(state: WhatsAppState): boolean {
  const job = state.job;
  return !!job?.done && job.kind === 'update' && job.outcome === 'already_latest';
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

/** Leituras seguidas que falham, com a página já mostrando dados, até ela trocar pelo erro. */
export const READ_FAILS_TO_ERROR = 3;

/** Falha de leitura vira a tela de erro na carga inicial ou na terceira seguida; antes disso fica o dado anterior. */
export function readErrorShows(hasData: boolean, failsInARow: number): boolean {
  return !hasData || failsInARow >= READ_FAILS_TO_ERROR;
}

/** `retrying`: a última leitura falhou e o dado na tela pode estar velho, então relê em qualquer estado. */
export function shouldPoll(state: WhatsAppState, awaitingPair: boolean, retrying = false): boolean {
  const view = viewOf(state);
  return retrying || view === 'working' || view === 'pairing' || (awaitingPair && view === 'unpaired');
}

/** Índice da etapa em curso; etapa desconhecida conta como a primeira. */
export function stepIndex(step: string | null | undefined): number {
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
