// @vitest-environment happy-dom
import React, { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { LegacySession, Server } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ open: vi.fn(), save: vi.fn(), toastOk: vi.fn() }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(),
  openLegacyGuests: mocks.open, saveGuest: mocks.save }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('expo-constants', () => ({ default: { deviceName: 'fixture-phone' } }));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  TextInput: (p: { accessibilityLabel?: string; value?: string; editable?: boolean; onChangeText?: (t: string) => void }) =>
    <input aria-label={p.accessibilityLabel} disabled={p.editable === false} value={p.value ?? ''} onChange={(e) => p.onChangeText?.(e.target.value)} />,
}));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: mocks.toastOk, erro: vi.fn() } }));
vi.mock('../login/syncCrypto', () => ({ syncCrypto: {} }));

import { LegacySheet } from './LegacySheet';
import { syncFailureText } from '../login/addressSignIn';
import * as m from '../../paraglide/messages';

const casa: Server = { id: 'casa', label: 'fixture-casa', baseUrl: 'https://casa.test', token: 'owner-casa' };
const lia = { user: 'lia.costa', password: 'fixture-lia-pw', seesOwner: false, ownerSees: true,
  servers: [{ serverId: 'casa', guestId: 'g-old', token: 'old-token', root: '/srv/lia' }] };
let root: Root;
let el: HTMLDivElement;

const button = (label: string) => [...el.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label) as HTMLButtonElement | undefined;
const click = async (label: string) => {
  const target = button(label);
  expect(target, label).toBeTruthy();
  await act(async () => target!.click());
};
async function type(label: string, value: string) {
  const input = el.querySelector(`input[aria-label="${label}"]`) as HTMLInputElement;
  expect(input, label).toBeTruthy();
  await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value); input.dispatchEvent(new Event('input', { bubbles: true })); });
}
const session = (remove: LegacySession['remove']): LegacySession => ({ hub: casa.baseUrl, ownerServers: [casa], guests: [lia], remove, close: vi.fn(async () => {}) });
const sheet = (onChanged = vi.fn()) => <LegacySheet hub={casa} syncUser="dono" store={casa} machines={[{ server: casa }]} mine={[casa]}
  onClose={() => {}} onChanged={onChanged} />;
const signIn = async () => {
  await type(m.login_senha(), 'fixture-owner-pw');
  await click(m.login_entrar());
};

beforeEach(() => {
  mocks.open.mockReset();
  mocks.save.mockReset();
  mocks.toastOk.mockReset();
  el = document.createElement('div');
  document.body.appendChild(el);
  root = createRoot(el);
});
afterEach(async () => {
  await act(async () => root.unmount());
  el.remove();
});

it('conta nova guardada e antiga que não saiu: a lista é relida e o erro da remoção aparece', async () => {
  const failure = Object.assign(new Error('sync_request_failed'), { code: 'sync_request_failed', status: 500 });
  const opened = session(vi.fn(async () => { throw failure; }));
  mocks.open.mockResolvedValue(opened);
  mocks.save.mockResolvedValue({ saved: {}, errors: [], storeFailed: false });
  const onChanged = vi.fn();
  await act(async () => root.render(sheet(onChanged)));
  await signIn();
  await vi.waitFor(() => expect(button('lia.costa')).toBeTruthy());

  await click('lia.costa');
  await vi.waitFor(() => expect(el.textContent).toContain(syncFailureText(failure)));
  expect(mocks.save).toHaveBeenCalledTimes(1);
  expect(opened.remove).toHaveBeenCalledWith('lia.costa');
  expect(onChanged).toHaveBeenCalledTimes(1);
  expect(mocks.toastOk).not.toHaveBeenCalled();
  // A antiga continua na lista para tentar de novo.
  expect(button('lia.costa')).toBeTruthy();
});

it('folha fechada com a entrada em voo fecha a sessão que chegou depois e não mexe na tela', async () => {
  let resolve!: (value: LegacySession) => void;
  mocks.open.mockReturnValue(new Promise<LegacySession>((done) => { resolve = done; }));
  await act(async () => root.render(sheet()));
  await signIn();
  expect(button(m.login_entrando())).toBeTruthy();

  await act(async () => root.render(<div />));
  const opened = session(vi.fn(async () => {}));
  await act(async () => { resolve(opened); });
  await vi.waitFor(() => expect(opened.close).toHaveBeenCalledTimes(1));
  expect(el.textContent).toBe('');
});
