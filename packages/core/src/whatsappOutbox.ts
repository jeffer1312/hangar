// Rascunhos de WhatsApp esperando o dono (evento `whatsapp_outbox` da lista e rotas
// `/api/whatsapp/outbox/{id}/send|send-edited|decline`). O modelo só cria o rascunho; quem envia é a pessoa, pelo cartão.
import { apiFetchForServer } from './api';
import { intlLocale } from './i18n';
import type { Server } from './servers';
import * as m from './paraglide/messages';

export type WhatsAppDraftState = 'pending' | 'sending' | 'sent' | 'declined' | 'expired' | 'failed';
/** `push`: nome que a própria pessoa escolheu (não vem da agenda); vazio quando a origem não é conhecida. */
export type WhatsAppChatNameSource = 'contact' | 'push' | 'group' | '';

/** Tipo de uma mensagem recente; reação nunca chega ao cartão. */
export type WhatsAppRecentKind = 'text' | 'image' | 'video' | 'audio' | 'document' | 'sticker' | 'other';

export interface WhatsAppDraftRecent {
  from_me: boolean;
  sender: string;
  at: string;
  /** Texto ou legenda; de terceiro, como o `file_name`. */
  text: string;
  kind: WhatsAppRecentKind;
  /** Nome do documento (até 120 caracteres); vazio nos outros tipos. */
  file_name: string;
}

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
  /** Foto do contato ou do grupo: base64 de JPEG; `null` sem foto (escondida, inexistente ou ainda não buscada). */
  chat_avatar: string | null;
  recent: WhatsAppDraftRecent[];
  text: string;
  reply_to: string;
  quoted_text: string;
  file: WhatsAppDraftFile | null;
  created_at: string;
  expires_at: string;
  /** Até quando o cartão conta: o menor entre `expires_at` e o prazo da tool que espera a resposta (do Hangar). */
  answer_by: string;
  state: WhatsAppDraftState;
  /** O dono editou o texto antes de enviar. */
  edited: boolean;
  /** O texto aprovado; vazio enquanto pendente (o cliente só o lê na resposta do envio). */
  sent_text: string;
}

/** Evento `whatsapp_outbox` da lista do dono: só os pendentes. `edit`: o serviço aceita texto editado (hangar-server antigo
 *  não manda, e aí o cartão não oferece a edição). */
export interface WhatsAppOutboxEvent { drafts: WhatsAppDraft[]; edit: boolean }

export type WhatsAppDraftAnswer = 'send' | 'decline';
export type WhatsAppDraftVia = 'desktop' | 'mobile';

const str = (v: unknown) => (typeof v === 'string' ? v : '');
const SOURCES: readonly WhatsAppChatNameSource[] = ['contact', 'push', 'group'];
const KINDS: readonly WhatsAppRecentKind[] = ['text', 'image', 'video', 'audio', 'document', 'sticker', 'other'];
// Os bytes FF D8 FF do JPEG em base64; o resto não vira imagem e o cartão mostra a inicial.
const AVATAR = /^\/9j\/[A-Za-z0-9+/]*={0,2}$/;
const avatarOf = (v: unknown) => (typeof v === 'string' && AVATAR.test(v) ? v : null);

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
    if (typeof e.text !== 'string' || e.kind === 'reaction') return [];
    // Rascunho da 0.2.0 não tem `kind`: era texto. Tipo que este cliente não conhece mostra só o texto.
    const kind = e.kind == null ? 'text' : KINDS.find((k) => k === e.kind) ?? 'other';
    return [{ from_me: e.from_me === true, sender: str(e.sender), at: str(e.at), text: e.text, kind, file_name: str(e.file_name) }];
  }) : [];
  const source = SOURCES.find((s) => s === r.chat_name_source) ?? '';
  return {
    id, origin, chat: str(r.chat), chat_kind: r.chat_kind === 'group' ? 'group' : 'contact',
    chat_number, chat_name: str(r.chat_name), chat_name_source: source, chat_avatar: avatarOf(r.chat_avatar), recent, text,
    reply_to: str(r.reply_to), quoted_text: str(r.quoted_text), file, created_at: str(r.created_at), expires_at, answer_by: str(r.answer_by), state: 'pending',
    edited: r.edited === true, sent_text: str(r.sent_text),
  };
}

/** Evento torto devolve `null` (o anterior fica); rascunho fora do formato ou não pendente sai da lista. */
export function parseWhatsAppOutboxEvent(data: string): WhatsAppOutboxEvent | null {
  let raw: unknown;
  try { raw = JSON.parse(data); } catch { return null; }
  if (!raw || typeof raw !== 'object') return null;
  const { drafts, edit } = raw as { drafts?: unknown; edit?: unknown };
  if (!Array.isArray(drafts)) return null;
  return { edit: edit === true, drafts: drafts.flatMap((d) => {
    const draft = draftOf(d);
    // Pendente que não se lê nunca vira cartão: a sessão que pediu fica esperando sem ninguém saber por quê.
    if (!draft && (d as { state?: unknown } | null)?.state === 'pending') console.warn('whatsapp outbox: unreadable pending draft', (d as { id?: unknown }).id);
    return draft ? [draft] : [];
  }) };
}

/** Responde o rascunho na máquina do WhatsApp; `via` só vai no envio e serve ao registro do serviço. */
export function answerWhatsAppDraft(s: Server, id: string, answer: WhatsAppDraftAnswer, via: WhatsAppDraftVia): Promise<unknown> {
  const path = `/api/whatsapp/outbox/${encodeURIComponent(id)}/${answer}`;
  const init: RequestInit = answer === 'send' ? { method: 'POST', body: JSON.stringify({ via }) } : { method: 'POST' };
  return apiFetchForServer(s, path, init, 30_000);
}

/** Quebras de linha como o serviço grava: `\r\n` e `\r` viram `\n`. */
export const normalizeDraftText = (text: string) => text.replace(/\r\n?/g, '\n');

/** O campo mudou o texto do rascunho? Os dois comparados já normalizados. */
export const draftTextChanged = (original: string, field: string) => normalizeDraftText(field) !== normalizeDraftText(original);

/** Teto do texto no serviço, em pontos de código. */
export const DRAFT_TEXT_MAX = 4096;

/** Frase do texto que o serviço recusaria (as regras da criação: até 4096 caracteres, vazio só com anexo); `undefined` quando
 *  pode sair. */
export function draftTextError(text: string, hasFile: boolean): string | undefined {
  const t = normalizeDraftText(text);
  if (draftCharCount(t) > DRAFT_TEXT_MAX) return m.whatsapp_draft_err_text_long();
  if (!t.trim() && !hasFile) return m.whatsapp_draft_err_text_empty();
  return undefined;
}

/** Envia o texto editado, normalizado. Devolve o corpo do 202 (o rascunho), que quem chama confere com `sentTextMatches`. */
export function sendEditedWhatsAppDraft(s: Server, id: string, text: string, via: WhatsAppDraftVia): Promise<unknown> {
  const path = `/api/whatsapp/outbox/${encodeURIComponent(id)}/send-edited`;
  return apiFetchForServer(s, path, { method: 'POST', body: JSON.stringify({ via, text: normalizeDraftText(text) }) }, 30_000);
}

/** O serviço confirmou que saiu o texto editado? `sent_text` ausente ou diferente é não: o cartão nunca diz que saiu o editado. */
export function sentTextMatches(body: unknown, text: string): boolean {
  const sent = body && typeof body === 'object' ? (body as { sent_text?: unknown }).sent_text : undefined;
  // Os dois lados normalizados: com texto igual ao do rascunho, o serviço devolve o texto cru dele, que pode ter `\r\n`.
  return typeof sent === 'string' && normalizeDraftText(sent) === normalizeDraftText(text);
}

/** Endereço da foto do destino para a imagem do cartão. */
export const draftAvatarUri = (avatar: string) => `data:image/jpeg;base64,${avatar}`;

/** Letra do círculo sem foto: a primeira visível do nome, maiúscula; sem nome, o primeiro dígito do número. A mesma regra do
 *  nativo (`initial`). */
export function draftInitial(draft: Pick<WhatsAppDraft, 'chat_name' | 'chat_number'>): string {
  const first = Array.from(draft.chat_name).find((ch) => !/\s/u.test(ch) && !isInvisible(ch.codePointAt(0)!));
  return first ? first.toLocaleUpperCase(intlLocale()) : draft.chat_number.match(/\d/)?.[0] ?? '';
}

const KIND_WORD: Partial<Record<WhatsAppRecentKind, () => string>> = {
  image: m.whatsapp_draft_kind_image, video: m.whatsapp_draft_kind_video, audio: m.whatsapp_draft_kind_audio,
  sticker: m.whatsapp_draft_kind_sticker, document: m.whatsapp_draft_kind_document,
};

/** Balão de uma mensagem recente: a etiqueta do tipo em maiúsculas (`null` em texto e em `other`) e o texto, que é a
 *  legenda, o nome do documento quando ela vem vazia ou, sem nenhum dos dois, a palavra do tipo. */
export function recentBubble(recent: Pick<WhatsAppDraftRecent, 'kind' | 'text' | 'file_name'>): { tag: string | null; text: string } {
  const word = KIND_WORD[recent.kind]?.();
  if (!word) return { tag: null, text: recent.text };
  const file = recent.kind === 'document' ? recent.file_name : '';
  return { tag: word.toLocaleUpperCase(intlLocale()), text: recent.text.trim() ? recent.text : file.trim() ? file : word };
}

/** Trecho do texto: o que aparece como veio, ou a marca de um caractere invisível ou de controle (`U+200B`). */
export type InvisiblePart = { kind: 'text'; text: string } | { kind: 'mark'; code: string };

// A mesma lista do nativo (`whatsapp_outbox.rs`), conferida pelos casos de `whatsappInvisible.fixtures.json`: o que não aparece
// na tela ou mexe na ordem do texto.
const INVISIBLE: ReadonlyArray<readonly [number, number]> = [
  [0x0000, 0x0008], [0x000b, 0x001f], [0x007f, 0x009f], [0x00ad, 0x00ad], [0x034f, 0x034f], [0x061c, 0x061c],
  [0x115f, 0x1160], [0x17b4, 0x17b5], [0x180b, 0x180f], [0x200b, 0x200f], [0x2028, 0x202e], [0x2060, 0x206f],
  [0x2800, 0x2800], [0x3164, 0x3164], [0xfe00, 0xfe0f], [0xfeff, 0xfeff], [0xffa0, 0xffa0], [0xfff0, 0xfffb],
  [0x1bca0, 0x1bca3], [0x1d173, 0x1d17a], [0xe0000, 0xe0fff],
];
// Seletores de emoji fazem parte de emoji (❤️, ©️): depois de símbolo, não são marcados.
const EMOJI_SELECTOR = new Set([0xfe0e, 0xfe0f]);
const ZWJ = 0x200d;
// O ZWJ só cola emoji (👨‍👩‍👧, ❤️‍🔥); entre letras ele não aparece e muda o texto. A mesma regra do nativo.
const EMOJI: ReadonlyArray<readonly [number, number]> = [
  [0x1f000, 0x1faff], [0x2600, 0x27bf], [0x2300, 0x23ff], [0x2b00, 0x2bff], [0xfe0f, 0xfe0f],
];
const isEmoji = (cp: number | undefined) => cp !== undefined && EMOJI.some(([a, b]) => cp >= a && cp <= b);

const isInvisible = (cp: number) => INVISIBLE.some(([a, b]) => cp >= a && cp <= b);

/** Código de um ponto, como `U+200B` (no mínimo quatro dígitos). */
export const codePointLabel = (cp: number) => `U+${cp.toString(16).toUpperCase().padStart(4, '0')}`;

/** A marca que a tela mostra no lugar do caractere. */
export const invisibleMark = (code: string) => `⟨${code}⟩`;

/** Quebra o texto em trechos visíveis e marcas; quebra de linha e tabulação ficam como estão. */
export function markInvisibles(text: string): InvisiblePart[] {
  const parts: InvisiblePart[] = [];
  let plain = '';
  const chars = Array.from(text);
  let prevMarked = false;
  chars.forEach((ch, i) => {
    const cp = ch.codePointAt(0)!;
    const prev = chars[i - 1]?.codePointAt(0);
    const glue = prev !== undefined && !prevMarked && (cp === ZWJ
      ? isEmoji(prev) && isEmoji(chars[i + 1]?.codePointAt(0))
      : EMOJI_SELECTOR.has(cp) && (prev >= 0x2000 || prev === 0xa9 || prev === 0xae));
    const mark = isInvisible(cp) && !glue;
    if (mark) {
      if (plain) { parts.push({ kind: 'text', text: plain }); plain = ''; }
      parts.push({ kind: 'mark', code: codePointLabel(cp) });
    } else plain += ch;
    prevMarked = mark;
  });
  if (plain) parts.push({ kind: 'text', text: plain });
  return parts;
}

/** Caracteres como o serviço conta (pontos de código, não unidades UTF-16). */
export const draftCharCount = (text: string) => Array.from(text).length;

/** Segundos até o rascunho deixar de aceitar resposta: até `answer_by`; sem ele legível, até `expires_at`. Nunca negativo;
 *  nenhum dos dois legível conta como expirado. */
export function draftSecondsLeft(draft: Pick<WhatsAppDraft, 'answer_by' | 'expires_at'>, now = Date.now()): number {
  const answerBy = Date.parse(draft.answer_by);
  const end = Number.isFinite(answerBy) ? answerBy : Date.parse(draft.expires_at);
  return Number.isFinite(end) ? Math.max(0, Math.floor((end - now) / 1000)) : 0;
}

/** Hora local de uma mensagem (`14:02`); vazio quando a data não se lê. */
export function draftClock(at: string): string {
  const t = Date.parse(at);
  return Number.isFinite(t) ? new Date(t).toLocaleTimeString(intlLocale(), { hour: '2-digit', minute: '2-digit' }) : '';
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
  // Aprovado depois de parear outro número: o serviço confere o contato de novo.
  whatsapp_new_contact: m.whatsapp_draft_err_new_contact,
  // Serviço sem envio (api 1): o remédio é o mesmo.
  whatsapp_send_unsupported: m.whatsapp_draft_err_outdated,
  // Serviço antes da edição (api < 3): o Hangar recusa sem chamar, e o cartão fica.
  whatsapp_edit_unsupported: m.whatsapp_draft_err_edit_unsupported,
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
