// @vitest-environment happy-dom
import { act, createElement, useEffect, useRef, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { SessionsBarPlacement } from '../../stores/tabletPrefs';
import * as ScreenOrientation from 'expo-screen-orientation';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const h = vi.hoisted(() => ({
  placement: 'bottom' as SessionsBarPlacement, panelOpen: true, collapsed: false, toggleCollapsed: vi.fn(),
  size: { width: 1180, height: 820, scale: 1, fontScale: 1 },
  pathname: '/s/srv/a', params: { server: 'srv', name: 'a' }, barMounts: 0, panelMounts: 0,
  dimensionsListener: vi.fn(), removeDimensionsListener: vi.fn(),
  router: { replace: vi.fn(), push: vi.fn(), dismissTo: vi.fn(), setParams: vi.fn(), canGoBack: () => true, back: vi.fn() },
}));
vi.mock('expo-router', () => ({
  useRouter: () => h.router, useIsFocused: () => true, useLocalSearchParams: () => h.params,
  useGlobalSearchParams: () => h.params, usePathname: () => h.pathname,
  DarkTheme: { dark: true, colors: { background: 'rgb(1, 1, 1)', card: 'rgb(1, 1, 1)' } },
  DefaultTheme: { dark: false, colors: { background: 'rgb(242, 242, 242)', card: 'rgb(255, 255, 255)' } },
  ThemeProvider: ({ children, value }: { children: ReactNode; value: { colors: { background: string } } }) =>
    createElement('div', { 'data-navigation-background': value.colors.background }, children),
  useSegments: () => h.pathname.startsWith('/s/') ? ['s', '[server]', '[name]', ...h.pathname.split('/').slice(4)] : h.pathname.split('/').filter(Boolean),
  useNavigation: () => ({ getState: () => ({ routes: [] }), addListener: () => () => {} }), useRoute: () => ({ key: h.pathname }),
  Stack: Object.assign(({ children, screenOptions }: { children: ReactNode; screenOptions: unknown }) => {
    const content = h.pathname.startsWith('/s/') ? createElement(ChatScreen, { key: h.pathname })
      : h.pathname === '/' ? createElement(Index) : createElement('main', { 'data-page': h.pathname });
    return createElement('div', { 'data-stack': true, 'data-options': JSON.stringify(screenOptions) }, children, content);
  }, { Screen: ({ name, options }: { name: string; options?: unknown }) => createElement('i', { 'data-screen': name, 'data-options': JSON.stringify(options) }) }),
  useFocusEffect: (effect: () => void | (() => void)) => useEffect(effect, [effect]),
}));
vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return { ...native,
    Dimensions: { ...native.Dimensions, addEventListener: h.dimensionsListener },
    useWindowDimensions: () => h.size,
    PanResponder: { create: () => ({ panHandlers: {} }) },
  };
});
vi.mock('../../stores/tabletPrefs', () => ({ useTabletPrefs: (select: (s: typeof h) => unknown) => select(h) }));
vi.mock('../../stores/servers', () => {
  const state = { activeId: 'srv', ready: true, servers: [{ id: 'srv' }], aviso: null, limparAviso: () => {}, load: async () => {} };
  return { useServers: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) };
});
vi.mock('../../stores/sessions', () => {
  const state = { rows: [], retain: () => () => {}, setForeground: () => {} };
  return { setMachineDiscovery: () => {}, useSessions: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) };
});
vi.mock('../../stores/aparencia', () => {
  const state = { homeScreen: 'sessions', idioma: 'pt' };
  return { aplicarTemaSalvo: () => {}, setTabletTextScale: () => {},
    useAparencia: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) };
});
vi.mock('react-native-url-polyfill/auto', () => ({}));
vi.mock('../../theme/unistyles', () => ({}));
vi.mock('../../theme/aplicarMaterial', () => ({ aplicarMaterial: () => {} }));
vi.mock('../../net/configureCore', () => ({ configureCore: () => {} }));
vi.mock('../machines/importMachines', () => ({ discoverSavedMachines: () => {} }));
vi.mock('../../stores/chat', () => ({ setChatsForeground: () => {} }));
vi.mock('../../chat/useQueueDelivery', () => ({ setQueueDeliveryForeground: () => {} }));
vi.mock('../../net/fatalErrors', () => ({ setFatalErrorRoute: () => {}, takeFatalErrorNotice: () => false }));
vi.mock('../push/expoPush', () => ({ installNotificationHandler: () => {}, pushRegistrationKey: () => '',
  registerExpoPush: async () => {}, retryExpoPushIfDenied: async () => {}, useNotificationNavigation: () => {} }));
vi.mock('../voice/LiveVoiceRoot', () => ({ LiveVoiceRoot: () => null }));
vi.mock('../../ui/Toast', () => ({ Toaster: () => null, toast: { aviso: vi.fn(), erro: vi.fn() } }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardProvider: ({ children }: { children: ReactNode }) => children }));
vi.mock('react-native-gesture-handler', async original => ({ ...await original<typeof import('react-native-gesture-handler')>(),
  GestureHandlerRootView: ({ children }: { children: ReactNode }) => children }));
vi.mock('../../chat/ChatBody', () => ({ ChatBody: () => createElement('article', null, 'conversation') }));
vi.mock('../files/FilesPanel', () => ({ FilesPanel: () => null }));
vi.mock('../activity/ActivityPanel', () => ({ ActivityPanel: () => null }));
vi.mock('../git/GitPanel', () => ({ GitPanel: ({ name }: { name: string }) => createElement('section', { 'data-git-session': name }) }));
vi.mock('./ContextTab', () => ({ ContextTab: ({ name }: { name: string }) => createElement('section', { 'data-context-session': name }) }));
vi.mock('../../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => createElement('aside', null, children) }));
vi.mock('../sessions/SessionsDrawer', () => ({ SessionsDrawer: ({ children }: { children: ReactNode }) => children }));
vi.mock('../sessions/ServerSheet', () => ({ ServerSheet: () => null }));
vi.mock('../sessions/SessionList', () => ({ SessionList: () => createElement('section', { 'data-home-sessions': true }) }));
vi.mock('../create/CreateSessionSheet', () => ({ CreateSessionSheet: ({ home }: { home: { mode: string; sessions: ReactNode } }) =>
  createElement('div', { 'data-create-mode': home.mode }, home.sessions, createElement('textarea')) }));
vi.mock('../../ui/Screen', () => ({ Screen: ({ children }: { children: ReactNode }) => children }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/HangarMark', () => ({ HangarMark: () => null }));
vi.mock('../voice/LiveVoiceChatButton', () => ({ LiveVoiceChatButton: () => null }));
vi.mock('./SessionsBar', () => ({ SessionsBar: ({ onPick, onNew, selected }: {
  onPick: (s: unknown) => void; onNew: () => void; selected?: { name: string };
}) => {
  const instance = useRef<number | null>(null);
  if (instance.current === null) instance.current = ++h.barMounts;
  return createElement('nav', { 'data-instance': instance.current, 'data-selected': selected?.name },
    createElement('button', { onClick: () => onPick({ serverId: 'srv', name: 'b' }) }, 'pick-b'),
    createElement('button', { onClick: onNew }, 'new-session'));
} }));
vi.mock('./ContextPanel', () => ({ ContextPanel: ({ name, focused, onPick }: { name: string; focused: boolean; onPick: (s: unknown) => void }) => {
  const instance = useRef<number | null>(null);
  if (instance.current === null) instance.current = ++h.panelMounts;
  return createElement('aside', { 'data-instance': instance.current, 'data-selected': name, 'data-focused': focused }, 'session-context',
    createElement('button', { onClick: () => onPick({ serverId: 'other-machine', name: 'pair name' }) }, 'pick-group-member'));
} }));
import { TabletShell } from './TabletShell';
import Index from '../../../app/index';
import ChatScreen from '../../../app/s/[server]/[name]/index';
import Layout from '../../../app/_layout';
import * as m from '../../paraglide/messages';
let root: Root;
let host: HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks(); h.placement = 'bottom'; h.panelOpen = true; h.collapsed = false;
  h.dimensionsListener.mockReturnValue({ remove: h.removeDimensionsListener });
  h.pathname = '/s/srv/a'; h.params = { server: 'srv', name: 'a' }; h.barMounts = 0; h.panelMounts = 0;
  h.size = { width: 1180, height: 820, scale: 1, fontScale: 1 };
  host = document.createElement('div'); root = createRoot(host);
});
afterEach(() => act(() => root.unmount()));
function render(selected: { serverId: string; name: string } | undefined = { serverId: 'srv', name: 'a' }) {
  act(() => root.render(createElement(TabletShell, { selected, children: createElement('main', null, 'conversation') })));
}
function click(label: string) {
  const button = [...host.querySelectorAll('button')].find(b => b.textContent === label || b.getAttribute('aria-label') === label)!;
  expect(button).toBeDefined(); act(() => button.click());
}
it('reaplica a orientação pela tela física ao abrir e fechar o dobrável e remove o observador', async () => {
  const lock = vi.spyOn(ScreenOrientation, 'lockAsync');
  const unlock = vi.spyOn(ScreenOrientation, 'unlockAsync');
  await act(async () => root.render(createElement(Layout)));
  expect(lock).toHaveBeenCalledExactlyOnceWith(ScreenOrientation.OrientationLock.PORTRAIT_UP);
  expect(h.dimensionsListener).toHaveBeenCalledWith('change', expect.any(Function));
  const change = h.dimensionsListener.mock.calls[0][1];
  const phone = { width: 390, height: 844, scale: 1, fontScale: 1 };
  const tablet = { ...phone, width: 1280, height: 800 };
  await act(async () => change({ screen: tablet, window: phone }));
  expect(unlock).toHaveBeenCalledOnce();
  await act(async () => change({ screen: phone, window: tablet }));
  expect(lock).toHaveBeenCalledTimes(2);
  act(() => root.unmount());
  expect(h.removeDimensionsListener).toHaveBeenCalledOnce();
});

it.each(['/s/srv/a/files', '/s/srv/a/ask', '/s/srv/a/activity', '/config'])('pausa o painel quando %s cobre a conversa e retoma ao voltar', pathname => {
  render();
  const panel = host.querySelector('aside')!;
  expect(panel.getAttribute('data-focused')).toBe('true');
  h.pathname = pathname; render();
  expect(panel.getAttribute('data-focused')).toBe('false');
  h.pathname = '/s/srv/a'; render();
  expect(host.querySelector('aside')).toBe(panel);
  expect(panel.getAttribute('data-focused')).toBe('true');
});
it.each(['left', 'bottom', 'top'] as const)('%s ordena as áreas como no mock e mantém o centro transparente', placement => {
  h.placement = placement; render();
  const center = host.querySelector('main')!;
  const bar = host.querySelector('nav')!;
  const panel = host.querySelector('aside')!;
  expect(center).not.toBeNull(); expect(bar).not.toBeNull(); expect(panel).not.toBeNull();
  expect(!!(bar.compareDocumentPosition(center) & Node.DOCUMENT_POSITION_FOLLOWING)).toBe(placement !== 'bottom');
  expect(!!(center.compareDocumentPosition(panel) & Node.DOCUMENT_POSITION_FOLLOWING)).toBe(true);
  // A faixa horizontal abrange também o painel, como no mock.
  if (placement === 'top' || placement === 'bottom') expect(bar.parentElement!.contains(panel)).toBe(false);
  expect(center.parentElement!.style.backgroundColor).toBe('');
  expect(center.parentElement!.style.flex).toBe('1 1 0%');
});
it('fechar o painel desmonta o contexto', () => {
  render(); expect(host.querySelector('aside')).not.toBeNull();
  h.panelOpen = false; render(); expect(host.querySelector('aside')).toBeNull();
});

it.each(['top', 'bottom'] as const)('%s recolhida desmonta a faixa e oferece expansão no topo com 44 pt', placement => {
  h.placement = placement; render();
  const center = host.querySelector('main');
  expect(host.querySelector(`[aria-label="${m.tablet_bar_expand()}"]`)).toBeNull();
  h.collapsed = true; render();
  expect(host.querySelector('nav')).toBeNull();
  const button = host.querySelector<HTMLButtonElement>(`[aria-label="${m.tablet_bar_expand()}"]`)!;
  expect(button.style.minWidth).toBe('44px');
  expect(button.style.minHeight).toBe('44px');
  click(m.tablet_bar_expand()); expect(h.toggleCollapsed).toHaveBeenCalledOnce();
  h.collapsed = false; render();
  expect(host.querySelector('nav')).not.toBeNull();
  expect(host.querySelector('main')).toBe(center);
});

it('esquerda recolhida conserva a barra sem duplicar o botão de expansão no topo', () => {
  h.placement = 'left'; h.collapsed = true; render();
  expect(host.querySelector('nav')).not.toBeNull();
  expect(host.querySelector(`[aria-label="${m.tablet_bar_expand()}"]`)).toBeNull();
});

it('em pé não monta a barra nem o botão de expansão', () => {
  h.collapsed = true;
  act(() => root.render(createElement(TabletShell, { active: false, children: createElement('main') })));
  expect(host.querySelector('nav')).toBeNull();
  expect(host.querySelector(`[aria-label="${m.tablet_bar_expand()}"]`)).toBeNull();
});
it('nova conversa no Início não monta o painel', () => {
  act(() => root.render(createElement(TabletShell, { children: createElement('main', null, 'new-conversation') })));
  expect(host.querySelector('aside')).toBeNull();
});
it('troca a sessão sem empilhar e abre criação, busca, configuração e Início', () => {
  render(); click('pick-b'); expect(h.router.replace).toHaveBeenCalledWith('/s/srv/b');
  click('new-session'); expect(h.router.push).toHaveBeenCalledWith('/create');
  click(m.lista_buscar()); expect(h.router.push).toHaveBeenCalledWith('/search');
  click(m.config_modal_titulo()); expect(h.router.push).toHaveBeenCalledWith('/config');
  click(m.chat_ir_inicio()); expect(h.router.dismissTo).toHaveBeenCalledWith('/');
});

it.each(['/s/srv/a', '/s/srv/a/files'])('o membro do painel usa a navegação da barra a partir de %s', pathname => {
  h.pathname = pathname;
  render();
  click('pick-group-member');
  const navigate = pathname.endsWith('/files') ? h.router.dismissTo : h.router.replace;
  expect(navigate).toHaveBeenCalledExactlyOnceWith('/s/other-machine/pair%20name');
});

it('abrir uma sessão pelo Início conserva o Início na pilha para o voltar', () => {
  act(() => root.render(createElement(TabletShell, { children: createElement('main', null, 'new-conversation') })));
  click('pick-b');
  expect(h.router.push).toHaveBeenCalledWith('/s/srv/b');
  expect(h.router.replace).not.toHaveBeenCalled();
});

it.each(['files', 'orchestration', 'ask'])('trocar de sessão com a folha %s aberta fecha a folha', async sheet => {
  h.pathname = `/s/srv/a/${sheet}`;
  await act(async () => root.render(createElement(Layout)));
  click('pick-b');
  expect(h.router.dismissTo).toHaveBeenCalledExactlyOnceWith('/s/srv/b');
  expect(h.router.replace).not.toHaveBeenCalled();
  expect(h.router.push).not.toHaveBeenCalled();
});

it.each(['/login', '/convite/code', '/importar'])('a rota pública %s esconde a barra sem remontar o Stack', async pathname => {
  await act(async () => root.render(createElement(Layout)));
  const stack = host.querySelector('[data-stack]');
  expect(host.querySelector('nav')).not.toBeNull();
  h.pathname = pathname;
  await act(async () => root.render(createElement(Layout)));
  expect(host.querySelector('nav')).toBeNull();
  expect(host.querySelector('aside')).toBeNull();
  expect(host.querySelector('[data-stack]')).toBe(stack);
  expect(host.querySelector(`[aria-label="${m.chat_ir_inicio()}"]`)).toBeNull();
  h.pathname = '/';
  await act(async () => root.render(createElement(Layout)));
  expect(host.querySelector('nav')).not.toBeNull();
  expect(host.querySelector('[data-stack]')).toBe(stack);
});

it.each([true, false])('Custos fica acessível na barra com sessão selecionada: %s', selected => {
  act(() => root.render(createElement(TabletShell, {
    selected: selected ? { serverId: 'srv', name: 'a' } : undefined,
    children: createElement('main', null, 'conversation'),
  })));
  const button = host.querySelector<HTMLButtonElement>(`[aria-label="${m.nav_custos()}"]`);
  expect(button).not.toBeNull();
  expect(button!.style.minWidth).toBe('44px');
  expect(button!.style.minHeight).toBe('44px');
  click(m.nav_custos());
  expect(h.router.push).toHaveBeenCalledWith('/costs');
});

it('a rota inicial conserva a criação ao girar e usa a barra no lugar da lista no tablet', async () => {
  h.pathname = '/';
  h.size = { ...h.size, width: 390, height: 844 };
  await act(async () => root.render(createElement(Layout)));
  const field = host.querySelector('textarea');
  const stack = host.querySelector('[data-stack]');
  expect(host.querySelector('[data-home-sessions]')).not.toBeNull();
  h.size = { ...h.size, width: 1180, height: 820 };
  await act(async () => root.render(createElement(Layout)));
  expect(host.querySelector('nav')).not.toBeNull();
  expect(host.querySelector('[data-create-mode]')?.getAttribute('data-create-mode')).toBe('newConversation');
  expect(host.querySelector('[data-home-sessions]')).toBeNull();
  expect(host.querySelector('aside')).toBeNull();
  expect(host.querySelector('textarea')).toBe(field);
  expect(host.querySelector('[data-stack]')).toBe(stack);
  h.size = { ...h.size, width: 820, height: 1180 };
  await act(async () => root.render(createElement(Layout)));
  expect(host.querySelector('textarea')).toBe(field);
  expect(host.querySelector('[data-stack]')).toBe(stack);
  expect(host.querySelector('nav')).toBeNull();
});

it('trocar de sessão no Stack conserva a mesma barra e o mesmo painel da janela', async () => {
  await act(async () => root.render(createElement(Layout)));
  const bar = host.querySelector('nav');
  const panel = host.querySelector('aside');
  const stack = host.querySelector('[data-stack]');
  expect(bar).not.toBeNull(); expect(panel).not.toBeNull(); expect(stack).not.toBeNull();
  click('pick-b');
  expect(h.router.replace).toHaveBeenCalledWith('/s/srv/b');
  h.pathname = '/s/srv/b'; h.params = { server: 'srv', name: 'b' };
  await act(async () => root.render(createElement(Layout)));
  expect(host.querySelector('nav')).toBe(bar);
  expect(host.querySelector('aside')).toBe(panel);
  expect(host.querySelector('[data-stack]')).toBe(stack);
  expect(bar!.getAttribute('data-selected')).toBe('b');
  expect(panel!.getAttribute('data-selected')).toBe('b');
  expect(h.barMounts).toBe(1); expect(h.panelMounts).toBe(1);
});

it('só a rota da sessão e suas subrotas selecionam a barra e o painel', async () => {
  h.pathname = '/s/srv/a/files';
  await act(async () => root.render(createElement(Layout)));
  expect(host.querySelector('aside')?.getAttribute('data-selected')).toBe('a');
  h.pathname = '/config';
  await act(async () => root.render(createElement(Layout)));
  expect(host.querySelector('nav')).not.toBeNull();
  expect(host.querySelector('aside')).toBeNull();
  expect(host.querySelector('[data-page="/config"]')).not.toBeNull();
});

it('só o chat no tablet deitado desliga a animação de navegação', async () => {
  await act(async () => root.render(createElement(Layout)));
  const stack = host.querySelector('[data-stack]');
  const chatOptions = () => JSON.parse(host.querySelector('[data-screen="s/[server]/[name]/index"]')!.getAttribute('data-options') ?? '{}');
  expect(chatOptions().animation).toBe('none');
  expect(host.querySelector('[data-navigation-background]')?.getAttribute('data-navigation-background')).toBe('transparent');
  expect(JSON.parse(host.querySelector('[data-screen="s/[server]/[name]/files"]')!.getAttribute('data-options')!).presentation).toBe('formSheet');
  // A folha usa o fundo da janela do app (bg.base do tema), nunca o cinza claro padrão da navegação.
  expect(JSON.parse(host.querySelector('[data-screen="s/[server]/[name]/files"]')!.getAttribute('data-options')!).contentStyle.backgroundColor).toBe('#fff');
  h.size = { ...h.size, width: 820, height: 1180 };
  await act(async () => root.render(createElement(Layout)));
  expect(chatOptions().animation).toBeUndefined();
  expect(host.querySelector('nav')).toBeNull();
  expect(host.querySelector('[data-stack]')).toBe(stack);
  expect(host.querySelector('[data-navigation-background]')?.getAttribute('data-navigation-background')).toBe('#fff');
  expect(JSON.parse(host.querySelector('[data-screen="s/[server]/[name]/files"]')!.getAttribute('data-options')!).contentStyle.backgroundColor).toBe('#fff');
});

it('o painel real mantém a estrutura ao trocar de sessão e volta a Contexto', async () => {
  const { ContextPanel } = await vi.importActual<typeof import('./ContextPanel')>('./ContextPanel');
  await act(async () => root.render(createElement(ContextPanel, { serverId: 'srv', name: 'a', focused: true })));
  const tabs = host.querySelector('[role="tablist"]');
  const panel = host.querySelector('aside');
  click(m.tablet_tab_git());
  const content = host.querySelector('[data-git-session="a"]');
  expect(content).not.toBeNull();
  await act(async () => root.render(createElement(ContextPanel, { serverId: 'srv', name: 'b', focused: true })));
  expect(host.querySelector('aside')).toBe(panel);
  expect(host.querySelector('[role="tablist"]')).toBe(tabs);
  expect(host.querySelector('[data-context-session="b"]')).not.toBeNull();
  expect(host.querySelector('[data-git-session]')).toBeNull();
  expect(host.querySelector('[data-git-session="a"]')).toBeNull();
});
