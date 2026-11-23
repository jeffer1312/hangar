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

/** Chave da cópia guardada: servidor + sessão + caminho, não a URL, porque a URL muda com a rota
 *  (rede local ↔ Tailscale) e o arquivo baixado deixaria de ser achado sem conexão. Data e tamanho
 *  ficam fora: chegam depois do download começar, e a chave mudaria entre gravar e ler. Eles vão
 *  junto da cópia (`KeptVersion`) e são conferidos na leitura. A imagem da conversa leva o evento:
 *  o `ref` dela é só a posição na mensagem, e repete de uma mensagem para outra. */
export function keptKey(serverId: string, sessionName: string, item: Pick<Ref, 'source' | 'ref' | 'eventId'>): string {
  const ref = item.source === 'transcript' ? `${item.eventId}#${item.ref}` : item.ref;
  return `${serverId}::${sessionName}::${item.source}:${ref}`;
}

/** Versão do arquivo no servidor, do que se sabe dela. */
export interface KeptVersion { size: number | null; mtime: number | null }

/** A cópia guardada ainda é o arquivo do servidor? Só o que os dois lados sabem e difere a descarta:
 *  o tamanho contra os bytes baixados, a data contra a gravada com a cópia. */
export function keptIsCurrent(stored: KeptVersion | null, keptBytes: number | null, now: KeptVersion): boolean {
  if (now.size != null && keptBytes != null && now.size !== keptBytes) return false;
  if (now.mtime != null && stored?.mtime != null && now.mtime !== stored.mtime) return false;
  return true;
}

/** A versão que vale para os bytes baixados, da medida do começo (`start`) e do fim (`end`). A data
 *  do começo vence: se o arquivo mudou no meio, a do fim descreveria bytes que talvez não vieram, e a
 *  cópia passaria por atual. Data que só chegou no meio vale para a cópia. */
export function downloadedVersion(start: KeptVersion, end: KeptVersion): KeptVersion {
  return { size: end.size ?? start.size, mtime: start.mtime ?? end.mtime };
}

export function attachmentMtime(a: Pick<Ref, 'source' | 'ref'>, meta: AttachmentMeta): number | null {
  const mtime = a.source === 'upload' ? meta.uploads.get(a.ref)?.mtime : a.source === 'cited' ? meta.cited.get(a.ref)?.mtime : undefined;
  return typeof mtime === 'number' && Number.isFinite(mtime) ? mtime : null;
}

/** Prazo do upload; undefined quando não é um upload que a lista conhece. */
export function attachmentExpiry(a: Pick<Ref, 'source' | 'ref'>, meta: AttachmentMeta): number | null | undefined {
  if (a.source !== 'upload') return undefined;
  const f = meta.uploads.get(a.ref);
  return f ? f.expires_in_days : undefined;
}
