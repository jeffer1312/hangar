// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import { configureApi, configureDiag, _resetRotasForTests, type ServerBucket, type SessionInfo } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return {
    ...native,
    View: ({ accessibilityElementsHidden, importantForAccessibility, accessible, ...props }: React.ComponentProps<typeof native.View>) =>
      React.createElement(native.View, props),
    Pressable: ({ hitSlop, accessibilityHint, ...props }: React.ComponentProps<typeof native.Pressable>) =>
      React.createElement(native.Pressable, props),
    ScrollView: ({ stickyHeaderIndices, contentInsetAdjustmentBehavior, contentContainerStyle, keyboardShouldPersistTaps, ...props }: React.ComponentProps<typeof native.ScrollView>) =>
      React.createElement(native.ScrollView, props),
    TextInput: ({ value, onChangeText, accessibilityLabel, placeholder }: React.ComponentProps<typeof native.TextInput>) =>
      React.createElement('input', { value, placeholder, 'aria-label': accessibilityLabel, onChange: (event: React.ChangeEvent<HTMLInputElement>) => onChangeText?.(event.currentTarget.value) }),
  };
});
vi.mock('expo-router', () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock('expo-router/react-navigation', () => ({ useHeaderHeight: () => 0 }));
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('expo-secure-store', () => ({ getItemAsync: vi.fn(async () => null), setItemAsync: vi.fn(async () => {}), deleteItemAsync: vi.fn(async () => {}) }));
vi.mock('expo-blur', async () => ({ BlurTargetView: (await import('react-native')).View }));
vi.mock('expo-linear-gradient', async () => ({ LinearGradient: (await import('react-native')).View }));
vi.mock('expo-file-system', () => ({ Directory: class {}, File: class {}, Paths: { cache: '/tmp' } }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => {
  const store = new Map<string, string>();
  return { getString: (k: string) => store.get(k), set: (k: string, v: string) => { store.set(k, v); }, remove: (k: string) => { store.delete(k); } };
} }));
vi.mock('sonner-native', () => ({ toast: { error: vi.fn(), success: vi.fn() }, Toaster: () => null }));
vi.mock('../../stores/aparencia', async () => {
  const { create } = await import('zustand');
  return { useAparencia: create(() => ({ fundo: 'flat', imagemUri: null, efeito: 'none', efeitoImagem: null, efeitoProcessando: false })) };
});

import ConfigIndex from '../../../app/config';
import { useServers } from '../../stores/servers';
import { useSessions, _resetSessionsForTests } from '../../stores/sessions';

const active = { id: 'settings-active', label: 'fixture-active', baseUrl: 'https://active.test', token: 'token-active' };
const other = { id: 'settings-other', label: 'fixture-other', baseUrl: 'https://other.test', token: 'token-other' };
let root: Root;
let container: HTMLDivElement;
let streams: { url: string; close: Mock<() => void>; emit: (sessions: SessionInfo[]) => void }[];
let fetchMock: Mock<(url: string, init?: RequestInit) => Promise<Response>>;
let unexpected: string[];
let diagnostics: unknown[];
let pendingLists: ((response: Response) => void)[];
const flush = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };

beforeEach(() => {
  vi.useFakeTimers();
  _resetSessionsForTests();
  _resetRotasForTests();
  streams = [];
  unexpected = [];
  diagnostics = [];
  pendingLists = [];
  configureDiag({ registrar: (entry) => { if (entry.nivel === 'erro') diagnostics.push(entry); }, novoReq: () => 'settings-connection' });
  useServers.setState({ servers: [active, other], activeId: active.id, ready: true, vaultReadable: true, aviso: null });
  configureApi({
    getBaseUrl: () => active.baseUrl, getToken: () => active.token, onUnauthorized: () => {}, origin: null,
    createEventSource: (url) => {
      const listeners = new Map<string, (event: { data: string }) => void>();
      const stream = { url, close: vi.fn(), emit: (sessions: SessionInfo[]) => listeners.get('sessions')?.({ data: JSON.stringify(sessions) }) };
      streams.push(stream);
      return { addEventListener: (type, listener) => { listeners.set(type, listener); }, removeEventListener() {}, close: stream.close, readyState: 1, onerror: null, onopen: null };
    },
  });
  fetchMock = vi.fn((url: string, init?: RequestInit) => {
    const path = new URL(url).pathname;
    if (path === '/api/peers/identificador') return Promise.resolve(new Response('{}'));
    if (path === '/api/external-pairs') return Promise.resolve(new Response('[]'));
    // Os cinco casos de cor controlam o retrato publicado, sem uma resposta HTTP sobrescrevê-lo.
    if (path === '/api/sessions') return new Promise<Response>((resolve, reject) => {
      pendingLists.push(resolve);
      init?.signal?.addEventListener('abort', () => reject(new DOMException('cancelled', 'AbortError')), { once: true });
    });
    unexpected.push(path);
    return Promise.reject(new Error(`unexpected_http:${path}`));
  });
  vi.stubGlobal('fetch', fetchMock);
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
    for (const resolve of pendingLists) resolve(new Response('[]'));
    await flush();
  });
  _resetSessionsForTests();
  _resetRotasForTests();
  configureDiag({ registrar() {}, novoReq: () => '' });
  vi.unstubAllGlobals();
  vi.useRealTimers();
  container.remove();
  expect(unexpected).toEqual([]);
  expect(diagnostics).toEqual([]);
});

describe('conexão do servidor nas configurações', () => {
  it('Config sozinha publica 25 sessões e compartilha conexão até o último consumidor sair', async () => {
    const sessions = Array.from({ length: 25 }, (_, index) => ({ name: `http-${index}`, state: 'idle', jsonl: `/tmp/${index}.jsonl` } as SessionInfo));
    const respond = fetchMock.getMockImplementation()!;
    fetchMock.mockImplementation((url: string, init?: RequestInit) => url === `${active.baseUrl}/api/sessions`
      ? Promise.resolve(new Response(JSON.stringify(sessions)))
      : url === `${other.baseUrl}/api/sessions` ? Promise.resolve(new Response('[]')) : respond(url, init));
    await act(async () => { root.render(React.createElement(ConfigIndex)); await flush(); });

    expect(useSessions.getState().rows).toHaveLength(25);
    expect(useSessions.getState().rows[0].name).toBe('http-0');
    const picker = [...container.querySelectorAll('button')].find((button) => button.textContent === active.label)!;
    expect(picker.getAttribute('aria-label')).toBe('fixture-active, Connected');
    expect([...picker.querySelectorAll('div')].find((element) => element.style.backgroundColor)?.style.backgroundColor).toBe('#0a0');
    expect(streams.map((stream) => new URL(stream.url).origin).sort()).toEqual([active.baseUrl, other.baseUrl]);

    const secondContainer = document.createElement('div');
    document.body.appendChild(secondContainer);
    const secondRoot = createRoot(secondContainer);
    try {
      await act(async () => { secondRoot.render(React.createElement(ConfigIndex)); await flush(); });
      expect(streams).toHaveLength(2);
      await act(async () => root.render(null));
      expect(streams.every((stream) => stream.close.mock.calls.length === 0)).toBe(true);
      await act(async () => {
        streams.find((stream) => stream.url.startsWith(active.baseUrl))!.emit(sessions.map((session, index) => ({ ...session, name: `live-${index}` })));
      });
      expect(useSessions.getState().rows).toHaveLength(25);
      expect(useSessions.getState().rows[0].name).toBe('live-0');
      expect(secondContainer.querySelector('[aria-label="fixture-active, Connected"]')).not.toBeNull();
      expect(streams).toHaveLength(2);
    } finally {
      await act(async () => secondRoot.unmount());
      secondContainer.remove();
    }
    expect(streams.map((stream) => stream.close.mock.calls.length)).toEqual([1, 1]);
    expect(useSessions.getState().rows).toEqual([]);
  });

  it.each([
    { name: 'lista carregada, mesmo sem sessões', bucket: { loaded: true, error: null }, label: 'fixture-active, Connected', color: '#0a0' },
    { name: 'offline com lista antiga', bucket: { loaded: true, error: 'offline' }, label: 'fixture-active offline', color: '#f00' },
    { name: 'erro real com lista antiga', bucket: { loaded: true, error: 'HTTP 503' }, label: 'fixture-active, HTTP 503', color: '#f00' },
    { name: 'conexão ainda desconhecida', bucket: { loaded: false, error: null }, label: 'fixture-active', color: '#999' },
    { name: 'sem informação do servidor ativo', bucket: null, label: 'fixture-active', color: '#999' },
  ])('mostra $name no cabeçalho', async ({ bucket, label, color }) => {
    await act(async () => root.render(React.createElement(ConfigIndex)));
    await act(async () => {
      const byServer: ServerBucket[] = [{ server: other, sessions: [], loaded: true, error: 'offline' }];
      if (bucket) byServer.push({ server: active, sessions: [], ...bucket });
      useSessions.setState({ byServer });
    });

    const picker = [...container.querySelectorAll('button')].find((button) => button.textContent === active.label);
    expect.soft(picker?.getAttribute('aria-label')).toBe(label);
    const indicator = [...picker!.querySelectorAll('div')].find((element) => element.style.backgroundColor);
    expect(indicator?.style.backgroundColor).toBe(color);
  });
});
