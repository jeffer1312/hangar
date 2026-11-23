// Tudo o que a conversa anexou ou citou, na ordem em que as bolhas desenham: a galeria da conversa
// e a tela "Mídia e arquivos" leem daqui, e a chave de cada item casa com a miniatura da bolha.
import type { ChatEvent } from './types';
import { fileKind, parseFilePaths, parseImageMessage, parseMediaUrls, type FileKind } from './format';

export type AttachmentOrigin = 'agent' | 'user';
/** `cited`: caminho na resposta (rota /file); `remote`: URL http(s) de mídia; `upload`: anexo enviado
 *  (rota /uploads); `transcript`: imagem colada no terminal (rota /transcript-image). */
export type AttachmentSource = 'cited' | 'remote' | 'upload' | 'transcript';

export interface ConversationAttachment {
  /** `<id do evento>#<posição na bolha>`: a miniatura da bolha monta a mesma chave. */
  key: string;
  eventId: string;
  ts: number | null;
  origin: AttachmentOrigin;
  source: AttachmentSource;
  kind: FileKind;
  /** Vazio na imagem do terminal: quem desenha usa o rótulo de "imagem enviada". */
  name: string;
  /** Caminho citado, URL remota, nome do upload ou, no terminal, o índice da imagem no evento. */
  ref: string;
}

export interface UserMessageFiles {
  videos: string[];
  images: string[];
  /** Imagens que vêm do transcript (coladas no terminal), não do texto. */
  transcript: number;
  /** Anexos `arquivo:` que não são vídeo. */
  files: string[];
}

/** Anexos da mensagem do usuário, na ordem da bolha: vídeos, fotos, imagens do terminal, arquivos.
 *  `imageCount` só conta quando a bolha tem o evento real (a do eco não tem). */
export function userMessageFiles(text: string, imageCount: number): UserMessageFiles | null {
  const parsed = parseImageMessage(text);
  if (!parsed && imageCount <= 0) return null;
  const transcript = Math.max(0, imageCount);
  // Com o caminho escrito para TODAS as fotos, as últimas `imageCount` já vêm do transcript: as duas
  // fontes duplicariam cada imagem.
  const images = !parsed ? []
    : transcript > 0 && parsed.marcadores === parsed.filenames.length
      ? parsed.filenames.slice(0, Math.max(0, parsed.filenames.length - transcript))
      : parsed.filenames;
  const files = parsed?.files ?? [];
  return {
    videos: files.filter((f) => fileKind(f) === 'video'),
    images,
    transcript,
    files: files.filter((f) => fileKind(f) !== 'video'),
  };
}

// Eco da fila e envio desistido são desenhados por outra bolha, sem o evento real: ficam fora.
function heldBack(ev: ChatEvent): boolean {
  return ev.id.startsWith('queued-') || ev.id.startsWith('held:') || !!ev.desistiu || ev.hook_error != null;
}

export function messageAttachments(ev: ChatEvent): ConversationAttachment[] {
  const ts = ev.ts ?? null;
  const out: ConversationAttachment[] = [];
  const push = (origin: AttachmentOrigin, source: AttachmentSource, kind: FileKind, name: string, ref: string) => {
    out.push({ key: `${ev.id}#${out.length}`, eventId: ev.id, ts, origin, source, kind, name, ref });
  };
  if (ev.kind === 'assistant_msg') {
    const text = ev.text ?? '';
    for (const r of parseFilePaths(text)) push('agent', 'cited', r.kind, r.name, r.path);
    for (const r of parseMediaUrls(text)) push('agent', 'remote', r.kind, r.name, r.url ?? r.path);
    return out;
  }
  if (ev.kind !== 'user_msg' || heldBack(ev)) return out;
  const f = userMessageFiles(ev.text ?? '', ev.image_count ?? 0);
  if (!f) return out;
  for (const v of f.videos) push('user', 'upload', 'video', v, v);
  for (const i of f.images) push('user', 'upload', 'image', i, i);
  for (let i = 0; i < f.transcript; i++) push('user', 'transcript', 'image', '', String(i));
  for (const x of f.files) push('user', 'upload', fileKind(x) ?? 'document', x, x);
  return out;
}

export function conversationAttachments(events: readonly ChatEvent[]): ConversationAttachment[] {
  return events.flatMap(messageAttachments);
}

export const isGalleryKind = (kind: FileKind | null | undefined): kind is 'image' | 'video' => kind === 'image' || kind === 'video';

/** Imagens e vídeos da conversa, do mais antigo ao mais novo: a ordem de passar na galeria. */
export function conversationMedia(events: readonly ChatEvent[]): ConversationAttachment[] {
  return conversationAttachments(events).filter((a) => isGalleryKind(a.kind));
}

export type OriginFilter = 'all' | AttachmentOrigin;

export function filterByOrigin<T extends { origin: AttachmentOrigin }>(items: readonly T[], filter: OriginFilter): T[] {
  return filter === 'all' ? [...items] : items.filter((x) => x.origin === filter);
}

/** Meia-noite local do dia, em segundos. */
export function localDayStart(ts: number): number {
  const d = new Date(ts * 1000);
  d.setHours(0, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
}

export interface DayGroup<T> {
  /** Meia-noite local do dia; null = sem data. */
  day: number | null;
  items: T[];
}

/** Grupos por dia na ordem em que os itens chegam; item sem data fica num grupo próprio no fim. */
export function groupByDay<T>(items: readonly T[], tsOf: (item: T) => number | null | undefined): DayGroup<T>[] {
  const groups: DayGroup<T>[] = [];
  const byDay = new Map<number, DayGroup<T>>();
  let undated: DayGroup<T> | null = null;
  for (const item of items) {
    const ts = tsOf(item);
    if (ts == null || !Number.isFinite(ts) || ts <= 0) {
      if (!undated) undated = { day: null, items: [] };
      undated.items.push(item);
      continue;
    }
    const day = localDayStart(ts);
    let g = byDay.get(day);
    if (!g) {
      g = { day, items: [] };
      byDay.set(day, g);
      groups.push(g);
    }
    g.items.push(item);
  }
  return undated ? [...groups, undated] : groups;
}

/** Do mais novo ao mais velho; sem data vai para o fim, e empate mantém a ordem de entrada. */
export function newestFirst<T>(items: readonly T[], tsOf: (item: T) => number | null | undefined): T[] {
  return items
    .map((item, i) => ({ item, i, ts: tsOf(item) ?? null }))
    .sort((a, b) => (a.ts === b.ts ? a.i - b.i : a.ts === null ? 1 : b.ts === null ? -1 : b.ts - a.ts))
    .map((x) => x.item);
}

/** Pasta do caminho citado para a linha do arquivo: `/home/<usuário>/` vira `~/`; nome solto, vazio. */
export function attachmentFolder(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  parts.pop();
  const dir = parts.join('/');
  if (!dir) return path.startsWith('/') ? '/' : '';
  const home = /^\/home\/[^/]+(?=\/|$)/.exec(dir) ?? /^\/Users\/[^/]+(?=\/|$)/.exec(dir);
  const short = home ? `~${dir.slice(home[0].length)}` : dir;
  return `${short}/`;
}
