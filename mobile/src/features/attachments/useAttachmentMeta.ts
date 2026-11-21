import { useEffect, useRef, useState } from 'react';
import { baseOf, listUploads, registrarDiag, resolverCitados, type Server, type UploadFile } from '@hangar/core';
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
 *  isso a falha não vira aviso na tela (a lista de uploads falhada aparece em "Mídia e arquivos"),
 *  mas vai ao diário de uso. */
export function useGalleryMeta(server: Server | null, sessionName: string, citedPaths: string[], uploads: boolean): AttachmentMeta {
  const [meta, setMeta] = useState<AttachmentMeta>(EMPTY_META);
  const pathsKey = citedPaths.join('\n');
  // O objeto do servidor troca a cada atualização da lista; só outra máquina ou outro endereço busca de novo.
  const serverRef = useRef(server);
  // Declarado antes da busca: no mesmo commit, roda primeiro.
  useEffect(() => { serverRef.current = server; }, [server]);
  const serverKey = server ? `${server.id}\n${baseOf(server)}` : null;
  useEffect(() => {
    const target = serverRef.current;
    if (!target || !sessionName) return;
    let alive = true;
    const failed = (codigo: string) => (e: unknown) => {
      registrarDiag({ evento: 'gallery.meta_failed', nivel: 'aviso', codigo, detalhe: (e instanceof Error ? e.message : String(e)).slice(0, 300) });
    };
    const listed: Promise<UploadFile[]> = uploads
      ? listUploads(sessionName, target).then((r) => r.files, (e: unknown) => { failed('gallery_uploads_failed')(e); return []; })
      : Promise.resolve([]);
    const cited = citedMeta(sessionName, citedPaths, target).catch((e: unknown) => {
      failed('gallery_cited_failed')(e);
      return new Map<string, { size?: number; mtime?: number }>();
    });
    void Promise.all([listed, cited]).then(([files, c]) => {
      if (alive) setMeta({ uploads: new Map(files.map((f) => [f.filename, f])), cited: c });
    });
    return () => { alive = false; };
    // `pathsKey` resume a lista: outro array com os mesmos caminhos não busca de novo.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverKey, sessionName, pathsKey, uploads]);
  return meta;
}
