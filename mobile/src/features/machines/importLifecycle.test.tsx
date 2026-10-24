// @vitest-environment happy-dom
import React, { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({
  read: vi.fn(), replace: vi.fn(), discover: vi.fn(), segments: ['importar'],
  appState: null as null | ((state: string) => void), remove: vi.fn(),
  warning: vi.fn(), error: vi.fn(), screens: {} as Record<string, Record<string, unknown>>,
  connected: null as null | ((server: any) => void),
  savedFatal: undefined as string | undefined,
}));
vi.mock('expo-secure-store', () => ({ getItemAsync: mocks.read, setItemAsync: vi.fn(async () => {}) }));
vi.mock('expo-blur', async () => { const { View } = await import('react-native'); return { BlurTargetView: View, BlurView: View }; });
vi.mock('expo-linear-gradient', async () => ({ LinearGradient: (await import('react-native')).View }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('@lodev09/react-native-true-sheet', () => ({ TrueSheet: () => null }));
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('expo-file-system', () => ({ Directory: class {}, File: class {}, Paths: { cache: '/tmp', document: '/tmp' } }));
vi.mock('expo-clipboard', () => ({}));
vi.mock('expo-intent-launcher', () => ({ startActivityAsync: vi.fn() }));
vi.mock('expo-audio', () => ({ AudioModule: {}, RecordingPresets: {} }));
vi.mock('react-native-enriched-markdown', () => ({ EnrichedMarkdownText: () => null }));
vi.mock('react-native-gifted-charts', async () => ({ BarChart: (await import('react-native')).View }));
vi.mock('./importMachines', () => ({ discoverSavedMachines: mocks.discover }));
vi.mock('react-native-url-polyfill/auto', () => ({}));
vi.mock('../../theme/unistyles', () => ({}));
vi.mock('../../net/configureCore', () => ({ configureCore: () => {} }));
vi.mock('../../stores/prefs', () => ({ prefs: { getString: () => mocks.savedFatal, getBoolean: () => undefined, set: (_k: string, v: string) => { mocks.savedFatal = v; } } }));
vi.mock('../../stores/chat', () => ({ setChatsForeground: () => {} }));
vi.mock('../../stores/sessions', () => ({
  useSessions: { getState: () => ({ setForeground: () => {} }) },
  setMachineDiscovery: (fn: typeof mocks.connected) => { mocks.connected = fn; },
}));
vi.mock('../../stores/aparencia', () => ({ aplicarTemaSalvo: () => {}, setTabletTextScale: () => {}, useAparencia: Object.assign((select: (s: object) => unknown) => select({}), { getState: () => ({}) }) }));
vi.mock('../../theme/aplicarMaterial', () => ({ aplicarMaterial: () => {} }));
vi.mock('../../features/voice/LiveVoiceRoot', () => ({ LiveVoiceRoot: () => null }));
vi.mock('../../features/push/expoPush', () => ({
  installNotificationHandler: () => {}, pushRegistrationKey: () => '', useNotificationNavigation: () => {},
  registerExpoPush: async () => {}, retryExpoPushIfDenied: async () => {},
}));
vi.mock('sonner-native', () => ({ Toaster: () => null, toast: Object.assign(mocks.warning, { error: mocks.error }) }));
vi.mock('expo-router', () => ({
  useRouter: () => ({ replace: mocks.replace }), useSegments: () => mocks.segments,
  usePathname: () => `/${mocks.segments.join('/')}`, useGlobalSearchParams: () => ({}),
  DarkTheme: { dark: true, colors: { background: 'rgb(1, 1, 1)', card: 'rgb(1, 1, 1)' } },
  DefaultTheme: { dark: false, colors: { background: 'rgb(242, 242, 242)', card: 'rgb(255, 255, 255)' } },
  ThemeProvider: ({ children }: { children: ReactNode }) => <>{children}</>,
  Stack: Object.assign(({ children }: { children: ReactNode }) => <>{children}</>, { Screen: ({ name, options }: any) => { mocks.screens[name] = options; return null; } }),
}));
vi.mock('react-native-gesture-handler', () => ({ GestureHandlerRootView: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardProvider: ({ children }: { children: ReactNode }) => <>{children}</> }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  AppState: { currentState: 'active', addEventListener: (_event: string, handler: (s: string) => void) => {
    mocks.appState = handler; return { remove: mocks.remove };
  } },
}));
import Layout from '../../../app/_layout';
import { Platform } from 'react-native';

let root: Root;
let el: HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks(); mocks.segments = ['importar']; mocks.appState = null;
  mocks.savedFatal = undefined;
  mocks.read.mockResolvedValue(null); mocks.discover.mockResolvedValue({ added: [], issues: [] });
  useServers.setState({ servers: [], activeId: null, ready: false, vaultReadable: false, aviso: null });
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el);
});
afterEach(async () => { await act(async () => root.unmount()); el.remove(); });

test('erro fatal salvo mostra o caminho do relatório na próxima abertura', async () => {
  mocks.savedFatal = JSON.stringify({ report: { timestamp: '2026-10-18T10:26:00Z', message: 'Out of memory' }, pending: true });
  await act(async () => root.render(<Layout />));
  expect(mocks.warning).toHaveBeenCalledExactlyOnceWith(m.native_last_error_available(), { duration: 12_000 });
  await act(async () => root.unmount());
  root = createRoot(el);
  await act(async () => root.render(<Layout />));
  expect(mocks.warning).toHaveBeenCalledOnce();
});

test.each(['android', 'ios'] as const)('registra Git com cabeçalho inicial e apresentação adequada no %s', async (platform) => {
  const previous = Platform.OS;
  try {
    Platform.OS = platform;
    await act(async () => root.render(<Layout />));
    expect(mocks.screens['s/[server]/[name]/git']).toEqual(platform === 'android'
      ? { presentation: 'card', headerShown: true }
      : { presentation: 'formSheet', headerShown: false, sheetAllowedDetents: [0.92], sheetGrabberVisible: true, contentStyle: { backgroundColor: '#fff' } });
  } finally { Platform.OS = previous; }
});

test('espera hidratação e primeiro contato por máquina, sem buscar ao retomar com rede ainda indisponível', async () => {
  let resolve!: (raw: string) => void;
  mocks.read.mockReturnValueOnce(new Promise<string>((r) => { resolve = r; }));
  await act(async () => root.render(<Layout />));
  expect(mocks.discover).not.toHaveBeenCalled();
  const server = { id: 'saved', label: 'fixture-saved', baseUrl: 'https://saved.test', token: 'saved-token' };
  await act(async () => { resolve(JSON.stringify({ servers: [server], activeId: server.id })); });
  expect(mocks.discover).not.toHaveBeenCalled();
  await act(async () => { mocks.connected?.(server); });
  expect(mocks.discover).toHaveBeenCalledExactlyOnceWith([server]);
  await act(async () => { useServers.getState().appendImported([{ ...server, id: 'new', baseUrl: 'https://new.test' }]); });
  expect(mocks.discover).toHaveBeenCalledTimes(1);
  await act(async () => { mocks.appState?.('background'); mocks.appState?.('active'); });
  expect(mocks.discover).toHaveBeenCalledTimes(1);
  await act(async () => { mocks.connected?.(server); });
  expect(mocks.discover).toHaveBeenCalledTimes(2);
});

test('aviso de identificação mostra a causa e limpa sem erro de persistência', async () => {
  await act(async () => root.render(<Layout />));

  await act(async () => {
    useServers.setState({ aviso: { tipo: 'identification', message: '503: identity-refused' } });
  });

  expect(mocks.warning).toHaveBeenCalledExactlyOnceWith('503: identity-refused', { duration: 12_000 });
  expect(mocks.error).not.toHaveBeenCalled();
  expect(useServers.getState().aviso).toBeNull();
});

test('rota importar fica aberta sem máquina salva, mas conversa continua protegida', async () => {
  await act(async () => root.render(<Layout />));
  expect(mocks.replace).not.toHaveBeenCalled();
  mocks.segments = ['s'];
  await act(async () => root.render(<Layout />));
  expect(mocks.replace).toHaveBeenCalledWith('/login');
});
