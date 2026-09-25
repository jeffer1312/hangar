import { relativeTime, type OrqFeedItem, type OrqFeedKind, type OrqWatchdog } from '@hangar/core';
import * as m from '../paraglide/messages';

export type ConductorChip =
  | { kind: 'alive'; lastCycle: number | null; watching: string[] }
  // `alarm` falso: execução terminada, condutor parado é o esperado e não pede atenção.
  | { kind: 'stopped'; lastCycle: number | null; alarm: boolean }
  | { kind: 'none' }
  | { kind: 'unavailable' };

// Servidor antigo da malha não manda `watchdog`: sem dado, sem chip — nunca um "parado" inventado.
export function conductorChip(w: OrqWatchdog | null | undefined, finished = false): ConductorChip | null {
  if (!w) return null;
  const ms = w.last_cycle ? Date.parse(w.last_cycle) : Number.NaN;
  const lastCycle = Number.isFinite(ms) ? ms / 1000 : null;
  if (w.alive) return { kind: 'alive', lastCycle, watching: w.watching };
  if (w.source === 'unavailable') return { kind: 'unavailable' };
  if (w.source === 'none') return { kind: 'none' };
  return { kind: 'stopped', lastCycle, alarm: !finished };
}

export function conductorChipLabel(chip: ConductorChip): string {
  switch (chip.kind) {
    case 'alive':
      if (chip.lastCycle === null) return m.orq_conductor_alive_no_heartbeat();
      return chip.watching.length
        ? m.orq_conductor_alive({ when: relativeTime(chip.lastCycle), who: chip.watching.join(', ') })
        : m.orq_conductor_alive_idle({ when: relativeTime(chip.lastCycle) });
    case 'stopped':
      return chip.lastCycle === null ? m.orq_conductor_stopped() : m.orq_conductor_stopped_since({ when: relativeTime(chip.lastCycle) });
    case 'none':
      return m.orq_conductor_none();
    case 'unavailable':
      return m.orq_conductor_unavailable();
  }
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
