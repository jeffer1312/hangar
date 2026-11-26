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

function isUpdate(v: unknown): boolean {
  if (v === undefined || v === null) return true;
  if (!isFields(v) || typeof v.latest !== 'string' || typeof v.migration !== 'boolean' || typeof v.checked_at !== 'string') return false;
  const tagged = (f: unknown) => f === undefined || f === null || (isFields(f) && typeof f.tag === 'string' && typeof f.code === 'string');
  return tagged(v.failed) && tagged(v.last_error);
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
    && optText(v.accepted_at) && optText(v.qr_svg) && optText(v.status_error) && isJob(v.job) && isStatus(v.status)
    && optText(v.installed_tag) && optText(v.update_checked_at) && isUpdate(v.update);
}

export function viewOf(state: WhatsAppState): WhatsAppView {
  if (jobRunning(state)) return 'working';
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

/** Parado porque o binário não conseguiu guardar a cópia do arquivo antes de converter: religar é o caminho,
 * só o texto muda. */
export function stoppedByBackup(state: WhatsAppState): boolean {
  return viewOf(state) === 'stopped' && state.status_error === 'whatsapp_backup_failed';
}

/** Instalar ou religar vale com o aceite gravado no servidor ou marcado agora. */
export function installAccepted(state: WhatsAppState, acceptedNow: boolean): boolean {
  return acceptedNow || !!state.accepted_at;
}

/** Sem aceite gravado (a migração instala sem gravá-lo), o parado pede o aceite antes de religar. */
export function asksAccept(state: WhatsAppState): boolean {
  return !state.accepted_at;
}

/** O código de um erro, venha como texto ou como `{code}`. */
export function codeOf(error: unknown): string | null {
  const code = typeof error === 'string' ? error
    : error && typeof error === 'object' && typeof (error as { code?: unknown }).code === 'string' ? (error as { code: string }).code : '';
  return code || null;
}

/** O job do dono (instalar ou atualizar) que terminou com erro: o tipo dele e o código. A passada automática
 * não entra: a falha dela aparece por `autoUpdateFailure`. */
export function failedJob(state: WhatsAppState): { kind: string; code: string } | null {
  const job = state.job;
  if (!job?.done || (job.kind !== 'install' && job.kind !== 'update')) return null;
  const code = codeOf(job.error);
  return code ? { kind: job.kind, code } : null;
}

export function jobRunning(state: WhatsAppState): boolean {
  return !!state.job && !state.job.done;
}

/** A passada automática também troca o programa de verdade. */
export function isUpdateJob(job: WhatsAppState['job']): boolean {
  return job?.kind === 'update' || job?.kind === 'auto_update';
}

/** A linha da versão no cartão de conectado. Com uma falha registrada, a faixa do topo já oferece a nova
 * versão: a linha só diz quando procurou. `checkedAt` em segundos epoch, `null` sem procura legível. */
export type VersionLine =
  | { kind: 'current' | 'checked'; tag: string; checkedAt: number | null }
  | { kind: 'available' | 'migration'; tag: string; latest: string };

export function versionLine(state: WhatsAppState): VersionLine | null {
  const tag = state.installed_tag;
  if (!tag || viewOf(state) !== 'connected') return null;
  const update = state.update;
  const checkedAt = epochSeconds(state.update_checked_at);
  if (!update) return { kind: 'current', tag, checkedAt };
  if (update.failed) return { kind: 'checked', tag, checkedAt };
  return { kind: update.migration ? 'migration' : 'available', tag, latest: update.latest };
}

function epochSeconds(at: string | null | undefined): number | null {
  const ms = at ? Date.parse(at) : NaN;
  return Number.isNaN(ms) ? null : ms / 1000;
}

/** A atualização registrada que falhou (a automática, ou uma do dono que o servidor não guarda mais em memória).
 * Com a faixa do job do dono na tela, ela já diz o mesmo. Só com o serviço de pé: a faixa diz que a instalada
 * continua rodando, e parado a vista do parado já explica. */
export function autoUpdateFailure(state: WhatsAppState): { tag: string; code: string; installed: string; migration: boolean } | null {
  const update = state.update;
  const failed = update?.failed;
  if (!update || !failed || !state.installed_tag || state.service !== 'running' || jobRunning(state) || failedJob(state)) return null;
  return { tag: failed.tag, code: failed.code, installed: state.installed_tag, migration: update.migration };
}

/** A atualização que esta tela pediu (`own`) terminou? `confirm`: parou porque muda o formato dos dados e a
 * confirmação abre sozinha; `ended`: terminou de outro jeito (ou o servidor não tem mais o job); `null`: segue
 * andando, ou não foi esta tela que pediu, e aí nada abre sozinho. */
export function ownUpdateEnd(state: WhatsAppState, own: boolean): 'confirm' | 'ended' | null {
  const job = state.job;
  if (!own || (job && !job.done)) return null;
  return job?.kind === 'update' && job.outcome === 'needs_confirm' && state.update && state.installed_tag ? 'confirm' : 'ended';
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
