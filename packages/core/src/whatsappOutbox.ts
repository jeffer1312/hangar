// Rascunhos de WhatsApp esperando o dono (evento `whatsapp_outbox` da lista e rotas
// `/api/whatsapp/outbox/{id}/send|decline`). O modelo só cria o rascunho; quem envia é a pessoa, pelo cartão.
import { apiFetchForServer } from './api';
import { intlLocale } from './i18n';
import type { Server } from './servers';
import * as m from './paraglide/messages';

export type WhatsAppDraftState = 'pending' | 'sending' | 'sent' | 'declined' | 'expired' | 'failed';
/** `push`: nome que a própria pessoa escolheu (não vem da agenda); vazio quando a origem não é conhecida. */
export type WhatsAppChatNameSource = 'contact' | 'push' | 'group' | '';

export interface WhatsAppDraftRecent { from_me: boolean; sender: string; at: string; text: string }

export interface WhatsAppDraftFile {
  name: string;
  size: number;
  sha256: string;
  /** Caminho com as pastas resolvidas. */
  source: string;
  outside_cwd: boolean;
}

/** Rascunho do C5 do hangar-whatsapp. Horários em UTC, `2006-01-02T15:04:05Z`. */
export interface WhatsAppDraft {
  id: string;
  /** `servidor::sessao` de quem pediu. */
  origin: string;
  chat: string;
  chat_kind: 'contact' | 'group';
  /** Dígitos do contato; para grupo, o JID inteiro. */
  chat_number: string;
  chat_name: string;
  chat_name_source: WhatsAppChatNameSource;
  recent: WhatsAppDraftRecent[];
  text: string;
  reply_to: string;
  quoted_text: string;
  file: WhatsAppDraftFile | null;
  created_at: string;
  expires_at: string;
  state: WhatsAppDraftState;
}

/** Evento `whatsapp_outbox` da lista do dono: só os pendentes. */
export interface WhatsAppOutboxEvent { drafts: WhatsAppDraft[] }

export type WhatsAppDraftAnswer = 'send' | 'decline';
export type WhatsAppDraftVia = 'desktop' | 'mobile';

const str = (v: unknown) => (typeof v === 'string' ? v : '');
const SOURCES: readonly WhatsAppChatNameSource[] = ['contact', 'push', 'group'];

function draftOf(raw: unknown): WhatsAppDraft | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as Record<string, unknown>;
  const { id, origin, chat_number, text, expires_at } = r;
  // Só pendente vira cartão: o resto não aceita mais resposta.
  if (typeof id !== 'string' || !id || r.state !== 'pending') return null;
  if (typeof origin !== 'string' || typeof chat_number !== 'string' || !chat_number) return null;
  if (typeof text !== 'string' || typeof expires_at !== 'string') return null;
  const f = r.file as Record<string, unknown> | null | undefined;
  const file = f && typeof f === 'object' && typeof f.name === 'string' && typeof f.source === 'string'
    ? { name: f.name, size: typeof f.size === 'number' ? f.size : 0, sha256: str(f.sha256), source: f.source, outside_cwd: f.outside_cwd !== false }
    : null;
  const recent = Array.isArray(r.recent) ? r.recent.flatMap((x) => {
    if (!x || typeof x !== 'object') return [];
    const e = x as Record<string, unknown>;
    return typeof e.text === 'string' ? [{ from_me: e.from_me === true, sender: str(e.sender), at: str(e.at), text: e.text }] : [];
  }) : [];
  const source = SOURCES.find((s) => s === r.chat_name_source) ?? '';
  return {
    id, origin, chat: str(r.chat), chat_kind: r.chat_kind === 'group' ? 'group' : 'contact',
    chat_number, chat_name: str(r.chat_name), chat_name_source: source, recent, text,
    reply_to: str(r.reply_to), quoted_text: str(r.quoted_text), file, created_at: str(r.created_at), expires_at, state: 'pending',
  };
}

/** Evento torto devolve `null` (o anterior fica); rascunho fora do formato ou não pendente sai da lista. */
export function parseWhatsAppOutboxEvent(data: string): WhatsAppOutboxEvent | null {
  let raw: unknown;
  try { raw = JSON.parse(data); } catch { return null; }
  if (!raw || typeof raw !== 'object') return null;
  const drafts = (raw as { drafts?: unknown }).drafts;
  if (!Array.isArray(drafts)) return null;
  return { drafts: drafts.flatMap((d) => { const draft = draftOf(d); return draft ? [draft] : []; }) };
}

/** Responde o rascunho na máquina do WhatsApp; `via` só vai no envio e serve ao registro do serviço. */
export function answerWhatsAppDraft(s: Server, id: string, answer: WhatsAppDraftAnswer, via: WhatsAppDraftVia): Promise<unknown> {
  const path = `/api/whatsapp/outbox/${encodeURIComponent(id)}/${answer}`;
  const init: RequestInit = answer === 'send' ? { method: 'POST', body: JSON.stringify({ via }) } : { method: 'POST' };
  return apiFetchForServer(s, path, init, 30_000);
}

/** Trecho do texto: o que aparece como veio, ou a marca de um caractere invisível ou de controle (`U+200B`). */
export type InvisiblePart = { kind: 'text'; text: string } | { kind: 'mark'; code: string };

// A mesma lista do nativo (`whatsapp_outbox.rs`): o que não aparece na tela ou mexe na ordem do texto.
const INVISIBLE: ReadonlyArray<readonly [number, number]> = [
  [0x0000, 0x0008], [0x000b, 0x001f], [0x007f, 0x009f], [0x00ad, 0x00ad], [0x034f, 0x034f], [0x061c, 0x061c],
  [0x115f, 0x1160], [0x17b4, 0x17b5], [0x180b, 0x180f], [0x200b, 0x200f], [0x2028, 0x202e], [0x2060, 0x206f],
  [0x2800, 0x2800], [0x3164, 0x3164], [0xfe00, 0xfe0f], [0xfeff, 0xfeff], [0xffa0, 0xffa0], [0xfff0, 0xfffb],
  [0x1bca0, 0x1bca3], [0x1d173, 0x1d17a], [0xe0000, 0xe0fff],
];
// ZWJ e seletores de emoji fazem parte de emoji (👨‍👩‍👧, ❤️): depois de símbolo, não são marcados.
const EMOJI_GLUE = new Set([0x200d, 0xfe0e, 0xfe0f]);

const isInvisible = (cp: number) => INVISIBLE.some(([a, b]) => cp >= a && cp <= b);

/** Código de um ponto, como `U+200B` (no mínimo quatro dígitos). */
export const codePointLabel = (cp: number) => `U+${cp.toString(16).toUpperCase().padStart(4, '0')}`;

/** A marca que a tela mostra no lugar do caractere. */
export const invisibleMark = (code: string) => `⟨${code}⟩`;

/** Quebra o texto em trechos visíveis e marcas; quebra de linha e tabulação ficam como estão. */
export function markInvisibles(text: string): InvisiblePart[] {
  const parts: InvisiblePart[] = [];
  let plain = '';
  let prev: number | null = null;
  let prevMarked = false;
  for (const ch of text) {
    const cp = ch.codePointAt(0)!;
    const glue = EMOJI_GLUE.has(cp) && prev !== null && !prevMarked && (prev >= 0x2000 || prev === 0xa9 || prev === 0xae);
    const mark = isInvisible(cp) && !glue;
    if (mark) {
      if (plain) { parts.push({ kind: 'text', text: plain }); plain = ''; }
      parts.push({ kind: 'mark', code: codePointLabel(cp) });
    } else plain += ch;
    prev = cp;
    prevMarked = mark;
  }
  if (plain) parts.push({ kind: 'text', text: plain });
  return parts;
}

/** Caracteres como o serviço conta (pontos de código, não unidades UTF-16). */
export const draftCharCount = (text: string) => Array.from(text).length;

/** Segundos até expirar, nunca negativo; data ilegível conta como expirada. */
export function draftSecondsLeft(expiresAt: string, now = Date.now()): number {
  const at = Date.parse(expiresAt);
  return Number.isFinite(at) ? Math.max(0, Math.floor((at - now) / 1000)) : 0;
}

/** Hora local de uma mensagem (`14:02`); vazio quando a data não se lê. */
export function draftClock(at: string): string {
  const t = Date.parse(at);
  return Number.isFinite(t) ? new Date(t).toLocaleTimeString(intlLocale(), { hour: '2-digit', minute: '2-digit' }) : '';
}

/** `9:12`. */
export const formatCountdown = (seconds: number) => `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;

/** Tamanho do anexo: bytes, KB inteiro, MB e GB com uma casa (unidades de 1024, como o nativo). */
export function draftFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  const fmt = (v: number) => v.toLocaleString(intlLocale(), { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  return bytes < 1024 ** 3 ? `${fmt(bytes / 1024 ** 2)} MB` : `${fmt(bytes / 1024 ** 3)} GB`;
}

/** Recusa que tira o cartão: o rascunho não aceita mais resposta. */
export function draftRefusalEndsCard(code: string | undefined): boolean {
  return code === 'whatsapp_expired' || code === 'whatsapp_answered' || code === 'whatsapp_not_found';
}

const REFUSAL: Record<string, () => string> = {
  whatsapp_expired: m.whatsapp_draft_err_expired,
  whatsapp_answered: m.whatsapp_draft_err_answered,
  whatsapp_not_found: m.whatsapp_draft_err_not_found,
  whatsapp_rate_limited: m.whatsapp_draft_err_rate_limited,
  whatsapp_not_paired: m.whatsapp_draft_err_not_paired,
  whatsapp_outdated: m.whatsapp_draft_err_outdated,
  // Serviço sem envio (api 1): o remédio é o mesmo.
  whatsapp_send_unsupported: m.whatsapp_draft_err_outdated,
};

/** Frase da recusa de `send`/`decline` pelo código `whatsapp_*`. `lockedUntil`: o `reachout_locked_until` do
 *  status, quando lido. `undefined` para código sem frase própria (quem chama mostra a mensagem do servidor). */
export function whatsappDraftRefusalText(code: string | undefined, lockedUntil?: string | null): string | undefined {
  if (!code) return undefined;
  if (code === 'whatsapp_reachout_locked') {
    const at = lockedUntil ? new Date(lockedUntil) : null;
    if (!at || !Number.isFinite(at.getTime())) return m.whatsapp_draft_err_reachout_locked_unknown();
    const locale = intlLocale();
    return m.whatsapp_draft_err_reachout_locked({
      date: at.toLocaleDateString(locale, { day: '2-digit', month: '2-digit' }),
      time: at.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit' }),
    });
  }
  return Object.prototype.hasOwnProperty.call(REFUSAL, code) ? REFUSAL[code]() : undefined;
}
