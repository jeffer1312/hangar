// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
type AlertButton = { text: string; onPress?: () => void };
const mocks = vi.hoisted(() => ({ fetch: vi.fn(), alert: vi.fn() }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(), Alert: { alert: mocks.alert } }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, getNumber: () => undefined, getBoolean: () => undefined, set: () => {}, remove: () => {} }) }));
// Pacotes externos carregariam o react-native de verdade (Flow), fora dos aliases do vitest.
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('expo-blur', () => ({ BlurView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('expo-glass-effect', () => ({ GlassView: ({ children }: { children: React.ReactNode }) => children, isLiquidGlassAvailable: () => false }));
import { MachineDevices } from './MachineDevices';
import * as m from '../../paraglide/messages';

const server = { id: 'devices-owner', label: 'fixture-owner', baseUrl: 'https://owner.test', token: 'owner-secret', deviceId: 'self' };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
const now = () => Math.floor(Date.now() / 1000);
const local = [
  { device_id: 'old-app', name: 'fixture-old-phone', status: 'pending', created_at: now(), last_seen: null, install_code: 'ABC-DEF' },
  { device_id: 'tablet', name: 'fixture-tablet', status: 'approved', created_at: now(), last_seen: now() },
  { device_id: 'self', name: 'fixture-self', status: 'approved', created_at: now(), last_seen: now() },
  { device_id: 'gone', name: 'fixture-denied', status: 'denied', created_at: now(), last_seen: null },
  { device_id: 'from-account', name: 'fixture-account-phone', status: 'approved', created_at: now(), last_seen: now(), account_device: 'ad1' },
];
let rows: unknown[];
let root: Root;
let el: HTMLDivElement;

const button = (label: string) => [...el.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label);
async function click(label: string) {
  const target = button(label);
  expect(target, label).toBeTruthy();
  await act(async () => target!.click());
}
// O último alerta aberto: toca no botão de rótulo `label`.
async function answer(label: string) {
  const buttons = mocks.alert.mock.calls.at(-1)![2] as AlertButton[];
  await act(async () => buttons.find((b) => b.text === label)!.onPress!());
}
const writes = () => mocks.fetch.mock.calls.filter(([, init]) => init?.method && init.method !== 'GET')
  .map(([url, init]) => `${init.method} ${new URL(url).pathname}`);
const render = () => act(async () => root.render(<MachineDevices server={server} />));

beforeEach(() => {
  mocks.fetch.mockReset(); mocks.alert.mockReset();
  rows = local;
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', origin: null, onUnauthorized: vi.fn(), createEventSource: vi.fn() });
  vi.stubGlobal('fetch', mocks.fetch);
  mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
    const { pathname } = new URL(url);
    if (pathname === '/api/connect/devices' && (init?.method ?? 'GET') === 'GET') return json(rows);
    const id = /^\/api\/connect\/devices\/([^/]+)/.exec(pathname)?.[1];
    if (id) rows = (rows as { device_id: string }[]).filter((row) => row.device_id !== id || pathname.endsWith('/approve'))
      .map((row) => (row.device_id === id ? { ...row, status: 'approved' } : row));
    return json({ ok: true });
  });
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el);
});
afterEach(async () => { await act(async () => root.unmount()); el.remove(); vi.unstubAllGlobals(); });

describe('aparelhos desta máquina', () => {
  it('mostra só quem entrou por esta máquina, sem os da conta nem os recusados', async () => {
    let release!: () => void;
    mocks.fetch.mockImplementationOnce(() => new Promise<Response>((done) => { release = () => done(json(rows)); }));
    await render();
    expect(el.textContent).toContain(m.comum_carregando());
    await act(async () => release());
    await vi.waitFor(() => expect(el.textContent).toContain('fixture-old-phone'));
    expect(el.textContent).not.toContain(m.comum_carregando());
    expect(el.textContent).toContain(m.machine_devices_pending());
    expect(el.textContent).toContain('ABC-DEF');
    expect(el.textContent).toContain('fixture-tablet');
    expect(el.textContent).toContain(m.connect_device_this());
    expect(el.textContent).not.toContain('fixture-account-phone');
    expect(el.textContent).not.toContain('fixture-denied');
    // O próprio aparelho não se tira daqui.
    expect(button('fixture-self')).toBeUndefined();
    expect(new URL(mocks.fetch.mock.calls[0][0]).origin).toBe(server.baseUrl);
  });

  it('aprova e recusa o pedido pendente e tira o acesso do aprovado, relendo a lista', async () => {
    await render();
    await vi.waitFor(() => expect(button('fixture-old-phone')).toBeTruthy());
    await click('fixture-old-phone');
    await answer(m.connect_device_approve());
    await vi.waitFor(() => expect(el.textContent).not.toContain(m.machine_devices_pending()));
    expect(writes()).toEqual(['POST /api/connect/devices/old-app/approve']);

    await click('fixture-tablet');
    expect(mocks.alert.mock.calls.at(-1)![0]).toBe(m.connect_device_revoke_title({ name: 'fixture-tablet' }));
    await answer(m.connect_device_revoke());
    await vi.waitFor(() => expect(el.textContent).not.toContain('fixture-tablet'));
    expect(writes().at(-1)).toBe('DELETE /api/connect/devices/tablet');

    rows = [{ device_id: 'new', name: 'fixture-new', status: 'pending', created_at: now(), last_seen: null }];
    await act(async () => root.unmount());
    root = createRoot(el);
    await render();
    await vi.waitFor(() => expect(button('fixture-new')).toBeTruthy());
    await click('fixture-new');
    await answer(m.connect_device_deny());
    await vi.waitFor(() => expect(el.textContent).toContain(m.machine_devices_empty()));
    expect(writes().at(-1)).toBe('DELETE /api/connect/devices/new');
  });

  it('sem aparelho local diz que está vazio', async () => {
    rows = [local[4]];
    await render();
    await vi.waitFor(() => expect(el.textContent).toContain(m.machine_devices_empty()));
  });

  it('leitura que falha mostra o erro e tenta de novo pelo botão', async () => {
    mocks.fetch.mockResolvedValueOnce(json({ detail: 'fixture-down' }, 503));
    await render();
    await vi.waitFor(() => expect(button(m.mobile_machines_retry())).toBeTruthy());
    expect(el.textContent).toContain('503');
    expect(el.textContent).not.toContain(m.machine_devices_empty());
    await click(m.mobile_machines_retry());
    await vi.waitFor(() => expect(el.textContent).toContain('fixture-old-phone'));
    expect(button(m.mobile_machines_retry())).toBeUndefined();
  });

  it('separa a chave dada a outro computador dos apps e diz quando nunca foi usada', async () => {
    rows = [
      { device_id: 'app-used', name: 'fixture-app-used', status: 'approved', created_at: now(), last_seen: now() },
      { device_id: 'app-new', name: 'fixture-app-new', status: 'approved', created_at: now(), last_seen: null },
      { device_id: 'peer-b', name: 'fixture-pc-b', status: 'approved', created_at: now(), last_seen: null, peer: 'pc-b' },
    ];
    await render();
    await vi.waitFor(() => expect(button('fixture-pc-b')).toBeTruthy());
    const text = el.textContent ?? '';
    const apps = text.indexOf(m.connect_local_apps());
    const peers = text.indexOf(m.connect_local_peers());
    expect(apps).toBeGreaterThanOrEqual(0);
    expect(peers).toBeGreaterThan(apps);
    expect(text.indexOf('fixture-app-used')).toBeGreaterThan(apps);
    expect(text.indexOf('fixture-app-new')).toBeLessThan(peers);
    expect(text.indexOf('fixture-pc-b')).toBeGreaterThan(peers);
    expect(text).toContain(m.connect_local_peer_caption());
    expect(text).toContain(m.connect_local_never_used());

    await click('fixture-pc-b');
    expect(mocks.alert.mock.calls.at(-1)![0]).toBe(m.connect_device_revoke_title({ name: 'fixture-pc-b' }));
    await answer(m.connect_device_revoke());
    await vi.waitFor(() => expect(el.textContent).not.toContain('fixture-pc-b'));
    expect(writes().at(-1)).toBe('DELETE /api/connect/devices/peer-b');
    expect(el.textContent).not.toContain(m.connect_local_peers());
  });

  it('servidor antigo sem `account_device` mostra todos os aparelhos', async () => {
    rows = local.map(({ account_device: _, ...row }) => row);
    await render();
    await vi.waitFor(() => expect(el.textContent).toContain('fixture-account-phone'));
  });
});
