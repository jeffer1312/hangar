// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { intlLocale, type EventSourceLike, type Server, type StatusSnapshot } from '@hangar/core';
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
import { bytes } from './format';
import { dayStartOf } from './StatusBlocks';
import { STATUS_WATCHDOG_MS, keepUnchanged } from './useStatus';

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

    // Fatia grande (Claude, ~62%) leva o valor dentro; a pequena (Pi, ~5%) fica sem rótulo.
    const bar = () => c.querySelector<HTMLElement>(`[role="image"][aria-label^="${m.status_sessions_title()}:"]`)!;
    const providers = FIXTURE.sessions!.by_provider;
    expect(bar().getAttribute('aria-label')).toContain('Codex');
    expect(bar().children[0].textContent).toBe(bytes(providers[0].rss_bytes));
    expect(providers[2].key).toBe('pi');
    expect(bar().children[2].textContent).toBe('');
    // Provedor e processo: a mesma barra troca de divisão.
    await act(async () => button(c, m.status_sessions_by_process())!.click());
    expect(bar().getAttribute('aria-label')).toContain(m.status_process_codex_app_server());

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

  it('endereço que não provou a identidade: frase própria, não "HTTP 409"', async () => {
    calls.getStatus.mockRejectedValue(Object.assign(new Error(m.mobile_machines_identity_unconfirmed()), { status: 409, code: 'server_identity_unconfirmed' }));
    const c = await mount();
    expect(c.textContent).toContain(m.mobile_machines_identity_unconfirmed());
    expect(c.textContent).not.toContain(m.status_error_http({ server: 'Casa', code: '409' }));
  });

  it('marcadores do mesmo dia que se sobrepõem viram um alvo que abre a lista do dia', async () => {
    const day = new Date(); day.setHours(12, 0, 0, 0);
    const base = day.getTime() - 86_400_000;
    const crash = (ms: number, cause: string) => ({ ts: new Date(ms).toISOString(), kind: 'crash' as const, cause, error_lines: [] });
    calls.getStatus.mockResolvedValue({ ...FIXTURE, generated_at: new Date().toISOString(), exits: { items: [crash(base, 'out_of_memory'), crash(base + 600_000, 'watchdog')] } });
    calls.openStream.mockReturnValue(fakeSource());
    const c = await mount();
    const marks = [...c.querySelectorAll<HTMLButtonElement>('button[aria-label]')]
      .filter((b) => b.getAttribute('aria-label')!.startsWith(`${m.status_exit_crash()} · `));
    expect(marks).toHaveLength(1);
    expect(marks[0].textContent).toBe('2');
    await act(async () => marks[0].click());
    expect(c.textContent).toContain(m.status_exit_cause_out_of_memory());
    expect(c.textContent).toContain(m.status_exit_cause_watchdog());
  });

  it('a saída escolhida segue a mesma quando o retrato seguinte muda a lista', async () => {
    const day = new Date(); day.setHours(12, 0, 0, 0);
    const ago = (n: number) => new Date(day.getTime() - n * 86_400_000).toISOString();
    const oom = { ts: ago(1), kind: 'crash' as const, cause: 'out_of_memory', error_lines: [] };
    const upd = (n: number) => ({ ts: ago(n), kind: 'update' as const, from: 'a', to: 'b', error_lines: [] });
    const source = fakeSource();
    const now = new Date().toISOString();
    calls.getStatus.mockResolvedValue({ ...FIXTURE, generated_at: now, exits: { items: [upd(3), oom] } });
    calls.openStream.mockReturnValue(source);
    const c = await mount();
    const mark = c.querySelector<HTMLButtonElement>(`button[aria-label^="${m.status_exit_crash()} · "]`)!;
    await act(async () => mark.click());
    expect(c.textContent).toContain(m.status_exit_cause_out_of_memory());
    await act(async () => source.emit('status', JSON.stringify({ ...FIXTURE, generated_at: now, exits: { items: [upd(5), upd(3), oom] } })));
    expect(c.textContent).toContain(m.status_exit_cause_out_of_memory());
    expect(c.textContent).not.toContain(m.status_exit_versions({ from: 'a', to: 'b' }));
  });

  it('curva e colunas: controle ajustável que diz o valor, e o ponto escolhido segue o minuto', async () => {
    const source = fakeSource();
    calls.getStatus.mockResolvedValue(FIXTURE);
    calls.openStream.mockReturnValue(source);
    const c = await mount();
    const series = FIXTURE.server!.rss_series;
    const lastV = series[series.length - 1].v;
    const curve = () => c.querySelector<HTMLElement>(`[role="adjustable"][aria-label^="${m.status_server_memory()},"]`)!;
    expect(curve().getAttribute('aria-valuetext')).toBe(m.status_point_ago({ value: bytes(lastV), n: '0' }));
    await act(async () => curve().click());
    expect(c.textContent).toContain(m.status_point_ago({ value: bytes(lastV), n: '0' }));
    // Um minuto depois a série anda: o ponto escolhido é o mesmo, agora "há 1 min".
    const next = [...series.slice(1), { ts: '2026-11-17T14:24:00Z', v: lastV * 2 }];
    await act(async () => source.emit('status', JSON.stringify({ ...FIXTURE, server: { ...FIXTURE.server!, rss_series: next } })));
    expect(c.textContent).toContain(m.status_point_ago({ value: bytes(lastV), n: '1' }));

    const hours = FIXTURE.slow!.hours;
    const lastHour = hours[hours.length - 1];
    const columns = c.querySelector<HTMLElement>(`[role="adjustable"][aria-label="${m.status_slow_chart()}"]`)!;
    expect(columns.getAttribute('aria-valuetext')).toBe(m.status_slow_hour({
      hour: `${String(new Date(lastHour.hour_start).getHours()).padStart(2, '0')}h`, slow: String(lastHour.slow), errors: String(lastHour.errors) }));
  });

  it('a janela de 7 dias vira com o retrato do dia seguinte, mesmo com as saídas iguais', async () => {
    const label = (iso: string) => {
      const d = new Date(dayStartOf(iso));
      return `${d.toLocaleDateString(intlLocale(), { weekday: 'short' }).replace(/\.$/, '')} ${d.getDate()}`;
    };
    const source = fakeSource();
    calls.getStatus.mockResolvedValue(FIXTURE);
    calls.openStream.mockReturnValue(source);
    const c = await mount();
    const oldest = new Date(dayStartOf(FIXTURE.generated_at) - 6 * 86_400_000).toISOString();
    const tomorrow = new Date(Date.parse(FIXTURE.generated_at) + 86_400_000).toISOString();
    expect(c.textContent).toContain(label(oldest));
    expect(c.textContent).not.toContain(label(tomorrow));
    await act(async () => source.emit('status', JSON.stringify({ ...FIXTURE, generated_at: tomorrow })));
    expect(c.textContent).not.toContain(label(oldest));
    expect(c.textContent).toContain(label(tomorrow));
  });

  it('servidor recriado com os mesmos dados não reabre o SSE; token novo reabre', async () => {
    calls.getStatus.mockResolvedValue(FIXTURE);
    calls.openStream.mockImplementation(() => fakeSource());
    const root = createRoot(document.createElement('div'));
    roots.add(root);
    const render = () => act(async () => root.render(createElement(StatusScreen)));
    await render();
    expect(calls.openStream).toHaveBeenCalledTimes(1);
    calls.servers = [{ ...a }];
    await render();
    expect(calls.getStatus).toHaveBeenCalledTimes(1);
    expect(calls.openStream).toHaveBeenCalledTimes(1);
    const renewed = { ...a, token: 'tb' };
    calls.servers = [renewed];
    await render();
    expect(calls.openStream).toHaveBeenCalledTimes(2);
    expect(calls.openStream).toHaveBeenLastCalledWith(renewed);
  });

  it('retrato igual bloco a bloco mantém a referência do bloco', () => {
    const next: StatusSnapshot = { ...JSON.parse(JSON.stringify(FIXTURE)), generated_at: 'depois', sessions: { ...FIXTURE.sessions!, queued: 99 } };
    const out = keepUnchanged(FIXTURE, next);
    expect(out.host).toBe(FIXTURE.host);
    expect(out.attention).toBe(FIXTURE.attention);
    expect(out.sessions).toBe(next.sessions);
    expect(out.generated_at).toBe('depois');
  });

  describe('com relógio falso', () => {
    beforeEach(() => { vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] }); });
    afterEach(() => { vi.useRealTimers(); });
    const tick = (ms: number) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

    it('SSE mudo: o vigia fecha e relê; ping zera o vigia', async () => {
      const first = fakeSource();
      calls.getStatus.mockResolvedValue(FIXTURE);
      calls.openStream.mockReturnValueOnce(first).mockReturnValue(fakeSource());
      await mount();
      await tick(STATUS_WATCHDOG_MS - 1000);
      await act(async () => first.emit('ping', ''));
      await tick(STATUS_WATCHDOG_MS - 1000);
      expect(first.close).not.toHaveBeenCalled();
      await tick(1000);
      expect(first.close).toHaveBeenCalled();
      expect(calls.getStatus).toHaveBeenCalledTimes(1);
      await tick(1000);
      expect(calls.getStatus).toHaveBeenCalledTimes(2);
      expect(calls.openStream).toHaveBeenCalledTimes(2);
    });

    for (const how of ['ping', 'queda'] as const) {
      it(`estacionado com a tela aberta (${how}): fecha o SSE, mostra parado e não tenta de novo`, async () => {
        const source = fakeSource();
        calls.getStatus.mockResolvedValue(FIXTURE);
        calls.openStream.mockReturnValue(source);
        const c = await mount();
        calls.parked.mockReturnValue(true);
        await act(async () => { if (how === 'ping') source.emit('ping', ''); else source.onerror?.(new Event('error')); });
        expect(source.close).toHaveBeenCalled();
        expect(c.textContent).toContain(m.status_down_title({ server: 'Casa' }));
        await tick(60_000);
        expect(calls.getStatus).toHaveBeenCalledTimes(1);
        expect(calls.openStream).toHaveBeenCalledTimes(1);
      });
    }
  });
});
