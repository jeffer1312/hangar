// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, installCode, type ConnectMachine, type Server } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ vault: new Map<string, string>() }));
vi.mock('expo-secure-store', () => ({
  getItemAsync: async (key: string) => mocks.vault.get(key) ?? null,
  setItemAsync: async (key: string, value: string) => { mocks.vault.set(key, value); },
  deleteItemAsync: async (key: string) => { mocks.vault.delete(key); },
}));
vi.mock('expo-constants', () => ({ default: { deviceName: 'fixture-phone' } }));
vi.mock('expo-router', () => ({ useFocusEffect: () => {}, useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('./ConnectHero', () => ({ ConnectMark: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('expo-blur', () => ({ BlurView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('expo-glass-effect', () => ({ GlassView: ({ children }: { children: React.ReactNode }) => children, isLiquidGlassAvailable: () => false }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));

import { useServers } from '../../stores/servers';
import { CONNECT_SERVICE_URL, useConnect } from './connectStore';
import { ConnectMachines } from './ConnectMachines';
import * as m from '../../paraglide/messages';

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const machine = (id: string): Server => ({ id, label: id, baseUrl: `https://${id}.acme.hangar.dev.br`, token: 't', credential: 'device', deviceId: `dev-${id}` });
const pc3: ConnectMachine = { machine_id: 'pc3', name: 'pc3', host: 'pc3.acme.hangar.dev.br', online: true };
let root: Root;
let el: HTMLDivElement;
let machinesFail = true;
let machineList: ConnectMachine[] = [];
// Status deste aparelho (`ad1`) na conta e a lista de aparelhos dela.
let deviceStatus = 'pending';
let accountRows: unknown[] = [];
// Relógio fixo: a linha calcula "pediu há" a partir dele; o pedido chegou dois minutos antes.
const CREATED_AT = 1_794_000_000;
const NOW = new Date((CREATED_AT + 120) * 1000);
const row = (device_id: string, name: string, status: string) => ({ device_id, name, status, created_at: CREATED_AT, approved_at: null, revoked_at: null });

const button = (label: string) => [...el.querySelectorAll('[role="button"]')].find((b) => b.getAttribute('aria-label') === label) as HTMLButtonElement | undefined;
const checkRow = (label: string) => {
  const found = el.querySelector(`[role="checkbox"][aria-label="${label}"]`) as HTMLButtonElement | null;
  expect(found, label).toBeTruthy();
  return found!;
};
const serviceCalls = () => vi.mocked(fetch).mock.calls.map(([input, init]) => ({ url: new URL(String(input)), method: init?.method ?? 'GET',
  device: new Headers(init?.headers).get('X-Hangar-Device') }));

beforeEach(async () => {
  mocks.vault.clear();
  mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [machine('pc1'), machine('pc2')], activeId: 'pc1' }));
  mocks.vault.set('cp_connect_device_v1', JSON.stringify({ name: 'acme', serviceUrl: CONNECT_SERVICE_URL, id: 'ad1', secret: 'ad-secret' }));
  await useServers.getState().load();
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized: vi.fn(), origin: null, createEventSource: () => { throw new Error('sem SSE'); } });
  useConnect.setState({ ...useConnect.getInitialState(), ready: true, account: { name: 'acme', token: 'account-token', serviceUrl: CONNECT_SERVICE_URL } });
  // Só o Date: as esperas e os pedidos do teste seguem no relógio de verdade.
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(NOW);
  machinesFail = true;
  machineList = [];
  deviceStatus = 'pending';
  accountRows = [];
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const url = new URL(input);
    if (url.origin !== CONNECT_SERVICE_URL) return json({});
    if (url.pathname === '/v1/machines') {
      if (machinesFail) throw new TypeError('network');
      return json(machineList);
    }
    if (url.pathname === '/v1/devices') return json(accountRows);
    if (url.pathname === '/v1/devices/ad1') return json({ status: deviceStatus });
    if (init?.method === 'DELETE') return new Response(null, { status: 204 });
    return json({ status: 'approved' });
  }));
  el = document.createElement('div');
  document.body.appendChild(el);
  root = createRoot(el);
});
afterEach(async () => {
  // A leitura do aparelho em voo é do store, que sobrevive ao teste: termina antes do próximo.
  await act(async () => { await useConnect.getState().refreshDevices(); });
  await act(async () => root.unmount());
  el.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const render = () => act(async () => root.render(<ConnectMachines onDone={() => {}} />));

it('lista de máquinas com erro mostra "Tentar de novo", que relê a lista', async () => {
  await render();
  const retry = button(m.mobile_machines_retry());
  expect(retry, 'tentar de novo à mostra').toBeTruthy();
  machinesFail = false;
  await act(async () => retry!.click());
  await vi.waitFor(() => expect(button(m.mobile_machines_retry())).toBeUndefined());
  expect(el.textContent).toContain(m.connect_machines_empty());
});

it('pendente mostra "Liberar este celular" com o código deste aparelho e não deixa pedir entrada', async () => {
  machinesFail = false;
  machineList = [pc3];
  mocks.vault.set('cp_connect_install_id_v1', 'a'.repeat(32));
  await render();
  await vi.waitFor(() => expect(el.textContent).toContain(m.connect_release_title()));
  expect(el.textContent).toContain(m.connect_release_waiting());
  await vi.waitFor(() => expect(el.textContent).toContain(installCode('a'.repeat(32))));
  const pc3Row = checkRow('pc3');
  expect(pc3Row.disabled || pc3Row.getAttribute('aria-disabled') === 'true').toBe(true);
  expect(button(m.login_guest_enter_one())).toBeUndefined();
  expect(el.textContent).not.toContain(m.connect_account_devices());
});

it('liberado, as ligadas vêm marcadas, a desligada não marca, e Entrar pede só as marcadas', async () => {
  deviceStatus = 'approved';
  machinesFail = false;
  const pc4: ConnectMachine = { machine_id: 'pc4', name: 'pc4', host: 'pc4.acme.hangar.dev.br', online: true };
  const pc5: ConnectMachine = { machine_id: 'pc5', name: 'pc5', host: 'pc5.acme.hangar.dev.br', online: false };
  machineList = [pc3, pc4, pc5];
  await render();
  await vi.waitFor(() => expect(button(m.login_guest_enter_many({ n: '2' }))).toBeTruthy());
  expect(checkRow('pc3').getAttribute('aria-checked')).toBe('true');
  expect(checkRow('pc4').getAttribute('aria-checked')).toBe('true');
  expect(checkRow('pc5').getAttribute('aria-checked')).toBe('false');
  expect(el.textContent).toContain(m.connect_machine_off_now());
  // Nada é pedido antes do toque.
  expect(serviceCalls().some((call) => /^\/v1\/machines\/\w+\/devices$/.test(call.url.pathname))).toBe(false);

  await act(async () => checkRow('pc4').click());
  const enter = button(m.login_guest_enter_one());
  expect(enter, 'o botão conta só as marcadas').toBeTruthy();
  await act(async () => enter!.click());
  await vi.waitFor(() => expect(serviceCalls().filter((call) => /^\/v1\/machines\/\w+\/devices$/.test(call.url.pathname))
    .map((call) => call.url.pathname)).toEqual(['/v1/machines/pc3/devices']));
});

it('o pendente da lista de aparelhos mostra o mesmo código do celular que pediu', async () => {
  deviceStatus = 'approved';
  const code = installCode('b'.repeat(32));
  accountRows = [row('ad1', 'iPhone', 'approved'), { ...row('ad2', 'Pixel', 'pending'), install_code: code }];
  await render();
  await vi.waitFor(() => expect(button('Pixel')).toBeTruthy());
  expect(el.textContent).toContain(m.connect_account_device_asked_code({ code, when: m.tempo_min_atras({ n: 2 }) }));
});

it('liberado lista os aparelhos da conta, e o pendente se libera pelo alerta, com a credencial deste aparelho', async () => {
  deviceStatus = 'approved';
  accountRows = [row('ad1', 'iPhone', 'approved'), row('ad2', 'Pixel', 'pending'), row('ad3', 'Moto', 'approved'), row('ad4', 'fixture-gone', 'revoked')];
  let release: (() => void) | undefined;
  const { Alert } = await import('react-native');
  const alert = vi.spyOn(Alert, 'alert').mockImplementation((_t, _b, buttons) => {
    release = (buttons as { onPress?: () => void }[] | undefined)?.[2]?.onPress;
  });
  await render();
  await vi.waitFor(() => expect(button('Pixel')).toBeTruthy());
  expect(el.textContent).toContain(m.connect_account_device_asked({ when: m.tempo_min_atras({ n: 2 }) }));
  expect(button('Moto')).toBeTruthy();
  expect(el.textContent).toContain('fixture-phone');
  expect(el.textContent).not.toContain('fixture-gone');

  await act(async () => button('Pixel')!.click());
  await act(async () => release!());

  await vi.waitFor(() => expect(serviceCalls().filter((call) => call.url.pathname === '/v1/devices/ad2/approve')
    .map((call) => `${call.method} ${call.device}`)).toEqual(['POST ad1']));
  alert.mockRestore();
});
