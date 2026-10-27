import { z } from 'zod';
import { prefs } from './prefs';
import * as m from '../paraglide/messages';

const attachmentSchema = z.object({
  uri: z.string().min(1),
  name: z.string().min(1),
  mime: z.string().min(1),
  kind: z.enum(['image', 'file']),
  uploadedPath: z.string().min(1).optional(),
  uploadedFor: z.object({
    serverId: z.string().min(1),
    name: z.string().min(1),
    transcript: z.string().min(1).nullable(),
  }).optional(),
});

// Rascunho gravado antes da lista tinha `attachment` (um só ou null): vira a lista na leitura.
const legacyAttachment = (raw: unknown) => {
  if (raw === null || typeof raw !== 'object' || 'attachments' in raw || !('attachment' in raw)) return raw;
  const { attachment, ...rest } = raw as { attachment: unknown };
  return { ...rest, attachments: attachment ? [attachment] : [] };
};

const draftSchema = z.preprocess(legacyAttachment, z.object({
  version: z.literal(1),
  text: z.string(),
  revision: z.number().int().nonnegative(),
  transcript: z.string().min(1).nullable(),
  dictationId: z.string().min(1).optional(),
  recordings: z.array(z.object({ id: z.string().min(1), serverPath: z.string().min(1) })).optional(),
  attachments: z.array(attachmentSchema),
  submission: z.object({
    text: z.string(),
    draftRevision: z.number().int().nonnegative(),
    status: z.enum(['sending', 'unknown', 'rejected']),
    // Relógio do envio (ms): só mensagem da conversa posterior a ele confirma um envio incerto.
    startedAt: z.number().optional(),
  }).nullable(),
}));

// Separado do anexo escolhido: transcrição tardia não substitui foto nem snapshot de envio.
const dictationSchema = z.object({
  version: z.literal(1),
  id: z.string().min(1),
  // null: ditado pedido pela galeria, sem cópia local; o áudio está só no servidor.
  audio: attachmentSchema.nullable(),
  // Caminho do áudio no servidor, gravado antes da transcrição: o "de novo" não sobe outra cópia.
  serverPath: z.string().min(1).optional(),
  // Estilo que o servidor aplicou (`estilo_aplicado`), para a barra marcar a versão certa.
  applied: z.string().optional(),
  transcript: z.string().min(1).nullable(),
  draftRevision: z.number().int().nonnegative(),
  before: z.string(),
  motivo: z.enum(['silencio', 'botao', 'teto', 'escondeu']),
  estilo: z.string().optional(),
  status: z.enum(['pending', 'ready', 'failed', 'applied']),
  text: z.string(),
  raw: z.string(),
  issue: z.string(),
});

export type DraftAttachment = z.infer<typeof attachmentSchema>;
export type ConversationDraft = z.infer<typeof draftSchema>;
export type DictationDraft = z.infer<typeof dictationSchema>;
export type UploadTarget = NonNullable<DraftAttachment['uploadedFor']>;

// Upload feito antes de a sessão ter transcript ainda é dela; transcript diferente é sessão recriada.
export function reusableUploadPath(attachment: DraftAttachment, target: UploadTarget): string | null {
  const done = attachment.uploadedFor;
  if (!attachment.uploadedPath || !done || done.serverId !== target.serverId || done.name !== target.name) return null;
  return done.transcript === null || done.transcript === target.transcript ? attachment.uploadedPath : null;
}

export function withoutUpload(attachment: DraftAttachment): DraftAttachment {
  const { uri, name, mime, kind } = attachment;
  return { uri, name, mime, kind };
}

const keyOf = (serverId: string, name: string) => `draft.v1:${serverId}::${name}`;
// Rascunho da sessão anterior de mesmo nome, guardado antes de a sessão recriada gravar na chave principal.
const recoverableKeyOf = (serverId: string, name: string) => `draft.v1.recoverable:${serverId}::${name}`;

export const readDraft = (serverId: string, name: string) => readAt(keyOf(serverId, name));
export const writeDraft = (serverId: string, name: string, value: ConversationDraft) => writeAt(keyOf(serverId, name), value);
export const clearDraft = (serverId: string, name: string) => clearAt(keyOf(serverId, name));

export function removeDraftRecording(serverId: string, name: string, id: string): ConversationDraft | null {
  const latest = readDraft(serverId, name);
  if (!latest) return null;
  const next = { ...latest, recordings: latest.recordings?.filter((recording) => recording.id !== id) };
  // Vazio some, como no Composer: guardado, viraria rascunho a recuperar na sessão recriada.
  if (!next.text && !next.attachments.length && !next.recordings?.length && !next.submission) {
    clearDraft(serverId, name);
    return null;
  }
  writeDraft(serverId, name, next);
  return next;
}
export const readRecoverableDraft = (serverId: string, name: string) => readAt(recoverableKeyOf(serverId, name));
export const writeRecoverableDraft = (serverId: string, name: string, value: ConversationDraft) =>
  writeAt(recoverableKeyOf(serverId, name), value);
export const clearRecoverableDraft = (serverId: string, name: string) => clearAt(recoverableKeyOf(serverId, name));

function readAt(key: string): ConversationDraft | null {
  let raw: string | undefined;
  try { raw = prefs.getString(key); } catch {
    throw new Error(m.draft_read_error());
  }
  if (raw === undefined) return null;

  let value: ConversationDraft;
  try { value = draftSchema.parse(JSON.parse(raw)); } catch {
    // O conteúdo continua guardado para recuperação; não expor JSON no erro.
    throw new Error(m.draft_invalid());
  }
  if (value.submission?.status === 'sending') value.submission.status = 'unknown';
  return value;
}

function writeAt(key: string, value: ConversationDraft): void {
  let validated: ConversationDraft;
  try { validated = draftSchema.parse(value); } catch {
    throw new Error(m.draft_write_error());
  }
  try { prefs.set(key, JSON.stringify(validated)); } catch {
    throw new Error(m.draft_write_error());
  }
}

function clearAt(key: string): void {
  try { prefs.remove(key); } catch {
    throw new Error(m.draft_clear_error());
  }
}

const dictationKeyOf = (serverId: string, name: string) => `draft.v1.dictation:${serverId}::${name}`;

// POST de ditado vivo neste processo: remontar o Composer não o dá por interrompido.
const inFlight = new Set<string>();
export function setDictationInFlight(serverId: string, name: string, id: string, on: boolean): void {
  const key = `${dictationKeyOf(serverId, name)}#${id}`;
  if (on) inFlight.add(key);
  else inFlight.delete(key);
}

export function readDictation(serverId: string, name: string): DictationDraft | null {
  let raw: string | undefined;
  try { raw = prefs.getString(dictationKeyOf(serverId, name)); } catch { throw new Error(m.draft_read_error()); }
  if (raw === undefined) return null;
  let value: DictationDraft;
  try { value = dictationSchema.parse(JSON.parse(raw)); } catch { throw new Error(m.draft_invalid()); }
  if (readDraft(serverId, name)?.dictationId === value.id
    || readRecoverableDraft(serverId, name)?.dictationId === value.id) {
    return { ...value, status: 'applied', issue: m.draft_clear_error() };
  }
  // Sem POST vivo neste processo, reabrir só oferece repetição explícita.
  return value.status === 'pending' && !inFlight.has(`${dictationKeyOf(serverId, name)}#${value.id}`)
    ? { ...value, status: 'failed', issue: m.composer_ditado_interrompido() } : value;
}

export function writeDictation(serverId: string, name: string, value: DictationDraft): void {
  let validated: DictationDraft;
  try { validated = dictationSchema.parse(value); } catch { throw new Error(m.draft_write_error()); }
  // Outro POST desta conversa ainda no ar: gravar por cima perderia o áudio e o resultado dele.
  if (validated.status === 'pending') {
    let current: DictationDraft | null = null;
    // Guardado ilegível: o novo grava por cima, como no rascunho, mas deixa rastro.
    try { current = readDictation(serverId, name); } catch (e) { console.warn('dictation: unreadable stored dictation overwritten', e); }
    if (current?.status === 'pending' && current.id !== validated.id) throw new Error(m.composer_aguarde_transcricao());
  }
  try { prefs.set(dictationKeyOf(serverId, name), JSON.stringify(validated)); } catch { throw new Error(m.draft_write_error()); }
}

export const clearDictation = (serverId: string, name: string) => clearAt(dictationKeyOf(serverId, name));

export function associateDictationTranscript(serverId: string, name: string, transcript: string): DictationDraft | null {
  const voice = readDictation(serverId, name);
  if (!voice || voice.transcript !== null) return voice;
  const associated = { ...voice, transcript };
  writeDictation(serverId, name, associated);
  return associated;
}

export function finishDictation(serverId: string, name: string, id: string,
  result: Pick<DictationDraft, 'text' | 'raw' | 'issue'> & Partial<Pick<DictationDraft, 'serverPath' | 'applied'>>, active: boolean,
): { draft: ConversationDraft | null; dictation: DictationDraft | null } {
  const voice = readDictation(serverId, name);
  if (!voice || voice.id !== id) return { draft: null, dictation: voice };
  if (voice.status === 'applied') return { draft: null, dictation: voice };
  const completed: DictationDraft = { ...voice, ...result, status: 'ready' };
  writeDictation(serverId, name, completed);
  const latest = readDraft(serverId, name);
  if (!active || !latest || latest.revision !== voice.draftRevision
    || (voice.transcript !== null && latest.transcript !== voice.transcript)) {
    return { draft: null, dictation: completed };
  }
  return insertDictation(serverId, name, latest, completed);
}

export function recoverDictation(serverId: string, name: string, id: string): ReturnType<typeof finishDictation> {
  const voice = readDictation(serverId, name);
  if (!voice || voice.id !== id || voice.status !== 'ready' || !voice.text) return { draft: null, dictation: voice };
  const latest = readDraft(serverId, name)
    ?? { version: 1 as const, text: '', revision: 0, transcript: null, attachments: [], submission: null };
  return insertDictation(serverId, name, latest, voice);
}

// Entra sozinho só com o rascunho como estava ao gravar e na mesma conversa (jsonl).
export function applyReadyDictation(serverId: string, name: string, transcript: string | null):
  (ReturnType<typeof finishDictation> & { voice: DictationDraft }) | null {
  const voice = readDictation(serverId, name);
  if (!voice || voice.status !== 'ready' || !voice.text) return null;
  if (voice.transcript !== null && voice.transcript !== transcript) return null;
  const latest = readDraft(serverId, name)
    ?? { version: 1 as const, text: '', revision: 0, transcript, attachments: [], submission: null };
  if (latest.revision !== voice.draftRevision) return null;
  if (voice.transcript !== null && latest.transcript !== null && latest.transcript !== voice.transcript) return null;
  return { ...insertDictation(serverId, name, latest, voice), voice };
}

function insertDictation(serverId: string, name: string, latest: ConversationDraft, voice: DictationDraft): ReturnType<typeof finishDictation> {
  // Texto e identidade entram juntos; falhar a limpeza depois não reoferece a mesma inserção.
  const next = { ...latest, text: latest.text.trim() ? `${latest.text.trim()} ${voice.text}` : voice.text,
    revision: latest.revision + 1, dictationId: voice.id,
    ...(voice.serverPath ? { recordings: [...(latest.recordings ?? []).filter((recording) => recording.id !== voice.id),
      { id: voice.id, serverPath: voice.serverPath }] } : {}),
  };
  writeDraft(serverId, name, next);
  try { clearDictation(serverId, name); } catch {
    return { draft: next, dictation: { ...voice, status: 'applied', issue: m.draft_clear_error() } };
  }
  return { draft: next, dictation: null };
}

export function resolveDraftTranscript(serverId: string, name: string, transcript: string | null): {
  draft: ConversationDraft | null;
  recoverable: ConversationDraft | null;
} {
  const draft = readDraft(serverId, name);
  if (!draft || transcript === null || draft.transcript === transcript) {
    return { draft, recoverable: null };
  }
  if (draft.transcript === null) {
    const associated = { ...draft, transcript };
    writeDraft(serverId, name, associated);
    return { draft: associated, recoverable: null };
  }
  // O consumidor conserva recoverable antes de gravar texto da sessão recriada.
  return { draft: null, recoverable: draft };
}
