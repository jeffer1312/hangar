// @vitest-environment happy-dom
import React, { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from '@hangar/core';
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => <div>{children}</div> }));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ fetch: vi.fn(), vault: new Map<string, string>() }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async (k: string) => mocks.vault.get(k) ?? null, setItemAsync: async (k: string, v: string) => { mocks.vault.set(k, v); } }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: vi.fn() }) }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  TextInput: (p: { accessibilityLabel?: string; value?: string; editable?: boolean; onChangeText?: (t: string) => void }) =>
    <input aria-label={p.accessibilityLabel} disabled={p.editable === false} value={p.value ?? ''} onChange={(e) => p.onChangeText?.(e.target.value)} />,
}));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('sonner-native', () => ({ toast: vi.fn(), Toaster: () => null }));
import { AddMachineSheet } from './AddMachineSheet';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';
const owner = { id: 'add-owner', label: 'fixture-owner', baseUrl: 'https://owner.test', token: 'owner-secret' };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
let root: Root;
let el: HTMLDivElement;
async function click(label: string) {
  const button = [...el.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label);
  expect(button, label).toBeTruthy(); await act(async () => button!.click());
}
async function type(label: string, value: string) {
  const input = el.querySelector(`input[aria-label="${label}"]`) as HTMLInputElement;
  expect(input, label).toBeTruthy();
  await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value); input.dispatchEvent(new Event('input', { bubbles: true })); });
}
beforeEach(async () => {
  mocks.fetch.mockReset(); mocks.vault.clear();
  mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [owner], activeId: owner.id })); await useServers.getState().load();
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', origin: null, onUnauthorized: vi.fn(), createEventSource: vi.fn() });
  vi.stubGlobal('fetch', mocks.fetch);
  mocks.fetch.mockImplementation(async (url: string) => {
    if (url.endsWith('/descobrir')) return json([{ nome: 'fixture-found', base_url: 'http://10.2.3.4:8765' }]);
    if (url.endsWith('/identificador')) return json({ identificador: 'remote', lan_url: '' });
    return json({ campos: {}, somente_leitura: {} });
  });
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el);
});
afterEach(async () => { await act(async () => root.unmount()); el.remove(); vi.unstubAllGlobals(); });
const render = (target = owner, onClose = () => {}) => act(async () => root.render(<AddMachineSheet owner={target} onClose={onClose} onQr={() => {}} />));

describe('adicionar máquina', () => {
  it('fechar folha com registro opcional pendente não impede a descoberta do cadastro já salvo', async () => {
    let postPending = false;
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url.endsWith('/api/peers') && init?.method === 'POST') {
        postPending = true;
        return new Promise<Response>((_resolve, reject) => {
          init.signal?.addEventListener('abort', () => reject(new DOMException('fixture-cancelled', 'AbortError')), { once: true });
        });
      }
      if (url === 'https://remote.test/api/peers/app-list') return json({ rev: 'r1', servers: [
        { id: 'found', label: 'fixture-found', address: 'https://found.test', token: 'found-token', disabled: false, invite: false },
      ] });
      if (url.endsWith('/api/peers/app-list')) return json({ rev: 'r1', servers: [] });
      if (url.endsWith('/api/peers') || url.endsWith('/api/sessions')) return json([]);
      if (url.endsWith('/identificador')) return json({ identificador: url.startsWith(owner.baseUrl) ? 'owner' : url.startsWith('https://found.test') ? 'found' : 'remote', lan_url: '' });
      return json({ campos: {}, somente_leitura: {} });
    });
    await render(owner, () => root.render(null)); await type(m.mobile_machines_endpoint(), 'https://remote.test');
    await type(m.mobile_machines_token(), 'remote-secret'); await click(m.mobile_machines_register_both());
    await click(m.mobile_machines_save());
    expect(postPending).toBe(true);
    expect(useServers.getState().servers.some((s) => s.baseUrl === 'https://remote.test')).toBe(true);
    await click(m.sessao_fechar());
    await act(async () => { await new Promise<void>((r) => setImmediate(r)); });
    expect(mocks.fetch.mock.calls.some(([url]) => url === 'https://remote.test/api/peers/app-list')).toBe(true);
    expect(useServers.getState().servers.find((s) => s.baseUrl === 'https://found.test'))
      .toMatchObject({ label: 'found', token: 'found-token' });
  });
  it('cadastro autenticado também descobre máquinas sem depender de novo login', async () => {
    mocks.fetch.mockImplementation(async (url: string) => {
      if (url === 'https://remote.test/api/peers/app-list') return json({ rev: 'r1', servers: [
        { id: 'found', label: 'fixture-found', address: 'https://found.test', token: 'found-token', disabled: false, invite: false },
      ] });
      if (url.endsWith('/api/peers/app-list')) return json({ rev: 'r1', servers: [] });
      if (url.endsWith('/api/peers') || url.endsWith('/api/sessions')) return json([]);
      if (url.endsWith('/identificador')) return json({ identificador: url.startsWith('https://found.test') ? 'found-name' : 'remote', lan_url: '' });
      return json({ campos: {}, somente_leitura: {} });
    });
    await render(); await type(m.mobile_machines_endpoint(), 'https://remote.test');
    await type(m.mobile_machines_token(), 'remote-secret'); await click(m.mobile_machines_save());
    await act(async () => { await new Promise<void>((r) => setImmediate(r)); });
    expect(useServers.getState().servers.find((s) => s.baseUrl === 'https://found.test'))
      .toMatchObject({ label: 'found-name', token: 'found-token' });
  });
  it('só descobre Tailscale pelo botão e mantém o nome digitado ao selecionar uma URL', async () => {
    await render(); expect(mocks.fetch).not.toHaveBeenCalled();
    await type(m.mobile_machines_label(), 'fixture-custom'); await click(m.mobile_machines_discover());
    expect(mocks.fetch.mock.calls[0][0]).toBe(`${owner.baseUrl}/api/peers/descobrir`);
    await click('fixture-found · http://10.2.3.4:8765');
    expect((el.querySelector(`input[aria-label="${m.mobile_machines_label()}"]`) as HTMLInputElement).value).toBe('fixture-custom');
    await type(m.mobile_machines_token(), 'remote-secret'); await click(m.mobile_machines_save());
    expect(useServers.getState().servers.at(-1)?.label).toBe('fixture-custom');
    expect(useServers.getState().servers.at(-1)?.baseUrl).toBe('http://10.2.3.4:8765');
  });

  it.each([{ id: 'remote', expected: 'remote' }, { id: '', expected: null }])('sem nome usa identificador ou rótulo neutro: $id', async ({ id, expected }) => {
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string) => url.endsWith('/identificador') ? json({ identificador: id, lan_url: '' }) : original(url));
    await render(); await type(m.mobile_machines_endpoint(), 'http://10.2.3.4:8765'); await type(m.mobile_machines_token(), 'remote-secret');
    expect(mocks.fetch).not.toHaveBeenCalled();
    await click(m.mobile_machines_save());
    expect(useServers.getState().servers.at(-1)?.label).toBe(expected ?? m.native_settings_group_server());
    if (!id) expect(el.textContent).toContain(m.mobile_machines_invalid_response());
  });

  it('sem nome mantém rótulo neutro quando a consulta falha', async () => {
    const original = mocks.fetch.getMockImplementation()!;
    mocks.fetch.mockImplementation(async (url: string) => url.endsWith('/identificador')
      ? json({ detail: 'fixture-identity-unavailable' }, 503) : original(url));
    await render(); await type(m.mobile_machines_endpoint(), 'http://10.2.3.4:8765'); await type(m.mobile_machines_token(), 'remote-secret');

    await click(m.mobile_machines_save());

    expect(useServers.getState().servers.at(-1)?.label).toBe(m.native_settings_group_server());
    expect(el.textContent).toContain('fixture-identity-unavailable');
  });

  it('sem nome aguarda identificador atrasado sem salvar o IP', async () => {
    const original = mocks.fetch.getMockImplementation()!;
    let resolve!: (response: Response) => void;
    const pending = new Promise<Response>((yes) => { resolve = yes; });
    mocks.fetch.mockImplementation(async (url: string) => url.endsWith('/identificador') ? pending : original(url));
    await render(); await type(m.mobile_machines_endpoint(), 'http://10.2.3.4:8765'); await type(m.mobile_machines_token(), 'remote-secret');

    await click(m.mobile_machines_save());
    expect(useServers.getState().servers).toEqual([owner]);
    await act(async () => { resolve(json({ identificador: 'remote', lan_url: '' })); });

    expect(useServers.getState().servers.at(-1)?.label).toBe('remote');
    expect(mocks.fetch.mock.calls.filter(([url]) => String(url).endsWith('/identificador'))).toHaveLength(1);
  });

  it.each(['digitado', 'salvo'])('nome %s prevalece sobre identificador da máquina', async (source) => {
    const saved = { id: 'saved-remote', label: 'fixture-saved', baseUrl: 'http://10.2.3.4:8765', token: 'saved-token' };
    if (source === 'salvo') {
      mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [owner, saved], activeId: owner.id }));
      await useServers.getState().load();
    }
    await render(); await type(m.mobile_machines_endpoint(), saved.baseUrl); await type(m.mobile_machines_token(), 'remote-secret');
    if (source === 'digitado') await type(m.mobile_machines_label(), 'fixture-custom');

    await click(m.mobile_machines_save());

    expect(useServers.getState().servers.at(-1)?.label).toBe(source === 'salvo' ? 'fixture-saved' : 'fixture-custom');
  });

  it('registra ida e volta sem gravar loopback na outra ponta e mostra falha parcial', async () => {
    const localOwner = { ...owner, baseUrl: 'http://127.0.0.1:8765' };
    mocks.fetch.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url.endsWith('/identificador')) return json({ identificador: url.startsWith(localOwner.baseUrl) ? 'owner' : 'remote', lan_url: '' });
      if (url.endsWith('/alcance')) return json({ loopback: true, bind: '127.0.0.1', enderecos: [
        { tipo: 'rede_local', url: 'http://192.168.0.9:8765', estado: 'ok', tempo_ms: 4, motivo: '' },
      ] });
      if (url.endsWith('/api/peers') && init?.method === 'POST') return url.startsWith(localOwner.baseUrl) ? json([]) : json({ detail: 'fixture-return-refused' }, 403);
      if (url.includes('/check?')) return json({ estado: url.startsWith(localOwner.baseUrl) ? 'ok' : 'recusou', identificador: url.startsWith(localOwner.baseUrl) ? 'remote' : '', tempo_ms: 5, motivo: 'fixture-return-check' });
      return json({ campos: {}, somente_leitura: {} });
    });
    await render(localOwner); await type(m.mobile_machines_endpoint(), 'https://remote.test'); await type(m.mobile_machines_token(), 'remote-secret');
    await click(m.mobile_machines_register_both()); await click(m.mobile_machines_save());
    const posts = mocks.fetch.mock.calls.filter(([, init]) => init?.method === 'POST');
    expect(posts).toHaveLength(2);
    expect(JSON.parse(posts[1][1].body).base_url).toBe('http://192.168.0.9:8765');
    expect(el.textContent).toContain('fixture-return-refused');
    expect(el.textContent).not.toContain(m.mobile_machines_registered());
    expect(useServers.getState().servers.some((s) => s.baseUrl === 'https://remote.test')).toBe(true);
    expect(mocks.fetch.mock.calls.filter(([url]) => String(url).includes('/check?'))).toHaveLength(2);
  });
});
