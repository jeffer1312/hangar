import { useCallback, useMemo } from 'react';
import { fmtBytes, intlLocale, type AttachmentOrigin } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { haptic } from '../../ui/haptics';
import { Lightbox, type LightboxProps, type LightboxStripItem } from './Lightbox';
import { LightboxModal } from './LightboxModal';

export interface GalleryEntry {
  key: string;
  uri: string;
  /** Versão reduzida para miniatura; o visor abre sempre o `uri`. */
  thumbUri?: string;
  headers?: Record<string, string>;
  name: string;
  kind: 'image' | 'video';
  origin?: AttachmentOrigin;
  ts?: number | null;
  size?: number | null;
  /** Prazo do upload em dias (null = sem prazo); ausente quando não é upload conhecido. */
  expiresInDays?: number | null;
  /** Mensagem onde a mídia está; sem ela não há "Na conversa". */
  eventId?: string;
}

const time = (d: Date) => d.toLocaleTimeString(intlLocale(), { hour: '2-digit', minute: '2-digit' });

function sameDay(a: Date, b: Date) {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

function relativeDay(d: Date): 'today' | 'yesterday' | null {
  const now = new Date();
  if (sameDay(d, now)) return 'today';
  now.setDate(now.getDate() - 1);
  return sameDay(d, now) ? 'yesterday' : null;
}

/** "hoje 11:42", "ontem 22:05" ou a data com a hora. */
export function mediaWhen(ts: number): string {
  const d = new Date(ts * 1000);
  const rel = relativeDay(d);
  if (rel === 'today') return m.media_when_today({ time: time(d) });
  if (rel === 'yesterday') return m.media_when_yesterday({ time: time(d) });
  return m.media_when_date({ date: d.toLocaleDateString(intlLocale(), { day: 'numeric', month: 'short' }), time: time(d) });
}

/** Cabeçalho do grupo do dia: "Hoje", "Ontem" ou a data. */
export function mediaDayTitle(day: number | null): string {
  if (day === null) return m.media_day_undated();
  const d = new Date(day * 1000);
  const rel = relativeDay(d);
  if (rel === 'today') return m.media_day_today();
  if (rel === 'yesterday') return m.media_day_yesterday();
  return d.toLocaleDateString(intlLocale(), { weekday: 'long', day: 'numeric', month: 'long' });
}

export const originLabel = (o: AttachmentOrigin) => (o === 'agent' ? m.media_origin_agent() : m.media_origin_user());

/** Prazo do upload em texto curto; `urgent` pinta de aviso. */
export function expiryText(d: number | null): { txt: string; urgent: boolean } {
  if (d === null) return { txt: m.anexos_sem_expiracao(), urgent: false };
  if (d <= 0) return { txt: m.anexos_vencido(), urgent: true };
  if (d < 1) return { txt: m.anexos_expira_h({ n: Math.max(1, Math.round(d * 24)) }), urgent: true };
  return { txt: m.anexos_expira_d({ n: Math.round(d) }), urgent: d <= 3 };
}

/** "580 KB · hoje 11:42 · agente", com o prazo do upload no fim; o que não se sabe fica fora. */
export function entryDetails(e: Pick<GalleryEntry, 'size' | 'ts' | 'origin' | 'expiresInDays'>): string {
  return [
    e.size != null ? fmtBytes(e.size) : null,
    e.ts ? mediaWhen(e.ts) : null,
    e.origin ? originLabel(e.origin) : null,
    e.expiresInDays !== undefined ? expiryText(e.expiresInDays).txt : null,
  ].filter(Boolean).join(' · ');
}

/** Visor de uma lista de mídias aberto em `openKey`. A chave, e não o índice: a lista pode mudar
 *  com ele aberto (mensagem nova, recarga), e a mídia na tela continua a mesma. */
export function GalleryViewer({ entries, openKey, onOpenKey, onClose, onShowInChat, modal = true }: {
  entries: GalleryEntry[];
  openKey: string | null;
  onOpenKey: (key: string) => void;
  onClose: () => void;
  onShowInChat?: (entry: GalleryEntry) => void;
  /** false = camada dentro da tela (a rota já é uma folha); true = Modal de tela cheia. */
  modal?: boolean;
}) {
  const index = openKey === null ? -1 : entries.findIndex((e) => e.key === openKey);
  const cur = index >= 0 ? entries[index] : null;
  const strip = useMemo<LightboxStripItem[]>(
    () => entries.map((e) => ({ key: e.key, uri: e.thumbUri ?? e.uri, headers: e.headers, name: e.name, kind: e.kind })),
    [entries],
  );
  const select = useCallback((i: number) => {
    const next = entries[i];
    if (!next || next.key === openKey) return;
    haptic.selection();
    onOpenKey(next.key);
  }, [entries, openKey, onOpenKey]);
  const step = useCallback((dir: 1 | -1) => {
    if (index < 0) return;
    select(Math.min(entries.length - 1, Math.max(0, index + dir)));
  }, [index, entries.length, select]);

  const props: Omit<LightboxProps, 'visible' | 'onClose'> | null = cur ? {
    uri: cur.uri,
    headers: cur.headers,
    filename: cur.name,
    kind: cur.kind,
    counter: entries.length > 1 ? m.gallery_counter({ n: index + 1, total: entries.length }) : undefined,
    onStep: entries.length > 1 ? step : undefined,
    details: entryDetails(cur) || undefined,
    onShowInChat: onShowInChat && cur.eventId ? () => onShowInChat(cur) : undefined,
    strip: entries.length > 1 ? { items: strip, index, onSelect: select } : undefined,
  } : null;

  if (modal) return <LightboxModal media={props} onClose={onClose} />;
  return <Lightbox visible={!!props} {...(props ?? { uri: '', filename: '' })} onClose={onClose} />;
}
