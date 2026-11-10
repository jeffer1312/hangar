// @vitest-environment happy-dom
import React, { act, useEffect, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi, type Mock } from 'vitest';
import { configureApi, configureDiag, _resetRotasForTests, retryAfterMs, estaDesligado, type SessionInfo } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const navigation = vi.hoisted(() => ({ push: vi.fn() }));

vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  const view = ({ children, accessibilityLabel, onLayout }: {
    children?: ReactNode; accessibilityLabel?: string;
    onLayout?: (event: { nativeEvent: { layout: { width: number; height: number } } }) => void;
  }) => {
    useEffect(() => { onLayout?.({ nativeEvent: { layout: { width: 390, height: 844 } } }); }, []);
    return React.createElement('div', { 'aria-label': accessibilityLabel }, children);
  };
  return {
    ...native, View: view, Text: view, ScrollView: view,
    Pressable: ({ children, accessibilityLabel, accessibilityValue, onPress, disabled }: {
      children?: ReactNode; accessibilityLabel?: string; accessibilityValue?: { text?: string }; onPress?: () => void; disabled?: boolean;
    }) => React.createElement('button', { 'aria-label': accessibilityLabel, 'aria-valuetext': accessibilityValue?.text, onClick: onPress, disabled }, children),
    TextInput: ({ value, accessibilityLabel, onChangeText, ref, editable, onFocus, onBlur }: {
      value?: string; accessibilityLabel?: string; onChangeText?: (value: string) => void;
      ref?: React.Ref<HTMLTextAreaElement>; editable?: boolean; onFocus?: () => void; onBlur?: () => void;
    }) => React.createElement('textarea', { ref, value, 'aria-label': accessibilityLabel, readOnly: editable === false, onFocus, onBlur,
      onInput: (event: React.FormEvent<HTMLTextAreaElement>) => onChangeText?.(event.currentTarget.value) }),
    SectionList: ({ sections, renderItem, renderSectionHeader, renderSectionFooter, ListHeaderComponent, ListEmptyComponent }: {
      sections: { id: string; data: SessionInfo[] }[];
      renderItem: (args: { item: SessionInfo; section: unknown; index: number }) => ReactNode;
      renderSectionHeader: (args: { section: unknown }) => ReactNode;
      renderSectionFooter?: (args: { section: unknown }) => ReactNode;
      ListHeaderComponent?: ReactNode; ListEmptyComponent?: ReactNode;
    }) => React.createElement('div', null, ListHeaderComponent, sections.length ? sections.map((section) =>
      React.createElement('div', { key: section.id }, renderSectionHeader({ section }), section.data.map((item, index) =>
        React.createElement(React.Fragment, { key: item.name }, renderItem({ item, section, index }))), renderSectionFooter?.({ section }))) : ListEmptyComponent),
    RefreshControl: () => null,
    AccessibilityInfo: { ...native.AccessibilityInfo, sendAccessibilityEvent: vi.fn(), announceForAccessibility: vi.fn() },
    BackHandler: { addEventListener: () => ({ remove() {} }) },
    PanResponder: { create: () => ({ panHandlers: {} }) },
  };
});
vi.mock('expo-router', () => ({
  useRouter: () => ({ push: navigation.push, replace: vi.fn(), setParams: vi.fn() }),
  useLocalSearchParams: () => ({}), useIsFocused: () => true,
  useFocusEffect: (callback: () => void | (() => void)) => useEffect(callback, [callback]),
}));
vi.mock('react-native-gesture-handler/ReanimatedDrawerLayout', () => ({
  DrawerType: {}, DrawerKeyboardDismissMode: {}, DrawerLockMode: {},
  default: ({ children }: { children: (progress: { value: number }) => ReactNode }) => children({ value: 0 }),
}));
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: ReactNode }) => children }));
vi.mock('@lodev09/react-native-true-sheet', () => ({ TrueSheet: () => null }));
vi.mock('expo-secure-store', () => ({ getItemAsync: vi.fn(async () => null), setItemAsync: vi.fn(async () => {}), deleteItemAsync: vi.fn(async () => {}) }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => {
  const values = new Map<string, string | number | boolean>();
  return { getString: (key: string) => values.get(key), getNumber: (key: string) => values.get(key), getBoolean: (key: string) => values.get(key), set: (key: string, value: string | number | boolean) => values.set(key, value), remove: (key: string) => values.delete(key) };
} }));
vi.mock('expo-blur', async () => { const { View } = await import('react-native'); return { BlurTargetView: View, BlurView: View }; });
vi.mock('expo-linear-gradient', async () => ({ LinearGradient: (await import('react-native')).View }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('expo-file-system', () => ({ Directory: class {}, File: class {}, Paths: { cache: '/tmp', document: '/tmp' } }));
vi.mock('expo-image-picker', () => ({}));
vi.mock('expo-document-picker', () => ({}));
vi.mock('expo-clipboard', () => ({}));
vi.mock('expo-intent-launcher', () => ({ startActivityAsync: vi.fn() }));
vi.mock('react-native-enriched-markdown', () => ({
  EnrichedMarkdownText: ({ markdown }: { markdown: string }) => React.createElement('pre', null, markdown),
}));
vi.mock('react-native-gifted-charts', async () => ({ BarChart: (await import('react-native')).View }));
vi.mock('expo-audio', () => ({ AudioModule: {}, RecordingPresets: {} }));
vi.mock('expo-updates', () => ({ runtimeVersion: 'native-2' }));
vi.mock('react-native-keyboard-controller', async () => ({
  KeyboardAvoidingView: (await import('react-native')).View,
  useReanimatedKeyboardAnimation: () => ({ height: { value: 0 }, progress: { value: 0 } }),
  useKeyboardState: (select: (state: { isVisible: boolean; height: number }) => unknown) => select({ isVisible: false, height: 0 }),
}));
vi.mock('sonner-native', () => ({ toast: { error: vi.fn(), success: vi.fn() }, Toaster: () => null }));

import Home from '../../../app/index';
import { useServers } from '../../stores/servers';
import { useSessions, _resetSessionsForTests } from '../../stores/sessions';
import { useAparencia } from '../../stores/aparencia';
import { _resetCarriedForTests } from './NewConversation';
import { getLocale, overwriteGetLocale } from '../../paraglide/runtime';

const initialGetLocale = getLocale;

const healthy = { id: 'home-healthy', label: 'fixture-healthy', baseUrl: 'http://localhost:8765', token: 'healthy' };
const pending = { id: 'home-pending', label: 'fixture-pending', baseUrl: 'http://localhost:8766', token: 'pending' };
const session = (name: string): SessionInfo => ({ name, state: 'working', jsonl: `/tmp/${name}.jsonl` } as SessionInfo);
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
type Stream = {
  url: string; closed: boolean; close: Mock<() => void>;
  emit: (sessions: SessionInfo[]) => void;
};
let streams: Stream[];
let root: Root;
let container: HTMLDivElement;
let fetchMock: Mock<(url: string, init?: RequestInit) => Promise<Response>>;
let unexpected: string[];
let diagnostics: unknown[];
const flush = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };
const names = () => useSessions.getState().rows.map((row) => row.name).sort();
const live = () => streams.filter((stream) => !stream.closed);
const creationPaths = new Set(['/api/claude-configs', '/api/cotas', '/api/providers', '/api/engines', '/api/model-options', '/api/codex-contas', '/api/fs/roots', '/api/fs/scan', '/api/archive-por-cwd', '/api/session-defaults', '/api/worktrees']);
const creationCalls = () => fetchMock.mock.calls.map(([url]) => new URL(url).pathname).filter((path) => creationPaths.has(path)).sort();
const button = (label: string) => [...container.querySelectorAll<HTMLButtonElement>('button, div[aria-label]')]
  .find((node) => node.getAttribute('aria-label') === label || node.textContent === label);
const click = async (label: string) => {
  expect(button(label), label).toBeTruthy();
  await act(async () => { button(label)!.click(); await flush(); });
};

beforeEach(() => {
  vi.useFakeTimers();
  navigation.push.mockReset();
  overwriteGetLocale(() => 'pt');
  _resetCarriedForTests();
  _resetSessionsForTests();
  _resetRotasForTests();
  streams = [];
  unexpected = [];
  diagnostics = [];
  configureDiag({ registrar: (entry) => { if (entry.nivel === 'erro') diagnostics.push(entry); }, novoReq: () => 'home-connection' });
  useServers.setState({ servers: [healthy, pending], activeId: healthy.id, ready: true, vaultReadable: true, aviso: null });
  useAparencia.setState({ homeScreen: 'sessions', agrupar: 'server', fundo: 'flat', efeito: 'none' });
  configureApi({
    getBaseUrl: () => healthy.baseUrl, getToken: () => healthy.token, onUnauthorized: () => {}, origin: null,
    createEventSource: (url) => {
      const listeners = new Map<string, (event: { data: string }) => void>();
      const stream: Stream = { url, closed: false, close: vi.fn(() => { stream.closed = true; }), emit: (sessions) => listeners.get('sessions')?.({ data: JSON.stringify(sessions) }) };
      streams.push(stream);
      return { addEventListener: (type, listener) => { listeners.set(type, listener); }, removeEventListener() {}, close: stream.close, readyState: 1, onerror: null, onopen: null };
    },
  });
  fetchMock = vi.fn(async (url: string) => {
    const path = new URL(url).pathname;
    // Reserva pré-aquecida da conversa nova: pedida e solta sozinha enquanto a tela está aberta.
    if (path === '/api/sessions/prewarm') return new Response(JSON.stringify({ id: 'fixture-prewarm', fingerprint: 'f', name: 'project' }));
    if (path.startsWith('/api/sessions/prewarm/')) return new Response(null, { status: 204 });
    const payloads: Record<string, unknown> = {
      '/api/sessions': [session(url.startsWith(healthy.baseUrl) ? 'healthy-before' : 'pending-new')],
      '/api/claude-configs': [], '/api/cotas': [], '/api/providers': {}, '/api/engines': { motores: {} },
      '/api/external-pairs': [], '/api/config': {}, '/api/voice/settings': { codex: false },
      '/api/model-options': { models: [], reduced: false }, '/api/fs/roots': [], '/api/worktrees': { repos: [] },
      '/api/session-defaults': { configured: false, defaults: null },
    };
    if (!(path in payloads)) { unexpected.push(path); throw new Error(`unexpected_http:${path}`); }
    return new Response(JSON.stringify(payloads[path]));
  });
  vi.stubGlobal('fetch', fetchMock);
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  _resetSessionsForTests();
  _resetRotasForTests();
  configureDiag({ registrar() {}, novoReq: () => '' });
  overwriteGetLocale(initialGetLocale);
  vi.unstubAllGlobals();
  vi.useRealTimers();
  container.remove();
});

it('Home real abre sessão com outro servidor pendente sem carregar criação oculta', async () => {
  const waiting = deferred<Response>();
  const respond = fetchMock.getMockImplementation()!;
  fetchMock.mockImplementation((url: string, init?: RequestInit) =>
    url === `${pending.baseUrl}/api/sessions` ? waiting.promise : respond(url, init));
  await act(async () => { root.render(React.createElement(Home)); await flush(); });
  expect(names()).toEqual(['healthy-before']);
  expect(container.textContent).toContain('fixture-healthy');
  expect(container.textContent).toContain('fixture-pending');
  expect(live()).toHaveLength(1);
  const row = container.querySelector<HTMLButtonElement>('button[aria-label^="healthy-before,"]');
  expect(row).not.toBeNull();
  await act(async () => { row!.click(); await flush(); });
  expect(navigation.push).toHaveBeenCalledExactlyOnceWith(`/s/${healthy.id}/healthy-before`);
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); await flush(); });
  const calls = creationCalls();
  console.info('arranque-sessoes', { count: calls.length, endpoints: calls });
  expect(calls).toEqual([]);
  expect(names()).toEqual(['healthy-before']);
  expect(unexpected).toEqual([]);
  expect(diagnostics).toEqual([]);
});

it('Home real carrega criação na primeira abertura e conserva escolhas ao alternar Sessões', async () => {
  const catalog = deferred<void>();
  const respond = fetchMock.getMockImplementation()!;
  fetchMock.mockImplementation((url: string, init?: RequestInit) => {
    const path = new URL(url).pathname;
    if (path === '/api/claude-configs') return catalog.promise.then(() => new Response(JSON.stringify([
      { path: '/fixture/claude', label: 'Conta fixture', active: true },
    ])));
    if (path === '/api/model-options') return catalog.promise.then(() => new Response(JSON.stringify({
      models: [{ id: 'fixture-a', name: 'Modelo fixture A' }, { id: 'fixture-b', name: 'Modelo fixture B' }], reduced: false,
    })));
    if (path === '/api/costs') return Promise.resolve(new Response(JSON.stringify({
      applied: { period: 'all' }, totals: { input: 0, output: 0, cache_write: 0, cache_read: 0, cost_usd: 0 }, by_model: [], by_day: [],
    })));
    return respond(url, init);
  });
  await act(async () => { root.render(React.createElement(Home)); await flush(); });
  expect(creationCalls()).toEqual([]);
  const input = container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Escreva a primeira mensagem"]')!;
  expect(input).not.toBeNull();
  expect(input.readOnly).toBe(true);
  await click('Nova conversa…');
  expect(input.readOnly).toBe(false);
  expect(button('Conta Claude: Carregando…')?.disabled).toBe(true);
  expect(creationCalls()).toEqual([
    '/api/claude-configs', '/api/cotas', '/api/engines', '/api/fs/roots', '/api/model-options', '/api/providers', '/api/session-defaults', '/api/worktrees',
  ]);
  await click('Modelo, nível e permissão');
  expect(container.querySelector('[aria-label="Lendo opções…"]')).not.toBeNull();
  expect(button('Modelo fixture B')).toBeUndefined();
  await act(async () => { catalog.resolve(); await flush(); });
  expect(button('Conta Claude: Conta fixture')?.disabled).toBe(false);
  await click('Modelo fixture B');
  await click('Fechar');
  await click('Abrir pasta…');
  const path = container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Caminho do diretório"]')!;
  expect(path).not.toBeNull();
  await act(async () => {
    path.value = '/fixture/project';
    path.dispatchEvent(new Event('input', { bubbles: true }));
    await flush();
  });
  await click('Usar');
  await act(async () => {
    input.value = 'rascunho preservado';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await flush();
  });
  const loaded = [
    '/api/claude-configs', '/api/cotas', '/api/engines', '/api/fs/roots', '/api/model-options', '/api/model-options', '/api/providers', '/api/session-defaults', '/api/worktrees',
  ];
  expect(creationCalls()).toEqual(loaded);
  console.info('criacao-aberta', { count: creationCalls().length, endpoints: creationCalls() });
  await click('Sessões');
  expect(input.readOnly).toBe(true);
  expect(container.textContent).toContain('healthy-before');
  await click('Nova conversa…');
  await act(async () => { await vi.advanceTimersByTimeAsync(1000); await flush(); });
  expect(container.querySelector('textarea[aria-label="Escreva a primeira mensagem"]')).toBe(input);
  expect(input.value).toBe('rascunho preservado');
  expect(document.activeElement).toBe(input);
  expect(button('Máquina: fixture-healthy')).toBeTruthy();
  expect(button('Pasta: project')).toBeTruthy();
  expect(button('Conta Claude: Conta fixture')).toBeTruthy();
  expect(button('Modelo, nível e permissão')?.getAttribute('aria-valuetext')).toBe('Modelo fixture B');
  expect(button('Enviar')?.disabled).toBe(false);
  console.info('criacao-reaberta', { count: creationCalls().length, endpoints: creationCalls() });
  expect(creationCalls()).toEqual(loaded);
  expect(unexpected).toEqual([]);
  expect(diagnostics).toEqual([]);
});

it('Home real conserva caminho manual não confirmado ao alternar Sessões sem reler raízes', async () => {
  await act(async () => { root.render(React.createElement(Home)); await flush(); });
  await click('Nova conversa…');
  await click('Abrir pasta…');
  const manual = container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Caminho do diretório"]')!;
  expect(manual).not.toBeNull();
  await act(async () => {
    manual.value = '/fixture/unfinished';
    manual.dispatchEvent(new Event('input', { bubbles: true }));
    await flush();
  });
  expect(manual.value).toBe('/fixture/unfinished');
  const rootsBefore = creationCalls().filter((path) => path === '/api/fs/roots').length;
  expect(rootsBefore).toBe(1);
  await click('Sessões');
  await click('Nova conversa…');
  const rootsAfter = creationCalls().filter((path) => path === '/api/fs/roots').length;
  console.info('pasta-nao-confirmada', { endpoint: '/api/fs/roots', before: rootsBefore, after: rootsAfter });
  const reopened = container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Caminho do diretório"]');
  expect(reopened).toBe(manual);
  expect(reopened?.value).toBe('/fixture/unfinished');
  expect(rootsAfter).toBe(1);
  expect(unexpected).toEqual([]);
  expect(diagnostics).toEqual([]);
});

it.each(['GET', 'json'] as const)('Home real recupera os dois servidores ao remontar sem aguardar %s antigo', async (phase) => {
  const old = deferred<Response>();
  const oldJson = deferred<SessionInfo[]>();
  let oldSignal: AbortSignal | undefined;
  const respond = fetchMock.getMockImplementation()!;
  let first = true;
  fetchMock.mockImplementation((url: string, init?: RequestInit) => {
    if (url === `${pending.baseUrl}/api/sessions` && first) {
      first = false;
      oldSignal = init?.signal as AbortSignal;
      return phase === 'GET' ? old.promise : Promise.resolve({ ok: true, status: 200, json: () => oldJson.promise } as Response);
    }
    return respond(url, init);
  });
  await act(async () => { root.render(React.createElement(Home)); await flush(); });
  expect(names()).toEqual(['healthy-before']);
  expect(live()).toHaveLength(1);
  expect(container.textContent).toContain('healthy-before');

  await act(async () => root.unmount());
  expect(oldSignal?.aborted).toBe(false);
  expect(live()).toHaveLength(0);
  root = createRoot(container);
  await act(async () => { root.render(React.createElement(Home)); await flush(); });

  expect(names()).toEqual(['healthy-before', 'pending-new']);
  expect(live().map((stream) => new URL(stream.url).origin).sort()).toEqual([healthy.baseUrl, pending.baseUrl]);
  expect(container.textContent).toContain('pending-new');
  const currentStreams = live();
  await act(async () => {
    currentStreams[0].emit([session('healthy-live')]);
    currentStreams[1].emit([session('pending-live')]);
    old.resolve(new Response(JSON.stringify([session('obsolete')])));
    oldJson.resolve([session('obsolete')]);
    await flush();
  });
  expect(names()).toEqual(['healthy-live', 'pending-live']);
  expect(live()).toEqual(currentStreams);
  expect(streams).toHaveLength(3);
  expect(unexpected).toEqual([]);
  expect(diagnostics).toEqual([]);
});

it('Home real remonta sem abortar GET antigo nem gerar queda falsa', async () => {
  const old = deferred<Response>();
  const next = deferred<Response>();
  const signals: AbortSignal[] = [];
  const respond = fetchMock.getMockImplementation()!;
  fetchMock.mockImplementation((url: string, init?: RequestInit) => {
    if (url === `${pending.baseUrl}/api/sessions` && signals.length < 2) {
      const signal = init?.signal as AbortSignal;
      signals.push(signal);
      if (signals.length === 1) {
        signal.addEventListener('abort', () => old.reject(new TypeError('cancelled_fetch')), { once: true });
        return old.promise;
      }
      return next.promise;
    }
    return respond(url, init);
  });
  try {
    await act(async () => { root.render(React.createElement(Home)); await flush(); });
    expect(names()).toEqual(['healthy-before']);
    await act(async () => root.unmount());
    expect.soft(signals[0].aborted).toBe(false);
    expect.soft(retryAfterMs(pending.id)).toBe(0);
    expect.soft(estaDesligado(pending.id)).toBe(false);
    root = createRoot(container);
    await act(async () => { root.render(React.createElement(Home)); await flush(); });
    expect(signals).toHaveLength(2);
    expect(signals[1].aborted).toBe(false);
    expect(useSessions.getState().byServer.find((bucket) => bucket.server.id === pending.id)?.error).toBeNull();

    await act(async () => { next.resolve(new Response(JSON.stringify([session('pending-new')]))); await flush(); });
    expect(names()).toEqual(['healthy-before', 'pending-new']);
    const currentStreams = live();
    expect(currentStreams.map((stream) => new URL(stream.url).origin).sort()).toEqual([healthy.baseUrl, pending.baseUrl]);
    await act(async () => { old.resolve(new Response(JSON.stringify([session('obsolete')]))); await flush(); });
    expect(names()).toEqual(['healthy-before', 'pending-new']);
    expect(live()).toEqual(currentStreams);
    expect(retryAfterMs(pending.id)).toBe(0);
    expect(estaDesligado(pending.id)).toBe(false);
    expect(unexpected).toEqual([]);
    expect(diagnostics).toEqual([]);
  } finally {
    await act(async () => {
      old.resolve(new Response('[]'));
      next.resolve(new Response('[]'));
      await flush();
    });
  }
});
