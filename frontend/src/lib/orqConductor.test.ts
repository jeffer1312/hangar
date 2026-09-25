import { describe, expect, it } from 'vitest';
import type { OrqFeedItem, OrqWatchdog } from '@hangar/core';
import { conductorChip, conductorChipLabel, feedTasks, filterFeed, fmtP } from './orqConductor';

const wd = (o: Partial<OrqWatchdog>): OrqWatchdog => ({
  alive: false, last_cycle: null, since: null, restarts: null, unit: null, unit_state: null,
  arbiter: null, watching: [], source: 'none', ...o,
});

const item = (id: string, kind: OrqFeedItem['kind'], task: number | null): OrqFeedItem =>
  ({ id, ts: '2026-09-25T10:00:00-03:00', kind, text: id, task, jev: null });

describe('conductorChip', () => {
  it('sem watchdog (servidor antigo na malha) não mostra chip', () => {
    expect(conductorChip(undefined)).toBeNull();
  });
  it('vivo pelo batimento leva a última volta e quem vigia', () => {
    expect(conductorChip(wd({ alive: true, source: 'heartbeat', last_cycle: '2026-09-25T10:00:00-03:00', watching: ['rev1', 'arb'] })))
      .toEqual({ kind: 'alive', lastCycle: Date.parse('2026-09-25T10:00:00-03:00') / 1000, watching: ['rev1', 'arb'] });
  });
  it('vivo só pelo systemd fica sem última volta', () => {
    expect(conductorChip(wd({ alive: true, source: 'systemd' }))).toEqual({ kind: 'alive', lastCycle: null, watching: [] });
  });
  it('batimento velho é parado desde a última volta', () => {
    expect(conductorChip(wd({ source: 'heartbeat', last_cycle: '2026-09-25T10:00:00-03:00' })))
      .toEqual({ kind: 'stopped', lastCycle: Date.parse('2026-09-25T10:00:00-03:00') / 1000, alarm: true });
  });
  it('execução terminada: parado é o esperado, sem tom de alarme', () => {
    expect(conductorChip(wd({ source: 'heartbeat', last_cycle: '2026-09-25T10:00:00-03:00' }), true))
      .toEqual({ kind: 'stopped', lastCycle: Date.parse('2026-09-25T10:00:00-03:00') / 1000, alarm: false });
  });
  it('nada achado com systemd é "sem condutor"; sem systemd é indisponível', () => {
    expect(conductorChip(wd({ source: 'none' }))).toEqual({ kind: 'none' });
    expect(conductorChip(wd({ source: 'unavailable' }))).toEqual({ kind: 'unavailable' });
  });
});

describe('conductorChipLabel', () => {
  it('vivo sem sessão vigiada não termina num "vigiando" solto', () => {
    const label = conductorChipLabel({ kind: 'alive', lastCycle: Date.now() / 1000, watching: [] });
    expect(label).not.toMatch(/vigiando|watching/);
    expect(label).toMatch(/·/);
  });
  it('vivo com sessão vigiada diz quem', () => {
    expect(conductorChipLabel({ kind: 'alive', lastCycle: Date.now() / 1000, watching: ['rev1', 'arb'] })).toMatch(/rev1, arb$/);
  });
});

describe('filterFeed e feedTasks', () => {
  const feed = [item('a', 'alarm', 1), item('b', 'woke', 2), item('c', 'event', 1), item('d', 'notice', null)];
  it('filtra por tipo e por Task, juntos', () => {
    expect(filterFeed(feed, 'all', null).map((i) => i.id)).toEqual(['a', 'b', 'c', 'd']);
    expect(filterFeed(feed, 'alarm', null).map((i) => i.id)).toEqual(['a']);
    expect(filterFeed(feed, 'all', 1).map((i) => i.id)).toEqual(['a', 'c']);
    expect(filterFeed(feed, 'event', 2)).toEqual([]);
  });
  it('lista as Tasks do feed sem repetir, em ordem', () => {
    expect(feedTasks(feed)).toEqual([1, 2]);
  });
});

describe('fmtP', () => {
  it('duas casas; ausente ou não finito vira traço', () => {
    expect(fmtP(0.97)).toBe('0.97');
    expect(fmtP(null)).toBe('—');
    expect(fmtP(Number.NaN)).toBe('—');
  });
});
