// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { ServerBucket, VaultEvent } from '@hangar/core';
import { VaultRequests } from './VaultRequests';
import { openVaultRequest, useVaultTarget } from './target';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';

vi.mock('../machines/parts', () => ({
  MachineSheet: ({ title, children }: { title: string; children: ReactNode }) => createElement('section', { 'aria-label': title }, children),
  Notice: ({ text }: { text: string }) => text || null,
}));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));

const event: VaultEvent = { state: 'open', requests: [{ id: 'r1', interaction_id: 'i1', kind: 'approve', origin: 'a::sessao',
  entry_name: 'github', reason: 'login', use: 'exec', target: 'gh auth status', expires_at: 1 }] };
const bucket = (id: string, extra: Partial<ServerBucket> = {}) => ({
  server: { id, label: id, baseUrl: `https://${id}.test`, token: 't' }, sessions: [], error: null, loaded: true, ...extra,
}) as unknown as ServerBucket;

let container: HTMLElement;
let root: Root;
const render = (byServer: ServerBucket[]) => act(async () => { root.render(<VaultRequests byServer={byServer} />); });
const title = (label: string) => m.vault_app_requests_title({ label, count: 1 });

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement('div');
  root = createRoot(container);
  openVaultRequest(null);
  useSessions.setState({ vault: { a: event } });
});

afterEach(async () => {
  await act(async () => { root.unmount(); });
  useSessions.setState({ vault: {} });
  openVaultRequest(null);
});

it('tocar no aviso do pedido pendente abre a folha daquela máquina', async () => {
  await render([bucket('a'), bucket('b')]);
  const notices = [...container.querySelectorAll('button')].filter((b) => b.getAttribute('aria-label') === title('a'));
  expect(notices).toHaveLength(1);
  expect(container.querySelector(`button[aria-label="${title('b')}"]`)).toBeNull();
  expect(useVaultTarget.getState().target).toBeNull();

  await act(async () => { notices[0].click(); });

  expect(useVaultTarget.getState().target).toEqual({ serverId: 'a', requestId: null });
  expect(container.querySelector(`section[aria-label="${m.vault_app_request_title()}"]`)).not.toBeNull();
  expect(container.textContent).toContain('gh auth status');
});

it('máquina com erro ou ainda carregando não mostra o aviso', async () => {
  await render([bucket('a', { error: 'offline' })]);
  expect(container.querySelector(`button[aria-label="${title('a')}"]`)).toBeNull();
  await render([bucket('a', { loaded: false })]);
  expect(container.querySelector(`button[aria-label="${title('a')}"]`)).toBeNull();
});
