// @vitest-environment happy-dom
import { act, createElement, forwardRef, useImperativeHandle, useState, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const boundary = vi.hoisted(() => ({
  params: {} as Record<string, string | string[]>, replace: vi.fn(), redeem: vi.fn(), sessions: vi.fn(), fetch: vi.fn(), warning: vi.fn(),
  scan: null as null | ((event: { data: string }) => void), vault: new Map<string, string>(),
}));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(), redeemInvite: boundary.redeem, getSessions: boundary.sessions }));
vi.mock('expo-router', () => ({ useLocalSearchParams: () => boundary.params, useRouter: () => ({ replace: boundary.replace }) }));
vi.mock('expo-constants', () => ({ default: { deviceName: 'Test phone' } }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async (key: string) => boundary.vault.get(key) ?? null, setItemAsync: async (key: string, value: string) => { boundary.vault.set(key, value); } }));
vi.mock('expo-blur', () => ({ BlurView: ({ children }: { children: ReactNode }) => createElement('div', null, children), BlurTargetView: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('react-native-keyboard-controller', async () => ({ KeyboardAwareScrollView: (await import('react-native')).View, KeyboardAvoidingView: (await import('react-native')).View }));
vi.mock('expo-linear-gradient', async () => ({ LinearGradient: (await import('react-native')).View }));
vi.mock('expo-file-system', () => ({ Directory: class {}, File: class {}, Paths: { cache: '/cache', document: '/document' } }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, getNumber: () => undefined, getBoolean: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('sonner-native', () => ({ toast: Object.assign(boundary.warning, { success: () => {}, error: () => {}, dismiss: () => {} }), Toaster: () => null }));
vi.mock('expo-camera', () => ({ useCameraPermissions: () => [{ granted: true }, () => {}], CameraView: (props: { onBarcodeScanned: (event: { data: string }) => void }) => { boundary.scan = props.onBarcodeScanned; return null; } }));
vi.mock('@lodev09/react-native-true-sheet', () => ({ TrueSheet: forwardRef(function NativeSheet({ children }: { children: ReactNode }, ref) {
  const [visible, setVisible] = useState(false);
  useImperativeHandle(ref, () => ({ present: async () => { setVisible(true); }, dismiss: async () => { setVisible(false); } }));
  return visible ? createElement('div', null, children) : null;
}) }));
vi.mock('react-native', async (original) => {
  const actual = await original<typeof import('react-native')>();
  return { ...actual,
    View: ({ accessibilityElementsHidden, importantForAccessibility, ...props }: Record<string, unknown>) => createElement(actual.View, props),
    Pressable: ({ accessibilityState, ...props }: Record<string, unknown>) => createElement(actual.Pressable, props),
    Text: ({ selectable, ...props }: Record<string, unknown>) => createElement(actual.Text, props),
    TextInput: (props: { accessibilityLabel?: string; value?: string; onChangeText?: (value: string) => void }) => createElement('input', {
      'aria-label': props.accessibilityLabel, value: props.value ?? '', onInput: (event: { currentTarget: { value: string } }) => props.onChangeText?.(event.currentTarget.value),
    }),
  };
});
vi.mock('react-native-unistyles', async (original) => {
  const actual = await original<typeof import('react-native-unistyles')>();
  const { theme, rt } = actual.useUnistyles();
  return { ...actual, StyleSheet: { ...actual.StyleSheet, create: (styles: unknown) => typeof styles === 'function'
    ? styles({ ...theme, base: { ...theme.base, weight: { medium: '500', semibold: '600' } } }, rt) : styles } };
});
import Invite from '../../../app/convite/[code]';
import Login from '../../../app/login';
import { useServers } from '../../stores/servers';

const LINK = 'https://guest.ts.net:8443/convite/CODE';
const RESULT = { token: 'guest-token', session: 'shared', owner: 'Ana', address: 'https://guest.ts.net:8443' };
const roots: Root[] = [];
async function mount(Component: typeof Invite) {
  const el = document.createElement('div');
  const root = createRoot(el);
  roots.push(root);
  await act(async () => { root.render(createElement(Component)); });
  return el;
}
async function click(el: HTMLElement, label: string) {
  await act(async () => { (el.querySelector(`[aria-label="${label}"]`) as HTMLButtonElement | null)?.click(); });
}
const input = (el: HTMLElement, label = m.mobile_share_invite_link_label()) => el.querySelector(`[aria-label="${label}"]`) as HTMLInputElement | null;

beforeEach(async () => {
  boundary.params = {};
  boundary.scan = null;
  boundary.redeem.mockReset().mockResolvedValue(RESULT);
  boundary.sessions.mockReset().mockResolvedValue([]);
  boundary.fetch.mockReset().mockImplementation(async (url: string) => {
    if (url !== 'http://10.0.0.5:8765/api/peers/identificador') throw new Error('unexpected-fetch');
    return new Response(JSON.stringify({ identificador: 'fixture-owner', lan_url: '' }), {
      status: 200, headers: { 'Content-Type': 'application/json' },
    });
  });
  vi.stubGlobal('fetch', boundary.fetch);
  boundary.replace.mockReset();
  boundary.warning.mockClear();
  boundary.vault.clear();
  useServers.setState({ servers: [], activeId: null, ready: false, vaultReadable: false, aviso: null });
  await useServers.getState().load();
});
afterEach(async () => {
  await act(async () => { roots.splice(0).forEach((root) => root.unmount()); });
  vi.unstubAllGlobals();
});

describe('nome de máquina no login', () => {
  it.each(['identificador atrasado', 'identificador vazio', 'consulta falha'])('cadastro novo não usa IP: %s', async (scenario) => {
    let resolve!: (response: Response) => void;
    const pending = new Promise<Response>((yes) => { resolve = yes; });
    boundary.fetch.mockImplementation(async (url: string) => {
      if (url.endsWith('/identificador')) return pending;
      return new Response(JSON.stringify(url.endsWith('/app-list') ? { rev: 'r1', servers: [] } : []));
    });
    const el = await mount(Login);
    await click(el, m.login_qr());
    await act(async () => { boundary.scan?.({ data: 'http://10.0.0.5:8765/?token=owner-token' }); });

    await click(el, m.login_conectar());

    expect(useServers.getState().active()?.label).toBe(m.native_settings_group_server());
    expect(JSON.parse(boundary.vault.get('cp_servers_v1')!).servers[0].label).toBe(m.native_settings_group_server());
    expect(boundary.replace).not.toHaveBeenCalled();
    await act(async () => { resolve(new Response(JSON.stringify(scenario === 'consulta falha'
      ? { detail: 'fixture-identity-unavailable' }
      : { identificador: scenario === 'identificador vazio' ? '' : 'fixture-owner', lan_url: '' }), { status: scenario === 'consulta falha' ? 503 : 200 })); });

    const label = scenario === 'identificador atrasado' ? 'fixture-owner' : m.native_settings_group_server();
    expect(useServers.getState().active()?.label).toBe(label);
    expect(JSON.parse(boundary.vault.get('cp_servers_v1')!).servers[0].label).toBe(label);
    expect(boundary.replace).toHaveBeenCalledExactlyOnceWith('/');
    expect(boundary.fetch.mock.calls.filter(([url]) => String(url).endsWith('/identificador'))).toHaveLength(1);
    if (scenario === 'identificador atrasado') expect(boundary.warning).not.toHaveBeenCalled();
    else expect(boundary.warning).toHaveBeenCalledExactlyOnceWith(scenario === 'consulta falha'
      ? '503: fixture-identity-unavailable' : m.mobile_machines_invalid_response(), expect.any(Object));
  });

  it('cadastro salvo conserva apelido inclusive 10 sem consultar identificador', async () => {
    const saved = { id: 'saved', baseUrl: 'http://10.0.0.5:8765', token: 'old-token', label: '10' };
    boundary.vault.set('cp_servers_v1', JSON.stringify({ servers: [saved], activeId: saved.id }));
    await useServers.getState().load();
    boundary.fetch.mockImplementation(async (url: string) => new Response(JSON.stringify(url.endsWith('/app-list') ? { rev: 'r1', servers: [] } : [])));
    const el = await mount(Login);
    await click(el, m.login_qr());
    await act(async () => { boundary.scan?.({ data: `${saved.baseUrl}/?token=new-token` }); });

    await click(el, m.login_conectar());

    expect(useServers.getState().active()).toEqual({ ...saved, token: 'new-token' });
    expect(boundary.fetch.mock.calls.some(([url]) => String(url).endsWith('/identificador'))).toBe(false);
  });

  it('queda de rede conserva a máquina salva e aponta o Tailscale', async () => {
    const other = { id: 'other', baseUrl: 'http://10.0.0.6:8765', token: 'other-token', label: 'other' };
    const saved = { id: 'saved', baseUrl: 'https://hangar.tail0000.ts.net', token: 'old-token', label: 'casa' };
    boundary.vault.set('cp_servers_v1', JSON.stringify({ servers: [other, saved], activeId: other.id }));
    await useServers.getState().load();
    // O erro de rede só vale como "sem resposta" passando pelo caminho real até o `fetch`.
    const { getSessions } = await vi.importActual<typeof import('@hangar/core')>('@hangar/core');
    boundary.sessions.mockImplementation(getSessions);
    boundary.fetch.mockRejectedValue(new TypeError('Network request failed'));
    const el = await mount(Login);
    await click(el, m.login_qr());
    await act(async () => { boundary.scan?.({ data: `${saved.baseUrl}/?token=new-token` }); });

    await click(el, m.login_conectar());

    expect(useServers.getState().servers).toEqual([other, saved]);
    expect(useServers.getState().activeId).toBe(other.id);
    expect(el.textContent).toContain(m.login_tailscale_sem_acesso({ host: 'hangar.tail0000.ts.net' }));
    expect(boundary.replace).not.toHaveBeenCalled();
  });
});

describe('entrada de convite nas rotas', () => {
  it('rota conserva link recebido, confirma e abre o chat no ID persistido', async () => {
    boundary.params = { code: 'CODE', link: LINK };
    const el = await mount(Invite);
    expect(input(el)?.value).toBe(LINK);
    expect(boundary.redeem).not.toHaveBeenCalled();
    await click(el, m.mobile_share_invite_accept());
    expect(boundary.replace).toHaveBeenCalledExactlyOnceWith(`/s/${useServers.getState().activeId}/shared`);
  });

  it.each([{ link: undefined }, { link: [LINK, LINK] }])('código sem link único não adivinha servidor: $link', async ({ link }) => {
    boundary.params = { code: 'CODE', ...(link === undefined ? {} : { link }) };
    const el = await mount(Invite);
    expect(input(el)?.value).toBe('');
    expect(el.querySelector('[role="alert"]')?.textContent).toBe(m.mobile_share_invite_invalid());
    expect(boundary.redeem).not.toHaveBeenCalled();
  });

  it('link válido colado na rota incompleta elimina o erro ao confirmar', async () => {
    boundary.params = { code: 'CODE' };
    const el = await mount(Invite);
    expect(el.querySelector('[role="alert"]')?.textContent).toBe(m.mobile_share_invite_invalid());
    const field = input(el)!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(field, LINK);
      field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await click(el, m.mobile_share_invite_retry());
    expect(el.querySelector('[role="alert"]')).toBeNull();
    expect(boundary.redeem).toHaveBeenCalledExactlyOnceWith(LINK, 'Test phone', undefined, undefined);
    expect(boundary.replace).toHaveBeenCalledExactlyOnceWith(`/s/${useServers.getState().activeId}/shared`);
  });

  it('login oferece colar convite com o mesmo formulário', async () => {
    const el = await mount(Login);
    await click(el, m.mobile_share_invite_open());
    expect(input(el)?.value).toBe('');
    expect(el.textContent).toContain(m.mobile_share_invite_title());
  });

  it('QR de convite passa ao formulário sem resgate automático e abre chat ao confirmar', async () => {
    const el = await mount(Login);
    await click(el, m.login_qr());
    expect(boundary.scan).not.toBeNull();
    await act(async () => { boundary.scan?.({ data: LINK }); });
    expect(input(el)?.value).toBe(LINK);
    expect(boundary.redeem).not.toHaveBeenCalled();
    await click(el, m.mobile_share_invite_accept());
    expect(boundary.replace).toHaveBeenCalledExactlyOnceWith(`/s/${useServers.getState().activeId}/shared`);
    expect(boundary.sessions).not.toHaveBeenCalled();
  });

  it('QR de máquina própria continua no pareamento original', async () => {
    const el = await mount(Login);
    await click(el, m.login_qr());
    await act(async () => { boundary.scan?.({ data: 'http://10.0.0.5:8765/?token=owner-token' }); });
    expect(input(el, m.sessao_url_servidor())?.value).toBe('http://10.0.0.5:8765');
    expect(input(el, m.sessao_token())?.value).toBe('owner-token');
    await click(el, m.login_conectar());
    expect(boundary.sessions).toHaveBeenCalledTimes(1);
    expect(boundary.redeem).not.toHaveBeenCalled();
    expect(useServers.getState().active()?.invite).toBeFalsy();
    expect(boundary.replace).toHaveBeenCalledWith('/');
  });
});
