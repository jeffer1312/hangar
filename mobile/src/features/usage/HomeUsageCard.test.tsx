// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureLocale, localeAtual, type CostReport, type Server } from '@hangar/core';
import { HomeUsageCard } from './HomeUsageCard';
import { useServers } from '../../stores/servers';
import { useUsagePreferences } from './ReportUI';
import * as m from '../../paraglide/messages';
import { getLocale, overwriteGetLocale } from '../../paraglide/runtime';

const net = vi.hoisted(() => ({ costs: vi.fn() }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(), fetchCostsForServer: net.costs }));
vi.mock('expo-secure-store', () => ({ getItemAsync: vi.fn(async () => null), setItemAsync: vi.fn(), deleteItemAsync: vi.fn() }));
vi.mock('expo-router', () => ({ Stack: {}, useRouter: () => ({}) }));
vi.mock('expo-blur', () => ({ BlurView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('expo-glass-effect', () => ({ GlassView: ({ children }: { children: React.ReactNode }) => children, isLiquidGlassAvailable: () => false }));
vi.mock('expo-router/react-navigation', () => ({ useHeaderHeight: () => 0 }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: () => null }));
vi.mock('../../ui/Screen', () => ({ Screen: ({ children }: { children: React.ReactNode }) => children }));
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const a: Server = { id: 'home-a', label: 'Machine A', baseUrl: 'https://a.test', token: 'token' };
const b: Server = { ...a, id: 'home-b', label: 'Machine B' };
const bucket = { sessions: 1, input: 100, output: 0, cache_write: 0, cache_read: 0, cost: 2, cost_input: 2, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0 };
const report = (period = 'all', cost = 2): Partial<CostReport> => ({ applied: { period }, totals: { ...bucket, key: 'totals', cost }, by_model: [{ ...bucket, key: 'shared-model', cost }], by_day: [{ ...bucket, key: '2026-10-01', cost }] });
const deferred = <T,>() => { let resolve!: (value: T) => void; const promise = new Promise<T>((r) => { resolve = r; }); return { promise, resolve }; };
let root: Root;
let node: HTMLDivElement;
let clockEpoch = 0;
const previousLocale = localeAtual();
const previousMessages = getLocale();
const render = async () => { await act(async () => root.render(React.createElement(HomeUsageCard))); };
const metric = (label: string) => node.querySelector(`[aria-label^="${label}:"]`)?.getAttribute('aria-label');
const click = async (label: string) => { const button = [...node.querySelectorAll('button')].find((item) => item.textContent === label)!; expect(button).toBeTruthy(); await act(async () => button.click()); };
beforeEach(() => {
  vi.useFakeTimers(); vi.setSystemTime(Date.parse('2026-10-22T12:00:00Z') + clockEpoch++ * 60_001);
  net.costs.mockReset().mockImplementation((_server: Server, period: string) => Promise.resolve(report(period)));
  configureLocale({ getLocale: () => 'pt' }); overwriteGetLocale(() => 'pt');
  useServers.setState({ servers: [a, b, { ...a, id: 'guest', invite: true }, { ...a, id: 'off', disabled: true }, { ...a, id: 'temporary', ephemeral: true }], activeId: a.id });
  useUsagePreferences.setState({ period: '1d', off: [b.id] });
  node = document.createElement('div'); root = createRoot(node);
});
afterEach(async () => { await act(async () => root.unmount()); vi.useRealTimers(); configureLocale({ getLocale: () => previousLocale }); overwriteGetLocale(() => previousMessages); });

it('Home soma máquinas próprias e dias e modelos únicos sem herdar destino ou filtros de Custos', async () => {
  await render();
  expect(metric(m.home_usage_cost())).toBe(`${m.home_usage_cost()}: US$ 4,00`);
  expect(metric(m.home_usage_sessions())).toBe(`${m.home_usage_sessions()}: 2`);
  expect(metric(m.home_usage_active_days())).toBe(`${m.home_usage_active_days()}: 1`);
  expect(metric(m.home_usage_model_count())).toBe(`${m.home_usage_model_count()}: 1`);
  expect(net.costs.mock.calls).toEqual([[a, 'all', false, true], [b, 'all', false, true]]);
  await act(async () => useServers.setState({ activeId: b.id }));
  expect(metric(m.home_usage_cost())).toContain('4,00');
  expect(net.costs).toHaveBeenCalledTimes(2);
});

it('Home conserva cache ao remontar e invalida credencial, endereço e conjunto', async () => {
  await render();
  await act(async () => root.render(null));
  await render();
  expect(metric(m.home_usage_cost())).toContain('4,00');
  expect(net.costs).toHaveBeenCalledTimes(2);
  const changed = { ...a, token: 'new-token', baseUrl: 'https://new.test' };
  await act(async () => useServers.setState({ servers: [changed, b] }));
  expect(net.costs).toHaveBeenCalledWith(changed, 'all', false, true);
  await act(async () => useServers.setState({ servers: [changed] }));
  expect(metric(m.home_usage_cost())).toContain('2,00');
  await act(async () => root.render(null));
  await act(async () => vi.advanceTimersByTimeAsync(60_001));
  net.costs.mockClear();
  await render();
  expect(net.costs).toHaveBeenCalledTimes(1);
});

it('Home identifica parcial e falha total sem transformá-las em zero', async () => {
  await act(async () => useServers.setState({ servers: [{ ...a, token: 'partial' }, b] }));
  net.costs.mockImplementation((s: Server, period: string) => s.id === b.id ? Promise.reject(new Error('offline')) : Promise.resolve(report(period, 5)));
  await render();
  expect(metric(m.home_usage_cost())).toContain('5,00');
  expect(node.textContent).toContain(m.custos_total_parcial());
  expect(node.textContent).toContain('Machine B:');
  net.costs.mockRejectedValue(new Error('offline'));
  await click(m.sync_retry());
  expect(metric(m.home_usage_cost())).toBeUndefined();
  expect(node.textContent).toContain('Machine A:');
  expect(node.textContent).not.toContain('US$ 0,00');
});

it('Home descarta resposta atrasada por período e não soma período incompatível', async () => {
  const old = deferred<Partial<CostReport>>();
  await act(async () => useServers.setState({ servers: [{ ...a, token: 'late' }, b] }));
  net.costs.mockImplementation((s: Server, period: string) => period === 'all' ? old.promise : Promise.resolve(report(s.id === b.id ? 'all' : period, 7)));
  await render();
  await click(m.home_usage_7d());
  await act(async () => old.resolve(report('all', 99)));
  expect(metric(m.home_usage_cost())).toContain('7,00');
  expect(node.textContent).toContain(`Machine B: ${m.home_usage_period_unsupported()}`);
  await click(m.home_usage_all());
});
