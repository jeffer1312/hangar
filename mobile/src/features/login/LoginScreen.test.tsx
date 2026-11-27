// @vitest-environment happy-dom
import React, { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, type Server } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ vault: new Map<string, string>(), replace: vi.fn(), push: vi.fn() }));
vi.mock('expo-secure-store', () => ({
  getItemAsync: async (key: string) => mocks.vault.get(key) ?? null,
  setItemAsync: async (key: string, value: string) => { mocks.vault.set(key, value); },
  deleteItemAsync: async (key: string) => { mocks.vault.delete(key); },
}));
vi.mock('expo-constants', () => ({ default: { deviceName: 'fixture-phone' } }));
vi.mock('expo-router', () => ({ useFocusEffect: () => {}, useRouter: () => ({ push: mocks.push, replace: mocks.replace }) }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  TextInput: (p: { accessibilityLabel?: string; value?: string; editable?: boolean; onChangeText?: (t: string) => void }) =>
    <input aria-label={p.accessibilityLabel} disabled={p.editable === false} value={p.value ?? ''} onChange={(e) => p.onChangeText?.(e.target.value)} />,
}));
vi.mock('react-native-keyboard-controller', () => ({
  KeyboardAwareScrollView: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  KeyboardAvoidingView: ({ children }: { children: ReactNode }) => <div>{children}</div>,
}));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ open, children }: { open: boolean; children: ReactNode }) => (open ? <div>{children}</div> : null) }));
vi.mock('../../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Screen', () => ({ Screen: ({ children }: { children: ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('../connect/ConnectHero', () => ({ ConnectMark: () => null }));
vi.mock('./QrScanner', () => ({ QrScanner: () => <div>fixture-qr-scanner</div> }));
vi.mock('../share/InvitePanel', () => ({ InvitePanel: () => <div>fixture-invite-panel</div> }));
vi.mock('expo-blur', () => ({ BlurView: ({ children }: { children: ReactNode }) => children }));
vi.mock('expo-glass-effect', () => ({ GlassView: ({ children }: { children: ReactNode }) => children, isLiquidGlassAvailable: () => false }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
// O pacote de verdade carrega o react-native real (Flow), que o Node não lê.
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, getNumber: () => undefined, getBoolean: () => undefined, set: () => {}, remove: () => {} }) }));
// A conta de verdade leva 600 mil voltas de PBKDF2; aqui a senha certa só precisa dar o hash certo.
vi.mock('./syncCrypto', () => ({
  syncCrypto: {
    deriveKeys: async (password: string) => ({ authHash: password === 'fixture-right' ? 'fixture-auth' : 'fixture-wrong', encKey: new Uint8Array(32) }),
    decryptJson: () => [{ id: 'g1', label: 'notebook', baseUrl: 'https://notebook.acme.hangar.dev.br', token: 'guest-token' }],
  },
}));

import { useServers } from '../../stores/servers';
import { CONNECT_SERVICE_URL, useConnect } from '../connect/connectStore';
import { LoginScreen } from './LoginScreen';
import * as m from '../../paraglide/messages';

const GUEST_HOST = 'https://notebook.acme.hangar.dev.br';
const MACHINE_HOST = 'https://pc2.acme.hangar.dev.br';
const ACCOUNT_KEY = 'cp_connect_account_v1';
// Desligada: aparece sem marcar, e nenhum pedido de entrada sai dela.
const OFF_MACHINE = { machine_id: 'pc1', name: 'fixture-pc', host: 'pc1.acme.hangar.dev.br', online: false };
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
let root: Root;
let el: HTMLDivElement;
let lookup: () => Response | Promise<Response>;
let deviceStatus = 'pending';
let machineList: unknown[] = [OFF_MACHINE];

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
const signIn = async (name: string, password: string) => {
  await type(m.login_account_or_user(), name);
  await type(m.login_senha(), password);
  await click(m.login_entrar());
};

async function setup(servers: Server[] = []) {
  mocks.vault.clear();
  mocks.vault.set('cp_servers_v1', JSON.stringify({ servers, activeId: servers[0]?.id ?? null }));
  await useServers.getState().load();
}

beforeEach(async () => {
  mocks.replace.mockReset();
  mocks.push.mockReset();
  await setup();
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized: vi.fn(), origin: null, createEventSource: () => { throw new Error('sem SSE'); } });
  useConnect.setState(useConnect.getInitialState());
  deviceStatus = 'pending';
  machineList = [OFF_MACHINE];
  lookup = () => json({ kind: 'account' });
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const url = new URL(input);
    if (url.origin === CONNECT_SERVICE_URL) {
      if (url.pathname === '/v1/lookup') return lookup();
      if (url.pathname === '/v1/sessions') {
        return JSON.parse(String(init?.body)).password === 'fixture-right' ? json({ token: 'account-token' }) : json({}, 401);
      }
      if (url.pathname === '/v1/devices' && init?.method === 'POST') return json({ device_id: 'ad1', device_secret: 'ad-secret' });
      if (url.pathname === '/v1/devices/ad1') return json({ status: deviceStatus });
      if (url.pathname === '/v1/devices') return json([]);
      if (url.pathname === '/v1/machines') return json(machineList);
      if (/^\/v1\/machines\/\w+\/devices$/.test(url.pathname)) return json({ url: MACHINE_HOST, ticket: '{"nonce":"n"}', signature: 'c2ln' });
      return json({});
    }
    // Bilhete pré-aprovado pela conta: a primeira consulta já entrega o token.
    if (url.origin === MACHINE_HOST) {
      if (url.pathname === '/api/connect/redeem') return json({ device_id: 'dev1', status: 'pending' }, 202);
      if (url.pathname === '/api/connect/redeem/status') return json({ status: 'approved', token: 'machine-token' });
      return json({});
    }
    if (url.origin === GUEST_HOST) {
      if (url.pathname === '/api/sync/prelogin') return json({ salt: 'AAAAAAAAAAAAAAAAAAAAAA==', iterations: 600_000 });
      if (url.pathname === '/api/sync/login') return JSON.parse(String(init?.body)).auth_hash === 'fixture-auth' ? json({ ok: true }) : json({}, 401);
      if (url.pathname === '/api/sync/vault') return json({ enc_blob: { iv: 'fixture-iv', data: 'fixture-data' }, rev: 1 });
      return json({ ok: true });
    }
    return json({});
  }));
  el = document.createElement('div');
  document.body.appendChild(el);
  root = createRoot(el);
});
afterEach(async () => {
  // A leitura do aparelho em voo é do store, que sobrevive ao teste.
  await act(async () => { await useConnect.getState().refreshDevices(); });
  await act(async () => root.unmount());
  el.remove();
  vi.unstubAllGlobals();
});

const render = () => act(async () => root.render(<LoginScreen />));

describe('entrar no Hangar', () => {
  it('abre com "Conta ou usuário" e as outras formas recolhidas, que abrem num toque', async () => {
    await render();
    expect(el.textContent).toContain(m.login_hangar_title());
    expect(el.querySelector(`input[aria-label="${m.login_account_or_user()}"]`)).toBeTruthy();
    expect(button(m.login_qr())).toBeUndefined();
    expect(button(m.login_address_token())).toBeUndefined();

    await click(m.login_other_ways());
    for (const label of [m.login_qr(), m.login_address_token(), m.mobile_share_invite_open(), m.mobile_demo_open()]) {
      expect(button(label), label).toBeTruthy();
    }
    await click(m.login_address_token());
    expect(el.querySelector(`input[aria-label="${m.sessao_url_servidor()}"]`)).toBeTruthy();
    expect(button(m.mobile_sync_login_open()), 'endereço + usuário e senha continua lá dentro').toBeTruthy();
  });

  it('a demonstração só aparece sem servidores', async () => {
    await setup([{ id: 'pc1', label: 'fixture-pc', baseUrl: 'https://pc1.test', token: 't' }]);
    await render();
    await click(m.login_other_ways());
    expect(button(m.login_qr())).toBeTruthy();
    expect(button(m.mobile_demo_open())).toBeUndefined();
  });

  it('conta entra e mostra "Liberar este celular" enquanto espera a liberação', async () => {
    await render();
    await signIn('acme', 'fixture-right');
    await vi.waitFor(() => expect(el.textContent).toContain(m.connect_release_title()));
    expect(el.textContent).toContain(m.connect_signed_in_as({ name: 'acme' }));
  });

  it('conta liberada mostra as máquinas dela', async () => {
    deviceStatus = 'approved';
    await render();
    await signIn('acme', 'fixture-right');
    // O registro nasce pendente; a releitura da tela de máquinas acha a liberação.
    await vi.waitFor(() => {
      expect(el.textContent).toContain('fixture-pc');
      expect(el.textContent).not.toContain(m.connect_release_title());
    });
    expect(el.textContent).toContain(m.connect_computers_title());
    expect(el.textContent).toContain(m.connect_machine_off_now());
  });

  it('convidada vê as máquinas liberadas, marca e entra nelas', async () => {
    lookup = () => json({ kind: 'guest', hosts: ['notebook.acme.hangar.dev.br'] });
    await render();
    await signIn('Ana.Silva', 'fixture-right');
    await vi.waitFor(() => expect(el.textContent).toContain(m.login_guest_machines()));
    expect(el.textContent).toContain(m.login_guest_tag());
    expect(el.textContent).toContain('ana.silva');
    expect(el.textContent).toContain(m.login_guest_owner({ owner: 'acme' }));
    expect(el.textContent).toContain(m.login_guest_hint());
    const sent = vi.mocked(fetch).mock.calls.filter(([input]) => new URL(String(input)).origin === CONNECT_SERVICE_URL);
    expect(sent.map(([, init]) => String(init?.body ?? '')).some((body) => body.includes('fixture-right')), 'a senha dela não vai ao serviço').toBe(false);

    await click(m.login_guest_enter_one());
    expect(useServers.getState().servers.map((s) => [s.label, s.baseUrl])).toEqual([['notebook', GUEST_HOST]]);
    expect(mocks.replace).toHaveBeenCalledWith('/');
  });

  it('desmarcar a única máquina trava o botão de entrar', async () => {
    lookup = () => json({ kind: 'guest', hosts: ['notebook.acme.hangar.dev.br'] });
    await render();
    await signIn('ana.silva', 'fixture-right');
    await vi.waitFor(() => expect(button('notebook')).toBeTruthy());
    await act(async () => button('notebook')!.click());
    expect(button(m.login_guest_enter_many({ n: '0' }))?.disabled).toBe(true);
  });

  it.each([
    ['nome desconhecido', () => json({ kind: 'none' }), 'fixture-right'],
    ['senha da conta errada', () => json({ kind: 'account' }), 'fixture-bad'],
    ['senha da convidada errada', () => json({ kind: 'guest', hosts: ['notebook.acme.hangar.dev.br'] }), 'fixture-bad'],
  ])('%s mostra o erro único', async (_case, answer, password) => {
    lookup = answer;
    await render();
    await signIn('acme', password);
    await vi.waitFor(() => expect(el.textContent).toContain(m.login_credentials_mismatch()));
    expect(el.textContent).not.toContain(m.login_guest_machines());
  });

  it('sem rede, o erro diz que foi a rede', async () => {
    lookup = () => { throw new TypeError('fixture-network'); };
    await render();
    await signIn('acme', 'fixture-right');
    await vi.waitFor(() => expect(el.textContent).toContain(m.connect_error_network()));
    expect(el.textContent).not.toContain(m.login_credentials_mismatch());
  });

  it('"Criar conta grátis" abre o Connect direto no formulário de criar', async () => {
    await render();
    await click(m.connect_create_free());
    expect(mocks.push).toHaveBeenCalledWith('/connect?mode=create');
  });

  it('convidada entra sem guardar conta do Connect', async () => {
    lookup = () => json({ kind: 'guest', hosts: ['notebook.acme.hangar.dev.br'] });
    await render();
    await signIn('ana.silva', 'fixture-right');
    await vi.waitFor(() => expect(button(m.login_guest_enter_one())).toBeTruthy());
    await click(m.login_guest_enter_one());
    expect(mocks.replace).toHaveBeenCalledWith('/');
    expect(useConnect.getState().account).toBeNull();
    expect(mocks.vault.has(ACCOUNT_KEY)).toBe(false);
  });

  it('convidada que sai da lista volta ao formulário com a senha limpa', async () => {
    lookup = () => json({ kind: 'guest', hosts: ['notebook.acme.hangar.dev.br'] });
    await render();
    await signIn('ana.silva', 'fixture-right');
    await vi.waitFor(() => expect(button(m.contas_sair())).toBeTruthy());
    await click(m.contas_sair());
    const field = el.querySelector(`input[aria-label="${m.login_senha()}"]`) as HTMLInputElement;
    expect(field).toBeTruthy();
    expect(field.value).toBe('');
  });

  it('conta esperando a liberação já mostra "Sair", e sair volta ao formulário com a senha limpa', async () => {
    await render();
    await signIn('acme', 'fixture-right');
    await vi.waitFor(() => expect(el.textContent).toContain(m.connect_release_title()));
    expect(button(m.connect_sign_out()), 'sair à mostra na espera').toBeTruthy();
    await click(m.connect_sign_out());
    await vi.waitFor(() => expect(el.querySelector(`input[aria-label="${m.login_senha()}"]`)).toBeTruthy());
    expect((el.querySelector(`input[aria-label="${m.login_senha()}"]`) as HTMLInputElement).value).toBe('');
    expect(mocks.vault.has(ACCOUNT_KEY)).toBe(false);
  });

  it('conta liberada só segue para o app depois de escolher as máquinas e tocar em Entrar', async () => {
    deviceStatus = 'approved';
    machineList = [{ machine_id: 'pc2', name: 'fixture-pc2', host: 'pc2.acme.hangar.dev.br', online: true }];
    await render();
    await signIn('acme', 'fixture-right');
    await vi.waitFor(() => expect(button(m.login_guest_enter_one())).toBeTruthy());
    expect(mocks.replace).not.toHaveBeenCalled();

    await click(m.login_guest_enter_one());
    await vi.waitFor(() => expect(mocks.replace).toHaveBeenCalledWith('/'));
    expect(useServers.getState().servers.map((s) => s.baseUrl)).toContain(MACHINE_HOST);
  });

  it('conta que caiu por 401 deixa o aviso no formulário', async () => {
    useConnect.setState({ notice: m.connect_session_ended() });
    await render();
    expect(el.querySelector(`input[aria-label="${m.login_account_or_user()}"]`)).toBeTruthy();
    expect(el.textContent).toContain(m.connect_session_ended());
  });

  it('QR, convite e endereço com token abrem os caminhos de antes', async () => {
    await render();
    await click(m.login_other_ways());
    expect(el.textContent).not.toContain('fixture-qr-scanner');
    await click(m.login_qr());
    expect(el.textContent).toContain('fixture-qr-scanner');

    expect(el.textContent).not.toContain('fixture-invite-panel');
    await click(m.mobile_share_invite_open());
    expect(el.textContent).toContain('fixture-invite-panel');

    await click(m.login_address_token());
    expect(el.querySelector(`input[aria-label="${m.sessao_url_servidor()}"]`)).toBeTruthy();
    expect(el.querySelector(`input[aria-label="${m.sessao_token()}"]`)).toBeTruthy();
  });
});
