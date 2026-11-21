import { useEffect, useState } from 'react';
import { listUploads, resolverCitados, type Server, type UploadFile } from '@hangar/core';
import { EMPTY_META, type AttachmentMeta } from './attachmentUrls';

/** Tamanho e data dos citados (`files/resolver`). Servidor que ainda não mede não manda: fica sem. */
export async function citedMeta(sessionName: string, paths: string[], server: Server): Promise<AttachmentMeta['cited']> {
  const out = new Map<string, { size?: number; mtime?: number }>();
  if (!paths.length) return out;
  const r = await resolverCitados(sessionName, paths, server);
  for (const [cru, v] of Object.entries(r.ok)) {
    out.set(cru, {
      size: typeof v.size === 'number' ? v.size : undefined,
      mtime: typeof v.mtime === 'number' ? v.mtime : undefined,
    });
  }
  return out;
}

/** Só a legenda da galeria da conversa usa: o tamanho é detalhe, e sem ele a mídia abre igual. Por
 *  isso a falha não vira aviso aqui; a lista de uploads falhada aparece na tela "Mídia e arquivos". */
export function useGalleryMeta(server: Server | null, sessionName: string, citedPaths: string[], uploads: boolean): AttachmentMeta {
  const [meta, setMeta] = useState<AttachmentMeta>(EMPTY_META);
  const pathsKey = citedPaths.join('\n');
  useEffect(() => {
    if (!server || !sessionName) return;
    let alive = true;
    const listed: Promise<UploadFile[]> = uploads ? listUploads(sessionName, server).then((r) => r.files, () => []) : Promise.resolve([]);
    const cited = citedMeta(sessionName, citedPaths, server).catch(() => new Map<string, { size?: number; mtime?: number }>());
    void Promise.all([listed, cited]).then(([files, c]) => {
      if (alive) setMeta({ uploads: new Map(files.map((f) => [f.filename, f])), cited: c });
    });
    return () => { alive = false; };
    // `pathsKey` resume a lista: outro array com os mesmos caminhos não busca de novo.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [server, sessionName, pathsKey, uploads]);
  return meta;
}
