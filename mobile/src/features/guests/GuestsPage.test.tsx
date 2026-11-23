// @vitest-environment happy-dom
import React, { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, type Server } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ vault: new Map<string, string>() }));
vi.mock('expo-secure-store', () => ({
  getItemAsync: async (key: string) => mocks.vault.get(key) ?? null,
  setItemAsync: async (key: string, value: string) => { mocks.vault.set(key, value); },
  deleteItemAsync: async (key: string) => { mocks.vault.delete(key); },
}));
vi.mock('expo-constants', () => ({ default: { deviceName: 'fixture-phone' } }));
vi.mock('expo-router', () => ({ Stack: { Screen: () => null }, useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  TextInput: (p: { accessibilityLabel?: string; value?: string; editable?: boolean; onChangeText?: (t: string) => void }) =>
    <input aria-label={p.accessibilityLabel} disabled={p.editable === false} value={p.value ?? ''} onChange={(e) => p.onChangeText?.(e.target.value)} />,
}));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../config/Pagina', () => ({ Pagina: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
// A senha antiga do dono levaria 600 mil voltas de PBKDF2; o cofre falso guarda o JSON aberto em `data`.
vi.mock('../login/syncCrypto', () => ({
  syncCrypto: {
    deriveKeys: async (password: string) => ({ authHash: password === 'fixture-owner-pw' ? 'fixture-auth' : 'fixture-wrong', encKey: new Uint8Array(32) }),
    decryptJson: (_key: Uint8Array, blob: { data: string }) => JSON.parse(blob.data),
  },
}));

import { Alert } from 'react-native';
import { useServers } from '../../stores/servers';
import { useConnect } from '../connect/connectStore';
import { GuestsPage } from './GuestsPage';
import * as m from '../../paraglide/messages';

const casa: Server = { id: 'casa', label: 'fixture-casa', baseUrl: 'https://casa.test', token: 'owner-casa' };
const pc2: Server = { id: 'pc2', label: 'fixture-pc2', baseUrl: 'https://pc2.test', token: 'owner-pc2' };
type Row = { user: string; sees_owner: boolean; owner_sees: boolean; name_taken: boolean; servers: { server_id: string; label: string; base_url: string; guest_id: string; root: string }[] };
const ana = (): Row => ({ user: 'ana.silva', sees_owner: false, owner_sees: true, name_taken: false,
  servers: [{ server_id: 'casa', label: 'fixture-casa', base_url: 'https://casa.test', guest_id: 'g-casa', root: '/srv/ana' }] });
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

let root: Root;
let el: HTMLDivElement;
let rows: Row[];
let listStatus = 200;
let listGate: Promise<void> | null = null;
let failOn: string | null = null;
let storeStatus = 200;
let syncSetup: Record<string, unknown> = { enabled: false, registered: false, user: null };
let legacyGuests: { user: string; admin: unknown }[] = [];
let calls: { origin: string; method: string; path: string; body: Record<string, unknown> | null }[] = [];

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
const posted = (origin: string, path: string) => calls.filter((c) => c.origin === origin && c.method === 'POST' && c.path === path);

beforeEach(async () => {
  mocks.vault.clear();
  mocks.vault.set('cp_servers_v1', JSON.stringify({ servers: [casa, pc2], activeId: 'casa' }));
  await useServers.getState().load();
  useConnect.setState(useConnect.getInitialState());
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized: vi.fn(), origin: null, createEventSource: () => { throw new Error('sem SSE'); } });
  rows = [ana()];
  listStatus = 200;
  listGate = null;
  failOn = null;
  storeStatus = 200;
  syncSetup = { enabled: false, registered: false, user: null };
  legacyGuests = [];
  calls = [];
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const url = new URL(input);
    const method = init?.method ?? 'GET';
    const body = init?.body ? JSON.parse(String(init.body)) as Record<string, unknown> : null;
    calls.push({ origin: url.origin, method, path: url.pathname, body });
    if (`${url.origin}${url.pathname}` === failOn) return json({ detail: 'fixture-broken' }, 500);
    if (url.pathname === '/api/sync/setup') return json(url.origin === casa.baseUrl ? syncSetup : { enabled: false, registered: false, user: null });
    if (url.pathname === '/api/sync/prelogin') return json({ salt: 'AAAAAAAAAAAAAAAAAAAAAA==', iterations: 600_000 });
    if (url.pathname === '/api/sync/login') return body?.auth_hash === 'fixture-auth' ? json({ ok: true }) : json({}, 401);
    if (url.pathname === '/api/sync/vault') return json({ enc_blob: { iv: 'x', data: JSON.stringify([{ id: 'web-casa', label: 'Casa', baseUrl: 'https://casa.test', token: 'x' }]) }, rev: 1 });
    if (url.pathname === '/api/sync/guests') return json(legacyGuests.map((g) => ({ user: g.user, admin_blob: { iv: 'x', data: JSON.stringify(g.admin) } })));
    if (url.pathname.startsWith('/api/sync/')) return json({ ok: true });
    if (url.pathname === '/api/guest-accounts' && method === 'GET') {
      await listGate;
      return listStatus === 200 ? json(rows) : json({ detail: 'fixture-list' }, listStatus);
    }
    if (url.pathname === '/api/guest-accounts') {
      if (storeStatus !== 200) return json({ detail: { code: 'erro_guest_name_taken', params: {}, msg: 'fixture-taken' } }, storeStatus);
      const saved = { ...(body as unknown as Row), name_taken: false };
      rows = [...rows.filter((r) => r.user !== saved.user), saved];
      return json(saved);
    }
    if (url.pathname.endsWith('/delete') && url.pathname.startsWith('/api/guest-accounts/')) {
      rows = rows.filter((r) => `/api/guest-accounts/${r.user}/delete` !== url.pathname);
      return json({ ok: true });
    }
    if (url.pathname === '/api/guests') return json({ id: `g-${url.hostname.split('.')[0]}`, token: `tok-${url.hostname.split('.')[0]}` });
    return json({ ok: true });
  }));
  el = document.createElement('div');
  document.body.appendChild(el);
  root = createRoot(el);
});
afterEach(async () => {
  await act(async () => root.unmount());
  el.remove();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const render = () => act(async () => root.render(<GuestsPage />));

describe('Configurações › Convidados', () => {
  it('carregando, depois a lista com o resumo de cada convidado', async () => {
    let open!: () => void;
    listGate = new Promise((resolve) => { open = resolve; });
    await render();
    expect(el.textContent).toContain(m.comum_carregando());
    await act(async () => open());
    await vi.waitFor(() => expect(button('ana.silva')).toBeTruthy());
    expect(el.textContent).toContain(`${m.convidados_resumo({ servidores: '1' })} · ${m.guests_not_sees_mine()}`);
  });

  it('nome recusado pelo Connect aparece na linha do convidado', async () => {
    rows = [{ ...ana(), name_taken: true }];
    await render();
    await vi.waitFor(() => expect(button('ana.silva')).toBeTruthy());
    expect(el.textContent).toContain(m.guests_name_taken());
    expect(el.textContent).not.toContain(m.guests_not_sees_mine());
  });

  it('vazio diz que não há convidado e oferece adicionar', async () => {
    rows = [];
    await render();
    await vi.waitFor(() => expect(el.textContent).toContain(m.convidados_vazio()));
    expect(button(m.convidados_adicionar())).toBeTruthy();
  });

  it('erro mostra o motivo e "Tentar de novo" relê', async () => {
    listStatus = 500;
    await render();
    await vi.waitFor(() => expect(button(m.mobile_machines_retry())).toBeTruthy());
    listStatus = 200;
    await click(m.mobile_machines_retry());
    await vi.waitFor(() => expect(button('ana.silva')).toBeTruthy());
  });

  it('servidor sem a versão nova diz para atualizar', async () => {
    listStatus = 404;
    await render();
    await vi.waitFor(() => expect(el.textContent).toContain(m.guests_update_server()));
  });

  it('adicionar cria o acesso na máquina marcada e grava a conta com a senha e a pasta', async () => {
    rows = [];
    await render();
    await vi.waitFor(() => expect(button(m.convidados_adicionar())).toBeTruthy());
    await click(m.convidados_adicionar());
    await type(m.login_usuario(), 'Bia.Souza');
    await type(m.login_senha(), 'fixture-guest-pw');
    await click('fixture-pc2');
    await type(m.convidados_pasta({ servidor: 'fixture-pc2' }), '/srv/bia');
    await click(m.convidados_salvar());
    await vi.waitFor(() => expect(posted(casa.baseUrl, '/api/guest-accounts')).toHaveLength(1));
    expect(posted(pc2.baseUrl, '/api/guests').map((c) => c.body)).toEqual([{ name: 'bia.souza', root: '/srv/bia', sees_owner: false, owner_sees: true }]);
    expect(posted(casa.baseUrl, '/api/guest-accounts')[0].body).toMatchObject({ user: 'bia.souza', password: 'fixture-guest-pw',
      servers: [{ server_id: 'pc2', guest_id: 'g-pc2', token: 'tok-pc2', root: '/srv/bia', base_url: 'https://pc2.test' }] });
    await vi.waitFor(() => expect(button('bia.souza')).toBeTruthy());
  });

  it('editar sem senha mantém a senha e só muda o que foi mexido', async () => {
    await render();
    await vi.waitFor(() => expect(button('ana.silva')).toBeTruthy());
    await click('ana.silva');
    expect(el.textContent).toContain(m.guests_password_keep());
    await click(m.convidados_ve_minhas());
    await click(m.convidados_salvar());
    await vi.waitFor(() => expect(posted(casa.baseUrl, '/api/guest-accounts')).toHaveLength(1));
    const body = posted(casa.baseUrl, '/api/guest-accounts')[0].body!;
    expect(body).not.toHaveProperty('password');
    expect(body).toMatchObject({ user: 'ana.silva', sees_owner: true, servers: [{ server_id: 'casa', guest_id: 'g-casa', root: '/srv/ana' }] });
    expect(posted(casa.baseUrl, '/api/guests/g-casa')).toHaveLength(1);
    expect(posted(casa.baseUrl, '/api/guests')).toHaveLength(0);
  });

  it('falha numa máquina aparece com o nome dela, e a conta guarda o resto', async () => {
    await render();
    await vi.waitFor(() => expect(button('ana.silva')).toBeTruthy());
    await click('ana.silva');
    failOn = `${pc2.baseUrl}/api/guests`;
    await click('fixture-pc2');
    await type(m.convidados_pasta({ servidor: 'fixture-pc2' }), '/srv/ana');
    await click(m.convidados_salvar());
    await vi.waitFor(() => expect(el.textContent).toContain(`fixture-pc2: `));
    expect(posted(casa.baseUrl, '/api/guest-accounts')[0].body).toMatchObject({ servers: [{ server_id: 'casa' }] });
  });

  it('nome em uso ao criar tira os acessos novos, e o que não saiu aparece com o nome da máquina', async () => {
    rows = [];
    await render();
    await vi.waitFor(() => expect(button(m.convidados_adicionar())).toBeTruthy());
    await click(m.convidados_adicionar());
    await type(m.login_usuario(), 'bia.souza');
    await type(m.login_senha(), 'fixture-guest-pw');
    await click('fixture-casa');
    await type(m.convidados_pasta({ servidor: 'fixture-casa' }), '/srv/bia');
    await click('fixture-pc2');
    await type(m.convidados_pasta({ servidor: 'fixture-pc2' }), '/srv/bia');
    storeStatus = 409;
    failOn = `${pc2.baseUrl}/api/guests/g-pc2/delete`;
    await click(m.convidados_salvar());
    await vi.waitFor(() => expect(el.textContent).toContain(m.convidados_usuario_em_uso()));
    const left = (servidor: string) => m.guests_access_left({ servidor, usuario: 'bia.souza', erro: '\u0000' }).split('\u0000')[0];
    expect(el.textContent).toContain(left('fixture-pc2'));
    expect(el.textContent).not.toContain(left('fixture-casa'));
    expect(posted(casa.baseUrl, '/api/guests/g-casa/delete')).toHaveLength(1);
    // A limpeza não escreve a conta de novo na máquina que guarda as contas.
    expect(posted(casa.baseUrl, '/api/guest-accounts')).toHaveLength(1);
  });

  it('remover pede confirmação com o aviso, tira o acesso das máquinas e apaga a conta', async () => {
    vi.spyOn(Alert, 'alert').mockImplementation((_title, body, buttons) => {
      expect(body).toBe(m.convidados_remover_aviso({ usuario: 'ana.silva' }));
      (buttons as { onPress?: () => void }[])[1].onPress?.();
    });
    await render();
    await vi.waitFor(() => expect(button('ana.silva')).toBeTruthy());
    await click('ana.silva');
    await click(m.convidados_remover());
    await vi.waitFor(() => expect(posted(casa.baseUrl, '/api/guest-accounts/ana.silva/delete')).toHaveLength(1));
    expect(posted(casa.baseUrl, '/api/guests/g-casa/delete')).toHaveLength(1);
    await vi.waitFor(() => expect(el.textContent).toContain(m.convidados_vazio()));
  });

  it('conta antiga vem para o Connect com a mesma senha e os mesmos acessos, e só depois sai do cofre antigo', async () => {
    syncSetup = { enabled: true, registered: true, user: 'dono' };
    legacyGuests = [{ user: 'lia.costa', admin: { user: 'lia.costa', password: 'fixture-lia-pw', seesOwner: true, ownerSees: false,
      servers: [{ serverId: 'web-casa', guestId: 'g-old', token: 'tok-old', root: '/srv/lia' }] } }];
    await render();
    await vi.waitFor(() => expect(el.textContent).toContain(m.guests_legacy_title()));
    await click('fixture-casa');
    await type(m.login_senha(), 'fixture-owner-pw');
    await click(m.login_entrar());
    await vi.waitFor(() => expect(button('lia.costa')).toBeTruthy());
    await click('lia.costa');
    await vi.waitFor(() => expect(posted(casa.baseUrl, '/api/sync/guests/lia.costa/delete')).toHaveLength(1));
    expect(posted(casa.baseUrl, '/api/guests/g-old')).toHaveLength(1);
    expect(posted(casa.baseUrl, '/api/guests')).toHaveLength(0);
    const saved = posted(casa.baseUrl, '/api/guest-accounts');
    expect(saved[0].body).toMatchObject({ user: 'lia.costa', password: 'fixture-lia-pw', sees_owner: true, owner_sees: false,
      servers: [{ server_id: 'casa', guest_id: 'g-old', token: 'tok-old', root: '/srv/lia' }] });
    const order = calls.map((c) => c.path);
    expect(order.indexOf('/api/guest-accounts', order.indexOf('/api/sync/guests'))).toBeLessThan(order.indexOf('/api/sync/guests/lia.costa/delete'));
  });

  it('conta antiga sem ponto nem @ pede um nome novo, já sugerido, e avisa que a senha continua a mesma', async () => {
    syncSetup = { enabled: true, registered: true, user: 'dono' };
    legacyGuests = [{ user: 'lia', admin: { user: 'lia', password: 'fixture-lia-pw', seesOwner: false, ownerSees: true,
      servers: [{ serverId: 'web-casa', guestId: 'g-old', token: 'tok-old', root: '/srv/lia' }] } }];
    await render();
    await vi.waitFor(() => expect(el.textContent).toContain(m.guests_legacy_title()));
    await click('fixture-casa');
    await type(m.login_senha(), 'fixture-owner-pw');
    await click(m.login_entrar());
    await vi.waitFor(() => expect(button('lia')).toBeTruthy());
    await click('lia');
    const field = el.querySelector(`input[aria-label="${m.guests_legacy_new_name()}"]`) as HTMLInputElement;
    expect(field.value).toBe('lia@dono');
    expect(el.textContent).toContain(m.guests_legacy_rename_help({ antigo: 'lia', novo: 'lia@dono' }));
    // Nada mudou nas máquinas antes de a pessoa confirmar o nome.
    expect(posted(casa.baseUrl, '/api/guest-accounts')).toHaveLength(0);
    await type(m.guests_legacy_new_name(), 'Lia.Costa');
    expect(el.textContent).toContain(m.guests_legacy_rename_help({ antigo: 'lia', novo: 'lia.costa' }));
    await click(m.guests_legacy_bring());
    await vi.waitFor(() => expect(posted(casa.baseUrl, '/api/sync/guests/lia/delete')).toHaveLength(1));
    expect(posted(casa.baseUrl, '/api/guest-accounts')[0].body).toMatchObject({ user: 'lia.costa', password: 'fixture-lia-pw',
      servers: [{ server_id: 'casa', guest_id: 'g-old', token: 'tok-old' }] });
  });
});
