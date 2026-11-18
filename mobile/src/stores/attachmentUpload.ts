import { uploadFromDiskForServer, waitUploadMediaForServer, type Server } from '@hangar/core';
import { diskFileSize, sendFromDisk } from '../net/uploadFromDisk';
import type { PickedAttachment } from '../ui/attachmentPicker';

/** O que o upload grava no anexo do rascunho. Os quatro campos vão sempre, para trocar o que havia. */
export type UploadedFields = {
  uploadedPath: string;
  uploadedMediaId: string | undefined;
  uploadedFrames: string[] | undefined;
  uploadedSpeech: string | undefined;
};

/**
 * Sobe o anexo do disco e, no vídeo, espera os quadros e a fala. `save` grava cada etapa no rascunho:
 * o caminho e o id da mídia vão antes da espera, para o app fechado no meio não subir o vídeo de novo.
 * `pending` retoma a espera de um vídeo já subido.
 */
export async function uploadAttachment(
  server: Server,
  session: string,
  attach: PickedAttachment,
  save: (fields: UploadedFields) => void,
  opts: { pending?: { path: string; mediaId: string } | null; signal?: AbortSignal } = {},
): Promise<UploadedFields> {
  let fields: UploadedFields;
  if (opts.pending) {
    fields = { uploadedPath: opts.pending.path, uploadedMediaId: opts.pending.mediaId, uploadedFrames: undefined, uploadedSpeech: undefined };
  } else {
    const size = attach.size ?? await diskFileSize(attach.uri);
    const saved = await uploadFromDiskForServer(server, session, { name: attach.name, type: attach.mime, size }, sendFromDisk(attach.uri));
    fields = { uploadedPath: saved.path, uploadedMediaId: saved.media_id, uploadedFrames: undefined, uploadedSpeech: undefined };
    save(fields);
  }
  if (!fields.uploadedMediaId) return fields;
  const media = await waitUploadMediaForServer(server, session, fields.uploadedMediaId, opts.signal);
  fields = { uploadedPath: fields.uploadedPath, uploadedMediaId: undefined, uploadedFrames: media?.frames ?? [], uploadedSpeech: media?.transcript ?? '' };
  save(fields);
  return fields;
}
