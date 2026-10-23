// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ fetch: vi.fn(), alert: vi.fn(), vault: new Map<string, string>() }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async (k: string) => mocks.vault.get(k) ?? null, setItemAsync: async (k: string, v: string) => { mocks.vault.set(k, v); } }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(), Alert: { alert: mocks.alert },
  TextInput: (p: { accessibilityLabel?: string; value?: string; editable?: boolean; onChangeText?: (t: string) => void }) =>
    <input aria-label={p.accessibilityLabel} disabled={p.editable === false} value={p.value ?? ''} onChange={(e) => p.onChangeText?.(e.target.value)} />,
}));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
import { PeerSettings } from './PeerSettings';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';
const server = { id: 'peer-owner', label: 'fixture-owner', baseUrl: 'https://owner.test', token: 'owner-secret' };
const remote = { baseUrl: 'https://remote.test', token: 'remote-secret', label: 'fixture-remote' };
const peer = { id: 'remote', base_url: remote.baseUrl, token: '••••mask', enabled: true };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
let root: Root;
let el: HTMLDivElement;
const writes = () => mocks.fetch.mock.calls.filter(([, init]) => init?.method && init.method !== 'GET');
async function click(label: string) {
  let button = [...el.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label);
  if (!button && [m.mobile_machines_check(), m.mobile_machines_peer_disable(), m.lista_remover()].some((value) => value === label)) {
    await click(peer.id);
    button = [...el.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label);
  }
  expect(button, label).toBeTruthy(); await act(async () => button!.click());
}
async function type(label: string, value: string) {
  if (!el.querySelector(`input[aria-label="${label}"]`)) await click(m.mobile_machines_add_peer());
  const input = el.querySelector(`input[aria-label="${label}"]`) as HTMLInputElement;
  expect(input, label).toBeTruthy();
  await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value); input.dispatchEvent(new Event('input', { bubbles: true })); });
}
beforeEach(async () => {
  mocks.fetch.mockReset(); mocks.alert.mockReset(); mocks.vault.clear();
  mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [server], activeId: server.id })); await useServers.getState().load();
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', origin: null, onUnauthorized: vi.fn(), createEventSource: vi.fn() });
  vi.stubGlobal('fetch', mocks.fetch);
  mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
    if (url.includes('/api/peers/check?')) return json({ estado: 'ok', identificador: 'remote', motivo: '', tempo_ms: 9 });
    if (url.endsWith('/api/peers/identificador')) return json({ identificador: 'owner', lan_url: '' });
    if (init?.method === 'DELETE') return json([]);
    if (url.endsWith('/enabled')) return json([{ ...peer, enabled: false }]);
    return json([peer]);
  });
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el);
});
afterEach(async () => { await act(async () => root.unmount()); el.remove(); vi.unstubAllGlobals(); });
const render = () => act(async () => root.render(<PeerSettings server={server} />));

describe('registro de outros servidores', () => {
  it('não usa a máscara da listagem e só registra com token explicitamente informado', async () => {
    await render();
    await type(m.mobile_machines_peer_id(), 'remote'); await type(m.mobile_machines_peer_url(), remote.baseUrl);
    await click(m.mobile_machines_save());
    expect(writes()).toHaveLength(0); expect(el.textContent).toContain(m.mobile_machines_token_required());
    await type(m.mobile_machines_token(), 'explicit-secret'); await click(m.mobile_machines_save());
    expect(writes()).toHaveLength(1);
    expect(JSON.parse(writes()[0][1].body)).toEqual({ id: 'remote', base_url: remote.baseUrl, token: 'explicit-secret' });
  });

  it('exige token explícito mesmo com máquina salva no mesmo endereço e outro identificador digitado', async () => {
    await act(async () => { useServers.getState().add(remote); });
    await render();
    await type(m.mobile_machines_peer_id(), 'other-machine'); await type(m.mobile_machines_peer_url(), remote.baseUrl);
    await click(m.mobile_machines_save());
    expect(el.textContent).toContain(m.mobile_machines_token_required());
    expect(writes()).toHaveLength(0);
    expect(JSON.stringify(mocks.fetch.mock.calls)).not.toContain(remote.token);
  });

  it('recusa endereços locais que apontariam para o próprio servidor', async () => {
    await render(); await type(m.mobile_machines_peer_id(), 'remote'); await type(m.mobile_machines_token(), 'explicit-secret');
    for (const url of ['http://localhost:8765', 'http://127.2.3.4:8765', 'http://0.0.0.0:8765', 'http://[::1]:8765', 'http://[::]:8765']) {
      await type(m.mobile_machines_peer_url(), url); await click(m.mobile_machines_save());
      expect(writes(), url).toHaveLength(0);
    }
  });

  it('testa ida e volta com token salvo e mostra quem respondeu com identidade divergente', async () => {
    await act(async () => { useServers.getState().add(remote); });
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => url.startsWith(remote.baseUrl) && url.includes('/check?')
      ? json({ estado: 'estranho', identificador: 'wrong-machine', motivo: 'fixture-divergent', tempo_ms: 14 }) : original(url, init));
    await render(); await click(m.mobile_machines_check());
    const checks = mocks.fetch.mock.calls.filter(([url]) => String(url).includes('/check?'));
    expect(checks).toHaveLength(2);
    expect(checks.map(([url]) => new URL(url).origin)).toEqual(expect.arrayContaining([server.baseUrl, remote.baseUrl]));
    expect(el.textContent).toContain('wrong-machine'); expect(el.textContent).toContain('fixture-divergent');
    expect(el.textContent).not.toContain(m.mobile_machines_registered());
  });

  it('desabilita e remove somente o registro local com confirmação explícita', async () => {
    await act(async () => { useServers.getState().add(remote); });
    await render(); await click(m.mobile_machines_peer_disable());
    expect(JSON.parse(writes()[0][1].body)).toEqual({ enabled: false });
    await click(m.lista_remover());
    expect(writes().filter(([, init]) => init.method === 'DELETE')).toHaveLength(0);
    expect(mocks.alert.mock.calls.at(-1)![1]).toBe(m.mobile_machines_peer_remove_body({ name: peer.id }));
    const accept = mocks.alert.mock.calls.at(-1)![2].find((b: { style?: string }) => b.style === 'destructive').onPress;
    await act(async () => { accept(); accept(); });
    const deletes = writes().filter(([, init]) => init.method === 'DELETE');
    expect(deletes).toHaveLength(1); expect(deletes[0][0]).toBe(`${server.baseUrl}/api/peers/remote`);
    expect(useServers.getState().servers).toHaveLength(2);
    expect(el.textContent).toContain(m.mobile_machines_peer_removed());
  });

  it('não mostra o diagnóstico antigo depois de substituir URL/token do mesmo identificador', async () => {
    const original = mocks.fetch.getMockImplementation()!;
    const nextUrl = 'https://replacement.test';
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url.includes('/check?')) return json({ estado: 'ok', identificador: 'remote', motivo: 'fixture-old-measurement', tempo_ms: 8 });
      if (url.endsWith('/api/peers') && init?.method === 'POST') return json([{ ...peer, base_url: nextUrl }]);
      return original(url, init);
    });
    await render(); await click(m.mobile_machines_check());
    expect(el.textContent).toContain('fixture-old-measurement');
    await type(m.mobile_machines_peer_id(), peer.id); await type(m.mobile_machines_peer_url(), nextUrl); await type(m.mobile_machines_token(), 'replacement-secret');
    await click(m.mobile_machines_save());
    expect(el.textContent).toContain(nextUrl);
    expect(el.textContent).not.toContain('fixture-old-measurement');
  });

  it('bloqueia o POST enquanto a nova leitura da lista está adiada e libera depois dela', async () => {
    let finishRead!: (response: Response) => void;
    const pending = new Promise<Response>((resolve) => { finishRead = resolve; });
    let reads = 0;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url.endsWith('/api/peers') && init?.method === 'POST') return json([{ ...peer, id: 'added', base_url: 'https://added.test' }]);
      if (url.endsWith('/api/peers')) return ++reads === 1 ? json({ detail: 'fixture-list-failed' }, 503) : pending;
      return json({ identificador: 'owner', lan_url: '' });
    });
    await render();
    await type(m.mobile_machines_peer_id(), 'added'); await type(m.mobile_machines_peer_url(), 'https://added.test'); await type(m.mobile_machines_token(), 'added-secret');
    await click(m.mobile_machines_retry());
    try {
      await click(m.mobile_machines_save());
      expect(writes()).toHaveLength(0);
    } finally { await act(async () => finishRead(json([]))); }
    await click(m.mobile_machines_save());
    expect(writes()).toHaveLength(1);
    await click('added');
    expect(el.textContent).toContain('https://added.test');
  });
});
