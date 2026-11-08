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

const button = (label: string) => [...el.querySelectorAll('[role="button"]')].find((b) => b.getAttribute('aria-label') === label) as HTMLElement | undefined;

beforeEach(async () => {
  mocks.vault.clear();
  mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [machine('pc1'), machine('pc2')], activeId: 'pc1' }));
  await useServers.getState().load();
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized: vi.fn(), origin: null, createEventSource: () => { throw new Error('sem SSE'); } });
  useConnect.setState({ ...useConnect.getInitialState(), ready: true, account: { name: 'acme', token: 'account-token', serviceUrl: CONNECT_SERVICE_URL } });
  machinesFail = true;
  pc2 = new Promise<void>((resolve) => { releasePc2 = resolve; });
  vi.stubGlobal('fetch', vi.fn(async (input: string) => {
    const url = new URL(input);
    if (url.origin === CONNECT_SERVICE_URL && url.pathname === '/v1/machines') {
      if (machinesFail) throw new TypeError('network');
      return json([]);
    }
    // pc2 só responde no fim do teste: até lá a lista dele segue a caminho.
    if (url.hostname.startsWith('pc2')) return pc2.then(() => json([]));
    if (url.pathname === '/api/connect/devices') return json([]);
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
