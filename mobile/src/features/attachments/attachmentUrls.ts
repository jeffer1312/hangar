import { fileAuthHeader, fileUrlNative, transcriptImageUrlNative, uploadUrlNative, type ConversationAttachment, type Server, type UploadFile } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { thumbWidthFor } from '../../chat/thumbs';

type Ref = Pick<ConversationAttachment, 'source' | 'ref' | 'eventId' | 'name' | 'kind'>;

// As mesmas rotas que as bolhas montam: a miniatura da conversa e a desta tela saem do mesmo cache.
export function attachmentUri(a: Ref, sessionName: string, server: Server, width?: number): string {
  switch (a.source) {
    case 'cited': return fileUrlNative(sessionName, a.ref, server, width);
    case 'upload': return uploadUrlNative(sessionName, a.ref, server, width);
    case 'transcript': return transcriptImageUrlNative(sessionName, a.eventId, Number(a.ref), server);
    case 'remote': return a.ref;
  }
}

/** URL remota vai sem o token: é de outro site. */
export function attachmentHeaders(a: Pick<Ref, 'source'>, server: Server): Record<string, string> | undefined {
  return a.source === 'remote' ? undefined : fileAuthHeader(server);
}

/** Miniatura reduzida pelo servidor; só para imagem servida por ele e de tipo que ele reduz. */
export function attachmentThumbUri(a: Ref, sessionName: string, server: Server, box?: number): string | undefined {
  if (a.kind !== 'image' || a.source === 'remote' || a.source === 'transcript') return undefined;
  const width = thumbWidthFor(a.name, box);
  return width ? attachmentUri(a, sessionName, server, width) : undefined;
}

export function attachmentName(a: Pick<Ref, 'name'>): string {
  return a.name || m.anexos_imagem_enviada();
}

/** Tamanho e data que o servidor informou: uploads pela lista, citados pelo `files/resolver`. */
export interface AttachmentMeta {
  uploads: ReadonlyMap<string, UploadFile>;
  cited: ReadonlyMap<string, { size?: number; mtime?: number }>;
}

export const EMPTY_META: AttachmentMeta = { uploads: new Map(), cited: new Map() };

export function attachmentSize(a: Pick<Ref, 'source' | 'ref'>, meta: AttachmentMeta): number | null {
  const size = a.source === 'upload' ? meta.uploads.get(a.ref)?.size : a.source === 'cited' ? meta.cited.get(a.ref)?.size : undefined;
  return typeof size === 'number' && Number.isFinite(size) ? size : null;
}

/** Prazo do upload; undefined quando não é um upload que a lista conhece. */
export function attachmentExpiry(a: Pick<Ref, 'source' | 'ref'>, meta: AttachmentMeta): number | null | undefined {
  if (a.source !== 'upload') return undefined;
  const f = meta.uploads.get(a.ref);
  return f ? f.expires_in_days : undefined;
}
