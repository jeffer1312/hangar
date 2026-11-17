// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { EventSourceLike, Server, StatusSnapshot } from '@hangar/core';
import * as m from '../../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const FIXTURE = JSON.parse(readFileSync(join(__dirname, '../../../../crates/hangar-api/tests/fixtures/status.json'), 'utf8')) as StatusSnapshot;
const a: Server = { id: 'a', label: 'Casa', baseUrl: 'https://a.test', token: 'ta' };

type Bucket = { server: Server; error: string | null; offlineSince?: number };
const calls = vi.hoisted(() => ({
  getStatus: vi.fn(), openStream: vi.fn(), parked: vi.fn(), reconnect: vi.fn(),
  servers: [] as Server[], activeId: null as string | null, byServer: [] as Bucket[],
}));

vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  getStatus: calls.getStatus,
  openStatusStream: calls.openStream,
  isServerParked: calls.parked,
  registrarDiag: () => {},
}));
vi.mock('../../stores/servers', () => ({
  useServers: (selector: (s: { servers: Server[]; activeId: string | null }) => unknown) => selector({ servers: calls.servers, activeId: calls.activeId }),
}));
vi.mock('../../stores/sessions', () => ({
  useSessions: Object.assign((selector: (s: { byServer: Bucket[] }) => unknown) => selector({ byServer: calls.byServer }),
    { getState: () => ({ retain: () => () => {}, reconnect: calls.reconnect }) }),
}));
vi.mock('expo-router', async () => {
  const { useEffect } = await import('react');
  return { Stack: { Screen: () => null }, useFocusEffect: (cb: () => void | (() => void)) => { useEffect(() => cb(), [cb]); } };
});
vi.mock('expo-router/react-navigation', () => ({ useHeaderHeight: () => 0 }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn(async () => true) }));
vi.mock('../config/Pagina', () => ({ Pagina: ({ children }: { children: ReactNode }) => createElement('main', null, children) }));
vi.mock('../config/InfoNotice', () => ({ InfoNotice: ({ text }: { text: string }) => createElement('p', null, text) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: () => {}, erro: () => {} } }));
vi.mock('../../ui/haptics', () => ({ haptic: { selection: () => {}, success: () => {}, error: () => {} } }));
vi.mock('../usage/CostsVisuals', () => ({
  Panel: ({ title, children }: { title: string; children: ReactNode }) => createElement('section', { 'aria-label': title }, createElement('h3', null, title), children),
  GrowFill: () => null,
  Segmented: ({ options, onChange }: { options: { key: string; label: string }[]; onChange: (k: string) => void }) =>
    createElement('div', null, options.map((o) => createElement('button', { key: o.key, 'aria-label': o.label, onClick: () => onChange(o.key) }, o.label))),
}));
vi.mock('../usage/ReportUI', () => ({
  useChartColors: () => ['#1', '#2', '#3', '#4', '#5', '#6', '#7'],
  Composition: ({ label, items }: { label: string; items: { label: string }[] }) => createElement('div', { 'data-composition': label }, items.map((i) => i.label).join('|')),
}));

import StatusScreen from './StatusScreen';

function fakeSource() {
  const listeners = new Map<string, (ev: { data: string }) => void>();
  const source: EventSourceLike & { emit: (type: string, data: string) => void } = {
    addEventListener: (type, fn) => { listeners.set(type, fn); },
    removeEventListener: (type) => { listeners.delete(type); },
    close: vi.fn(),
    onerror: null,
    onopen: null,
    readyState: 1,
    emit: (type, data) => listeners.get(type)?.({ data }),
  };
  return source;
}

const roots = new Set<Root>();
async function mount() {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.add(root);
  await act(async () => root.render(createElement(StatusScreen)));
  return container;
}
const button = (c: HTMLElement, label: string) => c.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);

beforeEach(() => {
  vi.resetAllMocks();
  calls.servers = [a];
  calls.activeId = 'a';
  calls.byServer = [{ server: a, error: null }];
  calls.parked.mockReturnValue(false);
});
afterEach(async () => {
  for (const root of roots) await act(async () => root.unmount());
  roots.clear();
});

describe('tela Status', () => {
  it('sucesso: desenha os blocos da fixture, a lista de atenção e segue o SSE', async () => {
    const source = fakeSource();
    calls.getStatus.mockResolvedValue(FIXTURE);
    calls.openStream.mockReturnValue(source);
    const c = await mount();
    const text = c.textContent ?? '';
    expect(calls.getStatus).toHaveBeenCalledWith(a);
    expect(calls.openStream).toHaveBeenCalledTimes(1);
    expect(text).toContain(m.status_health_attention({ count: '4' }));
    expect(button(c, m.status_attention_disk_full({ pct: '76' }))).not.toBeNull();
    expect(button(c, m.status_attention_machine_outdated({ name: 'Casa', version: '0.1.0.10790' }))).not.toBeNull();
    expect(text).toContain(m.status_days_title());
    expect(text).toContain('DELPHI-02');
    expect(text).toContain(m.status_machines_outdated());
    expect(text).toContain(m.status_sessions_dead_one({ name: 'hangar-3' }));
    expect(text).toContain('GET /api/costs');
    expect(text).toContain(m.status_host_title());
    expect(text).toContain(m.status_leftover_tmux({ count: '2', names: 'cx-antigo, teste-voz' }));
    expect(text).toContain(m.status_update_available({ version: '0.1.0.10833' }));

    // Provedor e processo: a mesma barra troca de divisão.
    expect(c.querySelector('[data-composition]')?.textContent).toContain('Codex');
    await act(async () => button(c, m.status_sessions_by_process())!.click());
    expect(c.querySelector('[data-composition]')?.textContent).toContain(m.status_process_codex_app_server());

    await act(async () => source.emit('status', JSON.stringify({ ...FIXTURE, attention: [] })));
    expect(c.textContent).toContain(m.status_health_ok());
  });

  it('vazio: sem pontos de atenção, sem sessões e só esta máquina', async () => {
    calls.getStatus.mockResolvedValue({
      ...FIXTURE,
      attention: [],
      exits: { items: [] },
      machines: FIXTURE.machines!.filter((x) => x.this_machine),
      sessions: { open: 0, by_provider: [], by_process: [], dead: [], asleep: 0, queued: 0 },
    } satisfies StatusSnapshot);
    calls.openStream.mockReturnValue(fakeSource());
    const text = (await mount()).textContent ?? '';
    expect(text).toContain(m.status_health_ok());
    expect(text).toContain(m.status_days_none());
    expect(text).toContain(m.status_machines_only_this());
    expect(text).toContain(m.status_sessions_none());
  });

  it('sem máquina cadastrada: o aviso de vazio e nenhum pedido', async () => {
    calls.servers = [];
    calls.activeId = null;
    calls.byServer = [];
    expect((await mount()).textContent).toContain(m.maquinas_vazio());
    expect(calls.getStatus).not.toHaveBeenCalled();
  });

  it('erro: diz o código e tenta de novo; sem SSE enquanto não leu', async () => {
    calls.getStatus.mockRejectedValue(Object.assign(new Error('500: boom'), { status: 500 }));
    const c = await mount();
    expect(c.textContent).toContain(m.status_error_title());
    expect(c.textContent).toContain(m.status_error_http({ server: 'Casa', code: '500' }));
    expect(calls.openStream).not.toHaveBeenCalled();
    calls.getStatus.mockResolvedValue(FIXTURE);
    calls.openStream.mockReturnValue(fakeSource());
    await act(async () => button(c, m.status_retry())!.click());
    expect(calls.getStatus).toHaveBeenCalledTimes(2);
    expect(c.textContent).toContain(m.status_host_title());
  });

  it('parado: só informa e manda abrir o computador; nada sai do aparelho', async () => {
    calls.byServer = [{ server: a, error: 'offline', offlineSince: Date.now() - 120_000 }];
    const c = await mount();
    expect(c.textContent).toContain(m.status_down_title({ server: 'Casa' }));
    expect(c.textContent).toContain(m.status_down_since({ time: m.status_duration_minutes({ n: '2' }) }));
    expect(calls.getStatus).not.toHaveBeenCalled();
    expect(calls.openStream).not.toHaveBeenCalled();
    expect([...c.querySelectorAll('button')].map((b) => b.getAttribute('aria-label'))).toEqual([m.status_retry()]);
    await act(async () => button(c, m.status_retry())!.click());
    expect(calls.reconnect).toHaveBeenCalledOnce();
  });

  it('servidor estacionado: nenhum SSE nem leitura', async () => {
    calls.parked.mockReturnValue(true);
    const c = await mount();
    expect(c.textContent).toContain(m.status_down_title({ server: 'Casa' }));
    expect(calls.getStatus).not.toHaveBeenCalled();
    expect(calls.openStream).not.toHaveBeenCalled();
  });
});
