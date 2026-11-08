// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, type Server } from '@hangar/core';

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

const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200 });
const machine = (id: string): Server => ({ id, label: id, baseUrl: `https://${id}.acme.hangar.dev.br`, token: 't', credential: 'device', deviceId: `dev-${id}` });
let root: Root;
let el: HTMLDivElement;
let machinesFail = true;
let pc2: Promise<void>;
let releasePc2: () => void;
// Pedidos esperando por máquina (primeiro pedaço do host); a recusa falha quando `denyFails`.
let deviceRows: Record<string, unknown[]> = {};
let denyFails = false;
// Relógio fixo: o cartão calcula "pediu há" a partir dele; o pedido chegou dois minutos antes.
const CREATED_AT = 1_794_000_000;
const NOW = new Date((CREATED_AT + 120) * 1000);
const pending = (device_id: string, install_group: string, install_code: string, name = 'iPhone') =>
  ({ device_id, name, status: 'pending', install_group, install_code, created_at: CREATED_AT });

const button = (label: string) => [...el.querySelectorAll('[role="button"]')].find((b) => b.getAttribute('aria-label') === label) as HTMLElement | undefined;

beforeEach(async () => {
  mocks.vault.clear();
  mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [machine('pc1'), machine('pc2')], activeId: 'pc1' }));
  await useServers.getState().load();
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized: vi.fn(), origin: null, createEventSource: () => { throw new Error('sem SSE'); } });
  useConnect.setState({ ...useConnect.getInitialState(), ready: true, account: { name: 'acme', token: 'account-token', serviceUrl: CONNECT_SERVICE_URL } });
  // Só o Date: as esperas e os pedidos do teste seguem no relógio de verdade.
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(NOW);
  machinesFail = true;
  deviceRows = {};
  denyFails = false;
  pc2 = new Promise<void>((resolve) => { releasePc2 = resolve; });
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const url = new URL(input);
    if (url.origin === CONNECT_SERVICE_URL && url.pathname === '/v1/machines') {
      if (machinesFail) throw new TypeError('network');
      return json([]);
    }
    const host = url.hostname.split('.')[0];
    if (init?.method === 'DELETE' && url.pathname.startsWith('/api/connect/devices/')) {
      // O pedido some da lista de qualquer jeito (expirou ou foi respondido em outro lugar).
      deviceRows[host] = [];
      return denyFails ? new Response(JSON.stringify({ detail: 'fora do ar' }), { status: 503 }) : json({ ok: true });
    }
    // pc2 só responde no fim do teste: até lá a lista dele segue a caminho.
    if (host === 'pc2') return pc2.then(() => json(deviceRows.pc2 ?? []));
    if (url.pathname === '/api/connect/devices') return json(deviceRows[host] ?? []);
    return json({});
  }));
  el = document.createElement('div');
  document.body.appendChild(el);
  root = createRoot(el);
});
afterEach(async () => {
  // A leitura de aparelhos em voo é do store, que sobrevive ao teste: termina antes do próximo.
  releasePc2();
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

it('com a lista de aparelhos de uma máquina a caminho, mostra carregando e não "nenhum aparelho"', async () => {
  await render();
  await vi.waitFor(() => expect(useConnect.getState().devices.pc1).toBeDefined());
  await act(async () => {});
  expect(el.textContent).toContain(m.comum_carregando());
  expect(el.textContent).not.toContain(m.connect_devices_empty());
});

const buttons = (label: string) => [...el.querySelectorAll('[role="button"], [role="switch"]')].filter((b) => b.getAttribute('aria-label') === label) as HTMLElement[];

it('dois celulares com o mesmo código travam os dois cartões', async () => {
  releasePc2();
  deviceRows = { pc1: [pending('a', 'g1', 'ABC-DEF'), pending('b', 'g2', 'ABC-DEF', 'Pixel')] };
  await render();
  await vi.waitFor(() => expect(buttons(m.connect_group_approve())).toHaveLength(2));
  expect(el.textContent?.split(m.connect_group_code_clash()).length).toBe(3);
  expect(buttons(m.connect_group_approve()).map((b) => (b as HTMLButtonElement).disabled)).toEqual([true, true]);
});

it('cada máquina do cartão anuncia se está marcada, e o toque troca', async () => {
  releasePc2();
  deviceRows = { pc1: [pending('a', 'g1', 'ABC-DEF')], pc2: [pending('b', 'g1', 'ABC-DEF')] };
  await render();
  await vi.waitFor(() => expect(buttons('pc1')).toHaveLength(1));
  expect(buttons('pc1')[0].getAttribute('aria-checked')).toBe('true');
  await act(async () => buttons('pc1')[0].click());
  expect(buttons('pc1')[0].getAttribute('aria-checked')).toBe('false');
});

it('o celular pede de novo com o alerta aberto: o "sim" não aprova o pedido antigo e avisa', async () => {
  releasePc2();
  deviceRows = { pc1: [pending('a', 'g1', 'ABC-DEF')] };
  let yes: (() => void) | undefined;
  const { Alert } = await import('react-native');
  const alert = vi.spyOn(Alert, 'alert').mockImplementation((_t, _b, buttons) => {
    yes = (buttons as { onPress?: () => void }[] | undefined)?.[1]?.onPress;
  });
  await render();
  await vi.waitFor(() => expect(buttons(m.connect_group_approve())).toHaveLength(1));
  await act(async () => buttons(m.connect_group_approve())[0].click());
  deviceRows = { pc1: [pending('a2', 'g1', 'ABC-DEF')] };
  await act(async () => { await useConnect.getState().refreshDevices(); });
  await act(async () => yes!());
  const approvals = vi.mocked(fetch).mock.calls.filter(([url]) => String(url).endsWith('/approve'));
  expect(approvals).toHaveLength(0);
  expect(el.textContent).toContain(m.connect_group_unavailable());
  alert.mockRestore();
});

it('o erro da resposta fica fora do cartão, mesmo depois de o cartão sumir, até dispensar', async () => {
  releasePc2();
  denyFails = true;
  deviceRows = { pc1: [pending('a', 'g1', 'ABC-DEF')] };
  await render();
  await vi.waitFor(() => expect(buttons(m.connect_group_deny())).toHaveLength(1));
  await act(async () => buttons(m.connect_group_deny())[0].click());
  await vi.waitFor(() => expect(buttons(m.connect_group_deny())).toHaveLength(0));
  expect(el.textContent).toContain('pc1');
  const dismiss = buttons(m.connect_notice_dismiss());
  expect(dismiss).toHaveLength(1);
  await act(async () => dismiss[0].click());
  expect(buttons(m.connect_notice_dismiss())).toHaveLength(0);
});
