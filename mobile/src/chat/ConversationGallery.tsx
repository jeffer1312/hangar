import { forwardRef, lazy, Suspense, useEffect, useImperativeHandle, useMemo, useRef, useState } from 'react';
import { conversationMedia, type ChatEvent } from '@hangar/core';
import type { GalleryEntry } from '../features/attachments/gallery';
import { attachmentExpiry, attachmentHeaders, attachmentName, attachmentSize, attachmentThumbUri, attachmentUri } from '../features/attachments/attachmentUrls';
import { useGalleryMeta } from '../features/attachments/useAttachmentMeta';
import { useServers } from '../stores/servers';
import { useRouteBase } from '../net/routeBase';
import { toast } from '../ui/Toast';
import * as m from '../paraglide/messages';

// O visor puxa os módulos nativos de arquivo (salvar, compartilhar, vídeo): a lista da conversa não
// os carrega, e eles só entram quando a pessoa abre uma mídia.
const GalleryViewer = lazy(() => import('../features/attachments/gallery').then((mod) => ({ default: mod.GalleryViewer })));

export interface ConversationGalleryHandle {
  /** false = a chave não está na conversa carregada. */
  open: (key: string) => boolean;
}

interface Props {
  events: ChatEvent[];
  serverId: string;
  sessionName: string;
  /** Leva a lista até a mensagem; false = ela não está na lista (o visor fica aberto e avisa). */
  onShowInChat: (eventId: string) => boolean;
}

// Fechada, só guarda a chave: nada do visor monta, e evento novo não refaz a lista de mídias.
export const ConversationGallery = forwardRef<ConversationGalleryHandle, Props>(function ConversationGallery(
  { events, serverId, sessionName, onShowInChat }, ref,
) {
  const [openKey, setOpenKey] = useState<string | null>(null);
  const eventsRef = useRef(events);
  useEffect(() => { eventsRef.current = events; }, [events]);
  useImperativeHandle(ref, () => ({
    open: (key) => {
      if (!conversationMedia(eventsRef.current).some((a) => a.key === key)) return false;
      setOpenKey(key);
      return true;
    },
  }), []);
  if (openKey === null) return null;
  return (
    <OpenGallery
      events={events}
      serverId={serverId}
      sessionName={sessionName}
      openKey={openKey}
      onOpenKey={setOpenKey}
      onClose={() => setOpenKey(null)}
      onShowInChat={(eventId) => {
        if (onShowInChat(eventId)) setOpenKey(null);
        else toast.erro(m.gallery_jump_not_found());
      }}
    />
  );
});

function OpenGallery({ events, serverId, sessionName, openKey, onOpenKey, onClose, onShowInChat }: Omit<Props, 'onShowInChat'> & {
  openKey: string;
  onOpenKey: (key: string) => void;
  onClose: () => void;
  onShowInChat: (eventId: string) => void;
}) {
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  const routeBase = useRouteBase(server);
  const media = useMemo(() => conversationMedia(events), [events]);
  const cited = useMemo(() => media.filter((a) => a.source === 'cited').map((a) => a.ref), [media]);
  const hasUploads = useMemo(() => media.some((a) => a.source === 'upload'), [media]);
  const meta = useGalleryMeta(server, sessionName, cited, hasUploads);
  const entries = useMemo<GalleryEntry[]>(() => (server ? media.map((a) => ({
    key: a.key,
    uri: attachmentUri(a, sessionName, server),
    thumbUri: attachmentThumbUri(a, sessionName, server),
    headers: attachmentHeaders(a, server),
    name: attachmentName(a),
    kind: a.kind === 'video' ? 'video' as const : 'image' as const,
    origin: a.origin,
    ts: a.ts,
    size: attachmentSize(a, meta),
    expiresInDays: attachmentExpiry(a, meta),
    eventId: a.eventId,
  })) : []),
  // `routeBase`: a URL leva o endereço da rota, que muda sem trocar o objeto do servidor.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  [media, server, sessionName, meta, routeBase]);
  // Servidor removido ou mídia que saiu da conversa (troca de transcript) com o visor aberto: fecha.
  const gone = !server || !entries.some((e) => e.key === openKey);
  useEffect(() => { if (gone) onClose(); }, [gone, onClose]);
  return (
    <Suspense fallback={null}>
      <GalleryViewer
        entries={entries}
        openKey={openKey}
        onOpenKey={onOpenKey}
        onClose={onClose}
        onShowInChat={(e) => { if (e.eventId) onShowInChat(e.eventId); }}
      />
    </Suspense>
  );
}
