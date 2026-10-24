// @vitest-environment happy-dom
import { act, createElement, useEffect, useImperativeHandle, useRef, useState, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { create } from 'zustand';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { ChatState } from '../stores/chat';
import type { ChatBodyProps } from './ChatBody';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const h = vi.hoisted(() => ({
  size: { width: 390, height: 844, scale: 1, fontScale: 1 },
  os: 'android',
  retain: vi.fn(), release: vi.fn(), bodyMounts: 0,
  session: { serverId: 'srv', name: 'chat', provider: 'claude', tracked: true, state: 'idle' },
  router: { push: vi.fn(), replace: vi.fn(), dismissTo: vi.fn(), canGoBack: () => false, back: vi.fn() },
  navigation: { getState: () => ({ routes: [{ key: 'home' }, { key: 'chat' }] }), addListener: () => () => {} },
}));

vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(), useWindowDimensions: () => h.size,
  Platform: { get OS() { return h.os; }, select: (options: Record<string, unknown>) => options[h.os] ?? options.default },
  Pressable: ({ children, onPress, accessibilityLabel, accessibilityState }: { children?: ReactNode; onPress?: () => void; accessibilityLabel?: string; accessibilityState?: { expanded?: boolean } }) =>
    createElement('button', { onClick: onPress, 'aria-label': accessibilityLabel, 'aria-expanded': accessibilityState?.expanded }, children),
}));
vi.mock('expo-router', () => ({
  useRouter: () => h.router, useIsFocused: () => true,
  useLocalSearchParams: () => ({ server: 'srv', name: 'chat' }),
  useGlobalSearchParams: () => ({ server: 'srv', name: 'chat' }),
  usePathname: () => '/s/srv/chat', useSegments: () => ['s', '[server]', '[name]'],
  DarkTheme: { dark: true, colors: { background: 'rgb(1, 1, 1)', card: 'rgb(1, 1, 1)' } },
  DefaultTheme: { dark: false, colors: { background: 'rgb(242, 242, 242)', card: 'rgb(255, 255, 255)' } },
  ThemeProvider: ({ children }: { children: ReactNode }) => children,
  Stack: Object.assign(() => createElement(ChatScreen), { Screen: () => null }),
  useNavigation: () => h.navigation, useRoute: () => ({ key: 'chat' }),
  useFocusEffect: (effect: () => void | (() => void)) => useEffect(effect, [effect]),
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(), fetchSessionsForServer: async () => [h.session],
}));
vi.mock('../stores/chat', () => {
  const use = create<Partial<ChatState>>(() => ({
    events: [], eventsRevision: 0, pending: [], loading: false, error: '',
    stateEvent: { session: 'chat', state: 'idle' }, askOpen: false,
  }));
  const chat = { use, retain: h.retain, release: h.release };
  return { chatStore: () => chat, setChatsForeground: () => {} };
});
vi.mock('../stores/servers', () => {
  const state = { ready: true, servers: [{ id: 'srv', label: 'local' }], ensureActive: () => true, ensureOwnActive: () => {},
    load: async () => {}, limparAviso: () => {}, aviso: null };
  return { useServers: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) };
});
vi.mock('../stores/sessions', () => {
  const state = { rows: [h.session], retain: () => () => {}, setForeground: () => {} };
  return { setMachineDiscovery: () => {}, useSessions: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) };
});
vi.mock('../stores/newConversation', () => ({
  useNewConversation: (select: (s: unknown) => unknown) => select({ attempts: {}, busy: {}, issues: {} }),
  readFirstInput: () => null, confirmFirstInput: () => {},
}));
vi.mock('../features/voice/liveVoiceStore', () => ({ useLiveVoice: { getState: () => ({ registerChat: () => () => {} }) } }));
vi.mock('../features/activity/useActivity', () => ({ useActivity: () => ({ agents: [] }) }));
vi.mock('expo-blur', () => ({ BlurTargetView: ({ children, ref }: { children: ReactNode; ref: React.Ref<HTMLDivElement> }) =>
  createElement('div', { ref }, children) }));
vi.mock('../ui/Background', () => ({ Background: () => createElement('div', { 'data-wallpaper': true }) }));
vi.mock('react-native-url-polyfill/auto', () => ({}));
vi.mock('../theme/unistyles', () => ({}));
vi.mock('../theme/aplicarMaterial', () => ({ aplicarMaterial: () => {} }));
vi.mock('../net/configureCore', () => ({ configureCore: () => {} }));
vi.mock('../features/machines/importMachines', () => ({ discoverSavedMachines: () => {} }));
vi.mock('./useQueueDelivery', () => ({ setQueueDeliveryForeground: () => {} }));
vi.mock('../net/fatalErrors', () => ({ setFatalErrorRoute: () => {}, takeFatalErrorNotice: () => false }));
vi.mock('../features/push/expoPush', () => ({ installNotificationHandler: () => {}, pushRegistrationKey: () => '',
  registerExpoPush: async () => {}, retryExpoPushIfDenied: async () => {}, useNotificationNavigation: () => {} }));
vi.mock('../features/voice/LiveVoiceRoot', () => ({ LiveVoiceRoot: () => null }));
vi.mock('../ui/Toast', () => ({ Toaster: () => null, toast: { aviso: vi.fn(), erro: vi.fn() } }));
vi.mock('../stores/aparencia', () => {
  const state = { idioma: 'pt' };
  return { aplicarTemaSalvo: () => {}, setTabletTextScale: () => {},
    useAparencia: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) };
});
vi.mock('react-native-gesture-handler', () => ({
  GestureDetector: ({ children }: { children: ReactNode }) => children,
  GestureHandlerRootView: ({ children }: { children: ReactNode }) => children,
  Gesture: {
    Pan: () => {
      const pan = { enabled: () => pan, runOnJS: () => pan, hitSlop: () => pan, activeOffsetX: () => pan, failOffsetY: () => pan, onEnd: () => pan };
      return pan;
    },
  },
}));
vi.mock('react-native-gesture-handler/ReanimatedDrawerLayout', () => ({
  DrawerType: { FRONT: 0 }, DrawerKeyboardDismissMode: { ON_DRAG: 0 }, DrawerLockMode: { UNLOCKED: 0, LOCKED_CLOSED: 1 },
  default: ({ ref, children, edgeWidth, renderNavigationView }: {
    ref: React.Ref<unknown>; children: (progress: { value: number }) => ReactNode;
    edgeWidth: number; renderNavigationView: (progress: { value: number }) => ReactNode;
  }) => {
    useImperativeHandle(ref, () => ({ closeDrawer: () => {}, openDrawer: () => {} }));
    const navigation = renderNavigationView({ value: 0 });
    return createElement('div', { 'data-drawer-enabled': String(edgeWidth > 0), 'data-drawer-navigation': String(navigation !== null) },
      navigation, children({ value: 0 }));
  },
}));
vi.mock('../features/sessions/DrawerMenu', () => ({ DrawerMenu: () => null }));
vi.mock('../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => children }));
vi.mock('../features/tablet/SessionsBar', () => ({ SessionsBar: () => createElement('nav', { 'data-tablet-bar': true }) }));
vi.mock('../features/tablet/ContextPanel', () => ({ ContextPanel: () => createElement('aside', null, 'session-context') }));
vi.mock('../ui/HangarMark', () => ({ HangarMark: () => null }));
vi.mock('../stores/tabletPrefs', () => {
  const useTabletPrefs = create(() => ({ placement: 'bottom', panelOpen: true,
    togglePanel: () => useTabletPrefs.setState(s => ({ panelOpen: !s.panelOpen })),
  }));
  return { useTabletPrefs };
});
vi.mock('../features/sessions/ServerSheet', () => ({ ServerSheet: () => null }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => children,
  KeyboardProvider: ({ children }: { children: ReactNode }) => children }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../ui/EdgeButton', () => ({ EdgeButton: ({ children, onPress, accessibilityLabel }: { children: ReactNode; onPress: () => void; accessibilityLabel: string }) =>
  createElement('button', { onClick: onPress, 'aria-label': accessibilityLabel }, children) }));
vi.mock('../ui/WorkingMark', async () => ({ WorkingMarkPaused: (await import('react')).createContext(false), WorkingMark: () => null }));
vi.mock('../features/sessions/SessionAvatar', () => ({ SessionAvatar: () => null }));
vi.mock('../features/voice/LiveVoiceChatButton', () => ({ LiveVoiceChatButton: () => null }));
vi.mock('./BubbleActions', () => ({ pararTts: () => {} }));
vi.mock('./LoopChip', () => ({ LoopChip: () => null }));
vi.mock('./MessageList', () => ({ MessageList: ({ maxReading }: { maxReading?: number }) => createElement('section', { 'data-max-reading': maxReading }) }));
vi.mock('./ComposerField', () => ({
  ComposerDock: ({ children }: { children: ReactNode }) => children,
  ComposerViewport: ({ children }: { children: ReactNode }) => children,
}));
vi.mock('./Composer', () => ({ Composer: function DraftField() {
  const [text, setText] = useState('');
  return createElement('textarea', { value: text, onInput: (e: React.FormEvent<HTMLTextAreaElement>) => setText(e.currentTarget.value) });
} }));
vi.mock('./ComposerStatusLine', () => ({ ComposerStatusLine: () => null }));
vi.mock('./OrqFooter', () => ({ OrqFooter: () => null }));
vi.mock('./TuiPill', () => ({ TuiPill: () => null }));
vi.mock('./RecarregarPill', () => ({ RecarregarPill: () => null }));
vi.mock('./MoreSheet', () => ({ MoreSheet: () => null }));
vi.mock('./PluginUi', () => ({ PluginUi: () => null }));
vi.mock('./OptionButtons', () => ({ OptionButtons: () => null }));
vi.mock('./PendingPlan', () => ({ PendingPlan: () => null }));
vi.mock('./SessionPickerSheet', () => ({ SessionPickerSheet: () => null }));
vi.mock('./PlanActions', () => ({ PlanActions: () => null }));
vi.mock('./ChatBody', async (original) => {
  const { ChatBody } = await original<typeof import('./ChatBody')>();
  return { ChatBody: function ObservedBody(props: ChatBodyProps) {
    const instance = useRef<number | null>(null);
    if (instance.current === null) instance.current = ++h.bodyMounts;
    return createElement('div', { 'data-body-instance': instance.current }, createElement(ChatBody, props));
  } };
});

import ChatScreen from '../../app/s/[server]/[name]/index';
import { ChatBody } from './ChatBody';
import * as m from '../paraglide/messages';
import { useTabletPrefs } from '../stores/tabletPrefs';
import Layout from '../../app/_layout';

let root: Root;
let host: HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks();
  h.size = { width: 390, height: 844, scale: 1, fontScale: 1 };
  h.os = 'android';
  h.bodyMounts = 0;
  useTabletPrefs.setState({ panelOpen: true });
  host = document.createElement('div');
  root = createRoot(host);
});
afterEach(() => act(() => root.unmount()));

it.each(['android', 'ios'])('%s: girar e estreitar a janela conserva o ChatBody, o rascunho e a conexão', async os => {
  h.os = os;
  await act(async () => root.render(createElement(Layout)));
  expect(host.querySelector('[data-body-instance]')).not.toBeNull();
  const instance = host.querySelector('[data-body-instance]')!.getAttribute('data-body-instance');
  const field = host.querySelector('textarea')!;
  act(() => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, 'rascunho ao girar');
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  expect(field.value).toBe('rascunho ao girar');
  for (const size of [{ width: 1180, height: 820 }, { width: 507, height: 820 }, { width: 820, height: 1180 }]) {
    h.size = { ...h.size, ...size };
    await act(async () => root.render(createElement(Layout)));
    const tablet = size.width === 1180;
    expect(host.querySelector('[data-tablet-bar]') !== null).toBe(tablet);
    expect(host.querySelectorAll('[data-tablet-bar]')).toHaveLength(tablet ? 1 : 0);
    expect(host.querySelector('[data-drawer-enabled]')!.getAttribute('data-drawer-enabled')).toBe(String(!tablet));
    expect(host.querySelector('[data-drawer-navigation]')!.getAttribute('data-drawer-navigation')).toBe(String(!tablet));
    expect(host.querySelector('[data-body-instance]')!.getAttribute('data-body-instance')).toBe(instance);
    expect(host.querySelector('textarea')).toBe(field);
    expect(host.querySelector('textarea')!.value).toBe('rascunho ao girar');
    expect(h.retain).toHaveBeenCalledTimes(1);
    expect(h.release).not.toHaveBeenCalled();
    expect(h.bodyMounts).toBe(1);
    expect(host.querySelectorAll('[data-wallpaper]')).toHaveLength(1);
  }
});

it('cabeçalho de tablet troca a navegação pelo painel sem remontar o campo', async () => {
  const onTogglePanel = vi.fn();
  const onOpenMore = vi.fn();
  const props: ChatBodyProps = { serverId: 'srv', name: 'chat', onBack: vi.fn(), deepStack: true, onTogglePanel, onOpenMore };
  await act(async () => root.render(createElement(ChatBody, props)));
  const field = host.querySelector('textarea');
  expect(host.querySelector(`[aria-label="${m.chat_voltar_sessoes()}"]`)).not.toBeNull();
  expect(host.querySelector(`[aria-label="${m.ctx_painel_titulo()}"]`)).toBeNull();
  await act(async () => root.render(createElement(ChatBody, { ...props, header: 'tablet' })));
  const panel = host.querySelector<HTMLButtonElement>(`[aria-label="${m.ctx_painel_titulo()}"]`);
  expect(panel).not.toBeNull();
  expect(host.querySelector(`[aria-label="${m.chat_voltar_sessoes()}"]`)).toBeNull();
  expect(host.querySelector(`[aria-label="${m.chat_ir_inicio()}"]`)).toBeNull();
  act(() => panel!.click());
  act(() => host.querySelector<HTMLButtonElement>(`[aria-label="${m.navbar_mais_acoes()}"]`)!.click());
  expect(onTogglePanel).toHaveBeenCalledTimes(1);
  expect(onOpenMore).toHaveBeenCalledTimes(1);
  expect(host.querySelector('textarea')).toBe(field);
  expect(h.retain).toHaveBeenCalledTimes(1);
});

it('a rota transmite painel aberto e limita a leitura só ao fechar o painel no tablet', async () => {
  h.size = { ...h.size, width: 1180, height: 820 };
  await act(async () => root.render(createElement(Layout)));
  const panel = () => host.querySelector(`[aria-label="${m.ctx_painel_titulo()}"]`);
  expect(panel()?.getAttribute('aria-expanded')).toBe('true');
  expect(host.querySelector('section')?.getAttribute('data-max-reading')).toBeNull();
  act(() => (panel() as HTMLButtonElement).click());
  expect(panel()?.getAttribute('aria-expanded')).toBe('false');
  expect(host.querySelector('aside')).toBeNull();
  expect(host.querySelector('section')?.getAttribute('data-max-reading')).toBe('760');
  h.size = { ...h.size, width: 390, height: 844 };
  await act(async () => root.render(createElement(Layout)));
  expect(host.querySelector('section')?.getAttribute('data-max-reading')).toBeNull();
});
