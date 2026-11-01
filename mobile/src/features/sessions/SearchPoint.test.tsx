// @vitest-environment happy-dom
import { act, createElement, forwardRef, useImperativeHandle, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, takeEntry, type Server } from '@hangar/core';
import * as m from '../../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const h = vi.hoisted(() => ({ context: vi.fn(), push: vi.fn(), replace: vi.fn(), scroll: vi.fn(), focused: true, canGoBack: true,
  options: null as { title?: string; headerShown?: boolean; headerTransparent?: boolean; headerLeft?: () => ReactNode } | null,
  edges: undefined as string[] | undefined, scrollInset: undefined as string | undefined,
  drag: null as null | (() => void),
  targetLayout: null as null | ((event: { nativeEvent: { layout: { y: number } } }) => void),
  contentLayout: null as null | ((width: number, height: number) => void),
  params: { server: 'a', project: 'proj', sid: 'sid', eventId: 'match', name: '', conversationTitle: '' } as
    { server: string; project: string; sid: string; eventId: string; name: string; conversationTitle: string; provider?: string; codexAccount?: string; configDir?: string } }));
const state = vi.hoisted(() => ({ activeId: 'b', servers: [
  { id: 'a', label: 'A', baseUrl: 'https://a.test', token: 'a' },
  { id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'b' },
] as Server[] }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(), getSearchContextForServer: h.context }));
vi.mock('../../stores/servers', () => ({ useServers: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) }));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: h.push, replace: h.replace, back: () => {}, canGoBack: () => h.canGoBack }),
  useLocalSearchParams: () => h.params, useIsFocused: () => h.focused,
  Stack: { Screen: ({ options }: { options: NonNullable<typeof h.options> }) => { h.options = options; return options.headerLeft?.() ?? null; } } }));
vi.mock('expo-router/react-navigation', () => ({ useHeaderHeight: () => 88 }));
vi.mock('expo-blur', async () => ({ BlurView: (await import('react-native')).View }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('react-native', async (original) => {
  const actual = await original<typeof import('react-native')>();
  return { ...actual,
    Platform: { OS: 'ios' },
    View: ({ onLayout, ...props }: Record<string, unknown>) => {
      if (props.testID === 'search-active-event') h.targetLayout = onLayout as typeof h.targetLayout;
      return createElement(actual.View, props);
    },
    ScrollView: forwardRef(function ContextScroll({ children, onContentSizeChange, contentInsetAdjustmentBehavior, onScrollBeginDrag }: {
      children: ReactNode; onContentSizeChange: (width: number, height: number) => void; contentInsetAdjustmentBehavior?: string; onScrollBeginDrag?: () => void;
    }, ref) {
      useImperativeHandle(ref, () => ({ scrollTo: h.scroll }));
      h.contentLayout = onContentSizeChange;
      h.scrollInset = contentInsetAdjustmentBehavior;
      h.drag = onScrollBeginDrag ?? null;
      return createElement('div', null, children);
    }),
  };
});
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Screen', () => ({ Screen: ({ children, edges }: { children: ReactNode; edges?: string[] }) => {
  h.edges = edges; return createElement('div', null, children);
} }));
vi.mock('../../chat/AssistantBubble', () => ({ AssistantBubble: ({ text }: { text: string }) => createElement('article', { 'data-markdown': true }, text) }));
vi.mock('../../chat/UserBubble', () => ({ UserBubble: ({ text }: { text: string }) => createElement('p', null, text) }));
import SearchPoint from '../../../app/search-point';

let root: Root; let container: HTMLElement;
const button = (label: string) => [...container.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === label || b.getAttribute('aria-label') === label);
const entry = { project: 'proj', session_id: 'sid', provider: 'codex', codex_account: 'owner-a', config_dir: '/account-a',
  conta: 'Conta A', cwd: '/repo', live: false, preview: 'conversa', ultima: 'fim', mtime: 4 };
async function mount() { await act(async () => root.render(createElement(SearchPoint))); }
beforeEach(() => { vi.resetAllMocks(); h.focused = true; h.canGoBack = true; h.options = null; h.edges = undefined; h.scrollInset = undefined;
  h.targetLayout = null; h.contentLayout = null; h.drag = null;
  h.params = { server: 'a', project: 'proj', sid: 'sid', eventId: 'match', name: '', conversationTitle: '' };
  container = document.createElement('div'); root = createRoot(container);
  configureApi({ getBaseUrl: () => 'https://b.test', getToken: () => 'b', onUnauthorized: () => {}, origin: null,
    createEventSource: () => { throw new Error('unexpected SSE'); } });
  h.context.mockResolvedValue([{ id: 'before', kind: 'user_msg', text: 'vizinha' },
    { id: 'match', kind: 'assistant_msg', text: '**trecho encontrado**' }, { id: 'after', kind: 'user_msg', text: 'posterior' }]);
});
afterEach(() => { act(() => root.unmount()); vi.unstubAllGlobals(); takeEntry('a', 'proj', 'sid'); });

it('ponto usa cabeçalho nativo e recuo medido, com voltar próprio só em link direto', async () => {
  await mount();
  expect(h.options?.headerShown).toBe(true);
  expect(h.options?.headerTransparent).toBe(true);
  expect(h.options?.title).toBe(m.busca_trecho_aqui());
  expect(h.options?.headerLeft).toBeUndefined();
  expect(container.querySelector('[role="header"]')).toBeNull();
  expect(h.edges).not.toContain('top');
  expect([...container.querySelectorAll<HTMLElement>('div')].some((element) => element.style.paddingTop === '88px')).toBe(true);
  expect(h.scrollInset).toBe('automatic');
  for (const control of container.querySelectorAll<HTMLButtonElement>('button')) {
    expect(parseFloat(control.style.minHeight || control.style.height)).toBeGreaterThanOrEqual(44);
    expect(parseFloat(control.style.minWidth || control.style.width)).toBeGreaterThanOrEqual(44);
  }
  h.canGoBack = false; await mount();
  const back = button(m.comum_voltar()); expect(back).toBeDefined();
  await act(async () => back!.click()); expect(h.replace).toHaveBeenCalledExactlyOnceWith('/search');
});

it('acompanha altura assíncrona antes do alvo até arrasto manual', async () => {
  await mount();
  await act(async () => { h.targetLayout?.({ nativeEvent: { layout: { y: 480 } } }); h.contentLayout?.(390, 1800); });
  expect(h.scroll).toHaveBeenLastCalledWith({ y: 480, animated: false });
  await act(async () => { h.targetLayout?.({ nativeEvent: { layout: { y: 960 } } }); h.contentLayout?.(390, 1800); });
  expect(h.scroll).toHaveBeenLastCalledWith({ y: 960, animated: false });
  expect(h.scroll).toHaveBeenCalledTimes(2);
  expect(h.drag).toBeTypeOf('function');
  await act(async () => { h.drag!(); h.targetLayout?.({ nativeEvent: { layout: { y: 1440 } } }); h.contentLayout?.(390, 1800); });
  expect(h.scroll).toHaveBeenCalledTimes(2);
});

it('realinha alvo quando só o conteúdo posterior cresce até arrasto manual', async () => {
  await mount();
  await act(async () => { h.targetLayout?.({ nativeEvent: { layout: { y: 480 } } }); h.contentLayout?.(390, 800); });
  expect(h.scroll).toHaveBeenLastCalledWith({ y: 480, animated: false });
  expect(h.scroll).toHaveBeenCalledTimes(1);
  await act(async () => { h.contentLayout?.(390, 1600); });
  expect(h.scroll).toHaveBeenCalledTimes(2);
  expect(h.scroll).toHaveBeenLastCalledWith({ y: 480, animated: false });
  expect(h.drag).toBeTypeOf('function');
  await act(async () => { h.drag!(); h.contentLayout?.(390, 2400); });
  expect(h.scroll).toHaveBeenCalledTimes(2);
});

it('ponto lê somente vizinhança de A e destaca evento sem buscar histórico inteiro nem retomar', async () => {
  const fetch = vi.fn(); vi.stubGlobal('fetch', fetch); await mount();
  expect(container.textContent).toContain('vizinha');
  expect(container.textContent).toContain('posterior');
  expect(container.querySelector('[data-markdown]')?.textContent).toBe('**trecho encontrado**');
  expect(container.querySelector('[testid="search-active-event"]')?.textContent).toContain('trecho encontrado');
  expect(h.context).toHaveBeenCalledExactlyOnceWith(state.servers[0], 'proj', 'sid', 'match', null, '', '');
  await act(async () => { h.targetLayout?.({ nativeEvent: { layout: { y: 480 } } }); h.contentLayout?.(390, 1800); });
  expect(h.scroll).toHaveBeenCalledWith({ y: 480, animated: false });
  expect(fetch).not.toHaveBeenCalled(); expect(h.push).not.toHaveBeenCalled();
});

it('título do arquivo fica acima da máquina sem transformar abertura em sessão viva', async () => {
  h.params = { ...h.params, conversationTitle: 'Conversa arquivada' };
  const fetch = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(JSON.stringify([entry]), { status: 200 })); vi.stubGlobal('fetch', fetch);
  await mount();
  expect(container.textContent).toContain('Conversa arquivada');
  const open = button(m.switcher_arquivo()); expect(open).toBeDefined();
  expect([...open!.parentElement!.firstElementChild!.children].map((element) => element.textContent)).toEqual(['Conversa arquivada', 'A']);
  expect(button(m.busca_abrir_sessao())).toBeUndefined();
  await act(async () => open!.click());
  expect(String(fetch.mock.calls[0][0])).toBe('https://a.test/api/archive/proj?session_id=sid');
  expect(h.push).toHaveBeenCalledExactlyOnceWith('/archive/a/proj/sid');
  expect(takeEntry('a', 'proj', 'sid')).toEqual(entry);
  expect(state.activeId).toBe('b');
});

it.each([
  ['sessao-viva', '123456789abcdef', 'sessao-viva'],
  ['', '123456789abcdef', '12345678'],
])('link sem título apresenta nome vivo ou início do ID: %s', async (name, sid, title) => {
  h.params = { ...h.params, name, sid };
  await mount();
  expect(container.textContent).toContain(title);
  const open = button(name ? m.busca_abrir_sessao() : m.switcher_arquivo());
  expect(open).toBeDefined();
  expect([...open!.parentElement!.firstElementChild!.children].map((element) => element.textContent)).toEqual([title, 'A']);
  if (name) {
    await act(async () => open!.click());
    expect(h.push).toHaveBeenCalledExactlyOnceWith('/s/a/sessao-viva');
  }
});

it('Abrir arquivo resolve conta e provider em A mesmo ativo B, sem enviar ou retomar', async () => {
  const fetch = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(JSON.stringify([entry]), { status: 200 })); vi.stubGlobal('fetch', fetch);
  await mount();
  const open = button(m.switcher_arquivo()); expect(open).toBeDefined(); await act(async () => open!.click());
  expect(fetch).toHaveBeenCalledTimes(1); expect(String(fetch.mock.calls[0][0])).toBe('https://a.test/api/archive/proj?session_id=sid');
  expect(h.push).toHaveBeenCalledExactlyOnceWith('/archive/a/proj/sid');
  expect(takeEntry('a', 'proj', 'sid')).toEqual(entry); expect(state.activeId).toBe('b');
});

it('ponto Codex lê a vizinhança e abre o arquivo pela conversa, com agente, conta e pasta da conta', async () => {
  h.params = { ...h.params, provider: 'codex', codexAccount: 'owner-a', configDir: '/account-a' };
  const fetch = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => new Response(JSON.stringify([entry]), { status: 200 })); vi.stubGlobal('fetch', fetch);
  await mount();
  expect(h.context).toHaveBeenCalledExactlyOnceWith(state.servers[0], 'proj', 'sid', 'match', 'codex', 'owner-a', '/account-a');
  const open = button(m.switcher_arquivo()); expect(open).toBeDefined(); await act(async () => open!.click());
  expect(String(fetch.mock.calls[0][0])).toBe('https://a.test/api/archive/proj?session_id=sid&provider=codex&codex_account=owner-a');
  expect(h.push).toHaveBeenCalledExactlyOnceWith('/archive/a/proj/sid');
});

it('arquivo ambíguo e falha de contexto aparecem com recuperação sem navegar', async () => {
  h.context.mockRejectedValueOnce(new Error('503 contexto indisponível'));
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify([entry, { ...entry, config_dir: '/other-account' }]), { status: 200 })));
  await mount(); expect(container.textContent).toContain('503 contexto indisponível');
  expect(button(m.busca_tentar_de_novo())).toBeDefined();
  h.context.mockResolvedValueOnce([{ id: 'match', kind: 'assistant_msg', text: 'recuperado' }]);
  await act(async () => button(m.busca_tentar_de_novo())!.click()); expect(container.textContent).toContain('recuperado');
  await act(async () => button(m.switcher_arquivo())!.click());
  expect(container.textContent).toContain(m.arquivo_conversa_conta_ambigua()); expect(h.push).not.toHaveBeenCalled();
});

it('a mesma conversa em duas contas abre a da pasta de conta do trecho', async () => {
  h.params = { ...h.params, configDir: '/other-account' };
  const other = { ...entry, config_dir: '/other-account' };
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify([entry, other]), { status: 200 })));
  await mount();
  await act(async () => button(m.switcher_arquivo())!.click());
  expect(container.textContent).not.toContain(m.arquivo_conversa_conta_ambigua());
  expect(h.push).toHaveBeenCalledExactlyOnceWith('/archive/a/proj/sid');
  expect(takeEntry('a', 'proj', 'sid')).toEqual(other);
});

it('repetir contexto durante abertura não prende arquivo', async () => {
  h.context.mockRejectedValueOnce(new Error('503 contexto indisponível'));
  let finish!: (response: Response) => void;
  const pending = new Promise<Response>((resolve) => { finish = resolve; });
  vi.stubGlobal('fetch', vi.fn(() => pending));
  await mount();
  await act(async () => button(m.switcher_arquivo())!.click());
  await act(async () => button(m.busca_tentar_de_novo())!.click());
  await act(async () => finish(new Response(JSON.stringify([entry]), { status: 200 })));
  expect(h.push).toHaveBeenCalledExactlyOnceWith('/archive/a/proj/sid');
  expect(button(m.switcher_arquivo())!.disabled).toBe(false);
});

it('aprender LAN durante abertura mantém contexto e conclui arquivo', async () => {
  let finish!: (response: Response) => void;
  const pending = new Promise<Response>((resolve) => { finish = resolve; });
  vi.stubGlobal('fetch', vi.fn(() => pending));
  await mount();
  await act(async () => button(m.switcher_arquivo())!.click());
  const before = state.servers;
  try {
    state.servers = state.servers.map((server) => server.id === 'a' ? { ...server, lan: { id: 'a', url: 'http://192.168.0.2:8765' } } : server);
    await mount();
    expect(h.context, 'LAN não reinicia a leitura contextual').toHaveBeenCalledTimes(1);
    await act(async () => finish(new Response(JSON.stringify([entry]), { status: 200 })));
    expect(h.push).toHaveBeenCalledExactlyOnceWith('/archive/a/proj/sid');
    expect(button(m.switcher_arquivo())!.disabled).toBe(false);
  } finally { state.servers = before; }
});

it('ponto descarta vizinhança antiga ao mudar destino ou perder foco', async () => {
  let finish!: (v: unknown) => void; const pending = new Promise((resolve) => { finish = resolve; }); h.context.mockReturnValueOnce(pending);
  await mount(); h.params = { ...h.params, eventId: 'next' };
  h.context.mockResolvedValueOnce([{ id: 'next', kind: 'assistant_msg', text: 'atual' }]); await mount();
  await act(async () => finish([{ id: 'match', kind: 'assistant_msg', text: 'obsoleto' }]));
  expect(container.textContent).toContain('atual'); expect(container.textContent).not.toContain('obsoleto');
  h.focused = false; await mount(); expect(container.querySelector('[data-markdown]')).toBeNull();
});
