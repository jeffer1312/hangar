import type { OrqFeedItem, OrqFeedKind, OrqWatchdog } from '@hangar/core';

export type ConductorChip =
  | { kind: 'alive'; lastCycle: number | null; watching: string[] }
  | { kind: 'stopped'; lastCycle: number | null }
  | { kind: 'none' }
  | { kind: 'unavailable' };

// Servidor antigo da malha não manda `watchdog`: sem dado, sem chip — nunca um "parado" inventado.
export function conductorChip(w: OrqWatchdog | null | undefined): ConductorChip | null {
  if (!w) return null;
  const ms = w.last_cycle ? Date.parse(w.last_cycle) : Number.NaN;
  const lastCycle = Number.isFinite(ms) ? ms / 1000 : null;
  if (w.alive) return { kind: 'alive', lastCycle, watching: w.watching };
  if (w.source === 'unavailable') return { kind: 'unavailable' };
  if (w.source === 'none') return { kind: 'none' };
  return { kind: 'stopped', lastCycle };
}

export type FeedFilter = 'all' | OrqFeedKind;
export const FEED_FILTERS: FeedFilter[] = ['all', 'woke', 'dropped', 'notice', 'alarm', 'event'];

export function filterFeed(feed: OrqFeedItem[], kind: FeedFilter, task: number | null): OrqFeedItem[] {
  return feed.filter((i) => (kind === 'all' || i.kind === kind) && (task === null || i.task === task));
}

export function feedTasks(feed: OrqFeedItem[]): number[] {
  return [...new Set(feed.flatMap((i) => (i.task === null ? [] : [i.task])))].sort((a, b) => a - b);
}

export function feedTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleString(undefined, { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

export function fmtP(v: number | null | undefined): string {
  return typeof v === 'number' && Number.isFinite(v) ? v.toFixed(2) : '—';
}
