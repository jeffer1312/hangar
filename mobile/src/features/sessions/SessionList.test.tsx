// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { OFFLINE_NOTICE_MS, type AggSession, type GroupBy, type Server, type SessionInfo } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const calls = vi.hoisted(() => ({
  alert: vi.fn(),
  deleteSession: vi.fn(),
  error: vi.fn(),
  ok: vi.fn(),
  push: vi.fn(),
  realRow: false,
  reduced: false,
  layoutAnimation: vi.fn(),
  listHeight: 4000,
  listContentSize: null as ((height: number) => void) | null,
  listLayout: null as ((height: number) => void) | null,
  scrollTo: vi.fn(),
  listeners: new Map<string, (event: { data: string }) => void>(),
  stream: { addEventListener: vi.fn(), close: vi.fn(), onerror: null },
  sessions: [{ name: 'fixture', state: 'idle', jsonl: '/tmp/fixture.jsonl' }] as SessionInfo[],
}));

vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  deleteSession: calls.deleteSession,
  rotaDecidida: () => true,
  probeServerResponse: async (server: Server) => ({ ok: true, status: 200,
    json: async () => calls.sessions.map((s) => ({ ...s,
      jsonl: server.invite ? '/tmp/guest.jsonl' : server.id === 'srv1' ? s.jsonl : `/tmp/${server.id}/${s.name}.jsonl`,
    })) }),
  openSessionsStream: () => calls.stream,
  listExternalPairs: async () => [],
}));
vi.mock('../../stores/servers', async () => {
  const { create } = await import('zustand');
  return {
    ownServers: (servers: Server[]) => servers.filter((s) => !s.invite),
    useServers: create<{ servers: Server[]; ready: boolean; activeId: string; ensureActive: ReturnType<typeof vi.fn>; remove: (id: string) => void }>((set) => ({
      servers: [], ready: true, activeId: 'srv1', ensureActive: vi.fn(() => true),
      remove: (id) => set((s) => ({ servers: s.servers.filter((server) => server.id !== id) })),
    })),
  };
});
vi.mock('../../stores/aparencia', async () => {
  const { create } = await import('zustand');
  return {
    useAparencia: create<{ agrupar: GroupBy; organizar: string; setAgrupar: (value: GroupBy) => void }>((set) => ({
      agrupar: 'server', organizar: 'sessions', setAgrupar: (agrupar) => set({ agrupar }),
    })),
    ehGroupBy: (value: unknown) => ['server', 'project', 'none'].includes(String(value)),
  };
});
vi.mock('expo-router', () => ({ useRouter: () => ({ push: calls.push }) }));
vi.mock('react-native-reanimated', async (original) => ({ ...await original<object>(), useReducedMotion: () => calls.reduced }));
vi.mock('@react-native-menu/menu', () => ({
  MenuView: ({ children, actions, onPressAction, shouldOpenOnLongPress }: { children: React.ReactNode; actions: { id: string; title: string; state?: string }[];
    onPressAction: (event: { nativeEvent: { event: string } }) => void; shouldOpenOnLongPress?: boolean }) =>
    React.createElement('div', null, children, ...(calls.realRow || !shouldOpenOnLongPress ? actions.map((action) =>
      React.createElement('button', { key: action.id, 'data-action': action.id, 'data-state': action.state,
        onClick: () => onPressAction({ nativeEvent: { event: action.id } }) }, action.title)) : [])),
}));
vi.mock('../../ui/HangarMark', () => ({ HangarMark: () => null }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { erro: calls.error, ok: calls.ok } }));
vi.mock('./SessionRow', async (original) => {
  const actual = await original<typeof import('./SessionRow')>();
  return { SessionRow: (props: React.ComponentProps<typeof actual.SessionRow>) => {
    if (calls.realRow) return React.createElement(actual.SessionRow, props);
    const { session, onExcluir, onPress, compact } = props;
    return React.createElement('div', null,
      React.createElement('button', { 'data-row': `${session.serverId}::${session.name}`, 'data-compact': String(!!compact), onClick: () => onExcluir?.(session) }, session.name),
      React.createElement('button', { 'data-open': `${session.serverId}::${session.name}`, onClick: () => onPress(session) }));
  } };
});
vi.mock('../pair/PairInviteSheet', () => ({ PairInviteSheet: () => null }));
vi.mock('../voice/LiveVoiceChatButton', () => ({ LiveVoiceChatButton: () => null }));
vi.mock('../pair/PairAcceptSheet', () => ({ PairAcceptSheet: () => null }));
vi.mock('../pair/leaveExternal', () => ({ leaveExternal: vi.fn() }));
vi.mock('./ConversationList', () => ({ ConversationList: () => null }));
vi.mock('./RenameSheet', () => ({ RenameSheet: () => null }));
vi.mock('../worktrees/WorktreeSheet', () => ({ WorktreeSheet: () => null }));
vi.mock('../../paraglide/messages', () => ({ ...Object.fromEntries((
  'lista_agrupar_nenhum lista_agrupar_servidor lista_agrupar_projeto lista_agrupar ' +
  'native_brand lista_buscar lista_filtrar lista_filtro_limpar native_connected config_modal_titulo switcher_buscar_conversas nav_custos ' +
  'native_new_chat_title native_sidebar_all_sessions lista_sessoes_plural sessao_expandir sessao_recolher ' +
  'lista_carregando lista_vazia_filtro lista_nenhuma_ativa comum_cancelar sessao_excluir sessao_excluir_curto servidor_nao_existe ' +
  'comum_carregando lista_nenhum_servidor lista_pareie_qr native_sidebar_awaiting home_sessions_awaiting home_sessions_working home_sessions_ready home_machines_offline sessao_recolher ' +
  'sessao_renomear sessao_git convite_parar orq_row_badge native_par_convidar native_par_menu sessao_worktree_abrir'
).split(' ').map((key) => [key, () => key])),
  home_sessions_more_ready: ({ n }: { n: number }) => `home_sessions_more_ready:${n}`,
  lista_buscar_dentro: ({ q }: { q: string }) => `lista_buscar_dentro:${q}`,
  lista_servidor_offline: ({ label }: { label: string }) => `${label}:offline`,
  pair_leave_outside: ({ dono }: { dono: string }) => `leave-${dono}`,
  pair_from_owner: ({ dono }: { dono: string }) => `from-${dono}`,
  pair_level_view: () => 'view-only',
}));

interface Section { id: string; data: AggSession[] }
vi.mock('react-native', async (original) => {
  const actual = await original<typeof import('react-native')>();
  return {
  ...actual,
  Pressable: ({ accessibilityState, accessibilityHint: _hint, ...props }: React.ComponentProps<typeof actual.Pressable>) =>
    React.createElement(actual.Pressable, { ...props, 'aria-expanded': accessibilityState?.expanded, 'aria-checked': accessibilityState?.checked }),
  Alert: { alert: calls.alert },
  LayoutAnimation: { configureNext: calls.layoutAnimation, create: (duration: number) => ({ duration }), Types: {}, Properties: {} },
  RefreshControl: () => null,
  TextInput: ({ value, onChangeText, accessibilityLabel }: { value: string; onChangeText: (value: string) => void; accessibilityLabel: string }) =>
    React.createElement('input', { value, 'aria-label': accessibilityLabel, onInput: (event: React.FormEvent<HTMLInputElement>) => onChangeText(event.currentTarget.value) }),
  SectionList: ({ ref, sections, ListHeaderComponent, ListEmptyComponent, renderSectionHeader, renderSectionFooter, renderItem,
    contentOffset, onScroll, onScrollBeginDrag, onLayout, onContentSizeChange }: {
    ref?: React.Ref<{ getScrollResponder: () => { scrollTo: (offset: { x: number; y: number; animated: boolean }) => void } }>;
    sections: Section[]; ListHeaderComponent: React.ReactNode; ListEmptyComponent: React.ReactNode;
    contentOffset?: { x: number; y: number };
    onScroll?: (event: { nativeEvent: { contentOffset: { x: number; y: number } } }) => void;
    onScrollBeginDrag?: (event: { nativeEvent: { contentOffset: { x: number; y: number } } }) => void;
    onLayout?: (event: { nativeEvent: { layout: { width: number; height: number; x: number; y: number } } }) => void;
    onContentSizeChange?: (width: number, height: number) => void;
    renderSectionFooter?: (props: { section: Section }) => React.ReactNode;
    renderSectionHeader: (props: { section: Section }) => React.ReactNode;
    renderItem: (props: { item: AggSession; section: Section; index: number }) => React.ReactNode;
  }) => {
    const list = React.useRef<HTMLDivElement>(null);
    const viewport = React.useRef(300);
    const height = React.useRef(calls.listHeight);
    const scroll = (offset: { x: number; y: number }) => {
      if (!list.current) return;
      list.current.scrollLeft = offset.x;
      list.current.scrollTop = Math.max(0, Math.min(offset.y, height.current - viewport.current));
      onScroll?.({ nativeEvent: { contentOffset: { x: list.current.scrollLeft, y: list.current.scrollTop } } });
    };
    React.useImperativeHandle(ref, () => ({ getScrollResponder: () => ({ scrollTo: (offset: { x: number; y: number; animated: boolean }) => {
      calls.scrollTo(offset);
      scroll(offset);
    } }) }));
    calls.listContentSize = (value) => { height.current = value; onContentSizeChange?.(390, value); };
    calls.listLayout = (value) => {
      viewport.current = value;
      onLayout?.({ nativeEvent: { layout: { width: 390, height: value, x: 0, y: 0 } } });
    };
    React.useLayoutEffect(() => {
      scroll(contentOffset ?? { x: 0, y: 0 });
      calls.listLayout?.(viewport.current);
      calls.listContentSize?.(height.current);
    }, []);
    return React.createElement('div', { ref: list, 'data-native-list': true,
      onTouchStart: () => onScrollBeginDrag?.({ nativeEvent: { contentOffset: { x: list.current!.scrollLeft, y: list.current!.scrollTop } } }),
      onScroll: (event: React.UIEvent<HTMLDivElement>) => onScroll?.({ nativeEvent: {
        contentOffset: { x: event.currentTarget.scrollLeft, y: event.currentTarget.scrollTop },
      } }),
    }, ListHeaderComponent, sections.length ? sections.map((section) =>
      React.createElement('div', { key: section.id, 'data-section': section.id }, renderSectionHeader({ section }),
        section.data.map((item, index) => React.createElement(React.Fragment, { key: `${item.serverId}::${item.name}` }, renderItem({ item, section, index }))),
        renderSectionFooter?.({ section }),
      )) : ListEmptyComponent);
  },
  };
});

import { useServers } from '../../stores/servers';
import { useAparencia } from '../../stores/aparencia';
import { useSessions, _resetSessionsForTests } from '../../stores/sessions';
import { SessionList } from './SessionList';

const server = { id: 'srv1', label: 'fixture-server', baseUrl: 'https://fixture.test', token: 'token' };
let root: Root;
let container: HTMLDivElement;

async function mount(props: Partial<React.ComponentProps<typeof SessionList>> = {}) {
  await act(async () => {
    root.render(React.createElement(SessionList, { onClose: vi.fn(), onOpenServers: vi.fn(), ...props }));
  });
}

function emit(sessions: SessionInfo[]) {
  calls.listeners.get('sessions')!({ data: JSON.stringify(sessions) });
}

function confirm(index = 0): () => void {
  return calls.alert.mock.calls[index][2].find((button: { style?: string }) => button.style === 'destructive').onPress;
}

beforeEach(() => {
  _resetSessionsForTests();
  vi.clearAllMocks();
  calls.deleteSession.mockReset();
  useAparencia.setState({ agrupar: 'server', organizar: 'sessions' });
  calls.realRow = false;
  calls.reduced = false;
  calls.listHeight = 4000;
  calls.listContentSize = null;
  calls.listLayout = null;
  calls.sessions = [{ name: 'fixture', state: 'idle', jsonl: '/tmp/fixture.jsonl' }];
  calls.listeners.clear();
  calls.stream.addEventListener.mockImplementation((event: string, listener: (event: { data: string }) => void) => calls.listeners.set(event, listener));
  useServers.setState({ servers: [server], ready: true, activeId: server.id });
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  for (const agrupar of ['server', 'project'] as const) {
    await act(async () => useAparencia.setState({ agrupar }));
    await act(async () => container.querySelectorAll<HTMLButtonElement>('button[aria-expanded="false"]').forEach((button) => button.click()));
  }
  await act(async () => {
    const list = container.querySelector<HTMLDivElement>('[data-native-list]');
    if (list) {
      list.dispatchEvent(new Event('touchstart', { bubbles: true }));
      list.scrollTop = 0;
      list.dispatchEvent(new Event('scroll'));
    }
  });
  await act(async () => root.unmount());
  _resetSessionsForTests();
  container.remove();
});

describe('SessionList', () => {
  it.each(['claude', 'orq'] as const)('git no menu real da linha %s abre a tela da conversa e fecha a lista', async (provider) => {
    calls.realRow = true;
    calls.sessions = [{ ...calls.sessions[0], cwd: '/repo', provider }];
    const onClose = vi.fn();
    await mount({ onClose });
    const git = container.querySelector<HTMLButtonElement>('[data-action="git"]');
    expect(git).not.toBeNull();
    await act(async () => git!.click());
    expect(calls.push).toHaveBeenCalledExactlyOnceWith('/s/srv1/fixture/git');
    expect(onClose).toHaveBeenCalledExactlyOnceWith();
  });

  it('git não aparece no menu real de servidor de convite com cwd', async () => {
    calls.realRow = true;
    calls.sessions = [{ ...calls.sessions[0], cwd: '/repo', provider: 'claude' }];
    useServers.setState({ servers: [server, { ...server, id: 'guest', baseUrl: 'https://guest.test', invite: true }] });
    await mount();
    const stop = container.querySelector<HTMLButtonElement>('[data-action="stop-following"]');
    expect(stop).not.toBeNull();
    expect(stop!.parentElement!.querySelector('[data-action="git"]')).toBeNull();
    expect(stop!.parentElement!.querySelector('[data-action="rename"]')).not.toBeNull();
    await act(async () => stop!.click());
    expect(useServers.getState().servers.map((s) => s.id)).toEqual(['srv1']);
    expect(calls.deleteSession).not.toHaveBeenCalled();
    expect(calls.push).not.toHaveBeenCalled();
  });

  it.each(['conversations', 'sessions'] as const)('gaveta mantém o acesso à busca global visível em %s', async (mode) => {
    useAparencia.setState({ organizar: mode });
    await mount({ variant: 'drawer' });
    const search = container.querySelector<HTMLButtonElement>('[aria-label="switcher_buscar_conversas"]');
    expect(search, 'ação Buscar nas conversas').not.toBeNull();
    await act(async () => search!.click());
    expect(calls.push).toHaveBeenCalledExactlyOnceWith('/search');
  });

  it.each(['home', 'drawer'] as const)('%s tem uma busca só: o termo filtra e oferece buscar dentro das conversas', async (variant) => {
    const onClose = vi.fn();
    await mount({ variant, onClose });
    if (variant === 'home') expect(container.querySelector('[aria-label="switcher_buscar_conversas"]'), 'sem segunda busca na home').toBeNull();
    expect(container.querySelector('[aria-label^="lista_buscar_dentro"]'), 'sem termo, sem atalho').toBeNull();
    const input = container.querySelector<HTMLInputElement>('input[aria-label="lista_filtrar"]')!;
    await act(async () => { input.value = 'a b'; input.dispatchEvent(new Event('input', { bubbles: true })); });
    expect(container.querySelector('[data-row]'), 'o termo filtra a lista').toBeNull();
    const deep = container.querySelector<HTMLButtonElement>('[aria-label="lista_buscar_dentro:a b"]');
    expect(deep).not.toBeNull();
    await act(async () => deep!.click());
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(calls.push).toHaveBeenCalledExactlyOnceWith('/search?q=a%20b');
  });

  it.each(['sessions', 'conversations'] as const)('Custos fecha a gaveta e abre página inteira no modo %s', async (mode) => {
    useAparencia.setState({ organizar: mode });
    const onClose = vi.fn();
    await mount({ onClose });
    const button = container.querySelector<HTMLButtonElement>('[aria-label="nav_custos"]');
    expect(button).not.toBeNull();
    await act(async () => button!.click());
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(calls.push).toHaveBeenCalledWith('/costs');
    expect(onClose.mock.invocationCallOrder[0]).toBeLessThan(calls.push.mock.invocationCallOrder[0]);
  });

  it.each([
    ['home', 'server'], ['drawer', 'server'], ['home', 'none'], ['drawer', 'none'],
  ] as const)('%s sobe esperando e trabalhando para o topo e deixa o resto por servidor (preferência %s)', async (variant, agrupar) => {
    useAparencia.setState({ agrupar });
    useServers.setState({ servers: [server, { ...server, id: 'srv2', label: 'another-server', baseUrl: 'https://another.test' },
      { ...server, id: 'outside', invite: true, ephemeral: true, baseUrl: 'https://outside.test' }] });
    await mount({ variant });
    const row = useSessions.getState().rows[0];
    const session = (name: string, state: AggSession['state'], serverId: string): AggSession => ({
      ...row, name, state, serverId, serverLabel: serverId === 'srv1' ? 'machine-z' : 'machine-a',
    });
    await act(async () => useSessions.setState({ loading: false, rows: [
      session('waiting-z', 'awaiting_input', 'srv1'), session('working-z', 'working', 'srv1'), session('working-a', 'working', 'srv2'),
      session('ready-a', 'idle', 'srv2'), session('ready-z', 'idle', 'srv1'),
      { ...session('outside-partner', 'awaiting_input', 'outside'), serverLabel: 'outside-machine',
        outside: { alias: 'peer', owner: 'owner', localServerId: 'srv1', localSession: 'waiting-z' } },
      session('dead-a', 'dead', 'srv2'),
    ] }));
    expect([...container.querySelectorAll('[data-section]')].map((el) => el.getAttribute('data-section')))
      .toEqual(['state:awaiting_input', 'state:working', 'srv2', 'srv1']);
    expect([...container.querySelectorAll('button[aria-label$="lista_sessoes_plural"]')].map((el) => el.getAttribute('aria-label')))
      .toEqual(['home_sessions_awaiting, 2 lista_sessoes_plural', 'home_sessions_working, 2 lista_sessoes_plural',
        'machine-a, 1 lista_sessoes_plural', 'machine-z, 1 lista_sessoes_plural']);
    const names = (id: string) => [...container.querySelectorAll(`[data-section="${id}"] [data-row]`)].map((el) => el.textContent);
    expect(names('state:awaiting_input')).toEqual(['waiting-z', 'outside-partner']);
    expect(names('state:working')).toEqual(['working-a', 'working-z']);
    expect(names('srv1')).toEqual(['ready-z']);
    expect(names('srv2')).toEqual(['ready-a']);
    const working = container.querySelector<HTMLButtonElement>('[aria-label="home_sessions_working, 2 lista_sessoes_plural"]')!;
    await act(async () => working.click());
    expect(names('state:working')).toEqual([]);
    expect(working.getAttribute('aria-label')).toBe('home_sessions_working, 2 lista_sessoes_plural');
    await act(async () => working.click());
    expect(names('state:working')).toEqual(['working-a', 'working-z']);
    // Tudo parado: só os servidores.
    await act(async () => useSessions.setState({ rows: useSessions.getState().rows.map((r) => r.state === 'dead' ? r : { ...r, state: 'idle' }) }));
    expect([...container.querySelectorAll('[data-section]')].map((el) => el.getAttribute('data-section'))).toEqual(['srv2', 'srv1']);
    expect(names('srv1')).toEqual(['ready-z', 'waiting-z', 'outside-partner', 'working-z']);
  });

  it('anima só quando uma sessão muda de seção, e nunca com Reduzir movimento', async () => {
    await mount({ variant: 'home' });
    const row = useSessions.getState().rows[0];
    await act(async () => useSessions.setState({ loading: false, rows: [{ ...row, name: 'a', state: 'working', label: 'lendo' }, { ...row, name: 'b', state: 'idle' }] }));
    calls.layoutAnimation.mockClear();
    await act(async () => useSessions.setState({ rows: [{ ...row, name: 'a', state: 'working', label: 'editando' }, { ...row, name: 'b', state: 'idle' }] }));
    expect(calls.layoutAnimation, 'atualização sem troca de seção não anima').not.toHaveBeenCalled();
    await act(async () => useSessions.setState({ rows: [{ ...row, name: 'a', state: 'idle' }, { ...row, name: 'b', state: 'idle' }] }));
    expect(calls.layoutAnimation, 'terminou o turno: desce animando').toHaveBeenCalledTimes(1);
    calls.reduced = true;
    await act(async () => useSessions.setState({ rows: [{ ...row, name: 'a', state: 'awaiting_input' }, { ...row, name: 'b', state: 'idle' }] }));
    expect(calls.layoutAnimation).toHaveBeenCalledTimes(1);
  });

  it.each(['home', 'drawer'] as const)('%s com uma máquina rotula as paradas só quando há sessão no topo', async (variant) => {
    await mount({ variant });
    const row = useSessions.getState().rows[0];
    await act(async () => useSessions.setState({ loading: false, rows: [{ ...row, name: 'ready', state: 'idle' }] }));
    expect(container.querySelector('button[aria-label$="lista_sessoes_plural"]')).toBeNull();
    await act(async () => useSessions.setState({ rows: [{ ...row, name: 'ready', state: 'idle' }, { ...row, name: 'busy', state: 'working' }] }));
    expect([...container.querySelectorAll('button[aria-label$="lista_sessoes_plural"]')].map((el) => el.getAttribute('aria-label')))
      .toEqual(['home_sessions_working, 1 lista_sessoes_plural', 'home_sessions_ready, 1 lista_sessoes_plural']);
  });

  it.each(['home', 'drawer'] as const)('%s alterna Servidor/Projeto no menu ao lado da busca e identifica servidor fora do grupo de servidor', async (variant) => {
    calls.realRow = true;
    useAparencia.setState({ agrupar: 'none' });
    useServers.setState({ servers: [server, { ...server, id: 'srv2', label: 'another-server', baseUrl: 'https://another.test' }] });
    await mount({ variant });
    const row = useSessions.getState().rows[0];
    await act(async () => useSessions.setState({ loading: false, rows: [
      { ...row, name: 'from-a', cwd: '/repo/shared', state: 'idle', serverLabel: 'machine-a' },
      { ...row, name: 'from-b', cwd: '/repo/shared', state: 'idle', serverId: 'srv2', serverLabel: 'machine-b' },
      { ...row, name: 'top-a', cwd: '/repo/other', state: 'awaiting_input', serverLabel: 'machine-a' },
    ] }));
    expect(container.querySelector('[role="radio"]'), 'sem faixa Servidor/Projeto').toBeNull();
    const menu = container.querySelector('[aria-label^="lista_agrupar:"]');
    expect(menu?.getAttribute('aria-label')).toBe('lista_agrupar: lista_agrupar_servidor');
    const option = (id: string) => container.querySelector<HTMLButtonElement>(`[data-action="group:${id}"]`)!;
    expect([option('server'), option('project')].map((el) => [el.textContent, el.getAttribute('data-state')]))
      .toEqual([['lista_agrupar_servidor', 'on'], ['lista_agrupar_projeto', 'off']]);
    const session = (name: string) => container.querySelector<HTMLButtonElement>(`button[aria-label^="${name},"]`)!;
    expect(session('from-a').textContent).not.toContain('machine-a');
    expect(session('top-a').textContent, 'no topo a linha diz a máquina').toContain('machine-a');
    await act(async () => option('project').click());
    expect(container.querySelector('button[aria-label="shared, 2 lista_sessoes_plural"]')).not.toBeNull();
    expect(option('project').getAttribute('data-state')).toBe('on');
    expect(container.querySelector('[aria-label^="lista_agrupar:"]')?.getAttribute('aria-label')).toBe('lista_agrupar: lista_agrupar_projeto');
    expect(session('from-a').textContent).toContain('machine-a');
    expect(session('from-b').textContent).toContain('machine-b');
    await act(async () => option('server').click());
    expect(container.querySelector('button[aria-label="shared, 2 lista_sessoes_plural"]')).toBeNull();
    expect(container.querySelector('button[aria-label="machine-a, 1 lista_sessoes_plural"]')).not.toBeNull();
    expect(container.querySelector('button[aria-label="machine-b, 1 lista_sessoes_plural"]')).not.toBeNull();
    expect(session('from-a').textContent).not.toContain('machine-a');
  });

  it.each(['home', 'drawer'] as const)('%s mantém Projeto com uma máquina e não conta convite no seletor', async (variant) => {
    useAparencia.setState({ agrupar: 'project' });
    calls.sessions = [{ ...calls.sessions[0], cwd: '/repo/shared' }];
    useServers.setState({ servers: [server, { ...server, id: 'outside', invite: true, baseUrl: 'https://outside.test' }] });
    await mount({ variant });
    expect(container.querySelector('[data-action^="group:"]')).toBeNull();
    expect(container.querySelector('[aria-label="shared, 2 lista_sessoes_plural"]')).not.toBeNull();
    await act(async () => useAparencia.getState().setAgrupar('server'));
    expect(container.querySelector('button[aria-label$="lista_sessoes_plural"]')).toBeNull();
    expect([...container.querySelectorAll('[data-row]')].map((el) => el.getAttribute('data-row'))).toEqual(['srv1::fixture', 'outside::fixture']);
  });

  it.each(['home', 'drawer'] as const)('%s identifica origem visível de sessões homônimas com uma máquina própria e um convite', async (variant) => {
    calls.realRow = true;
    useServers.setState({ servers: [server, { ...server, id: 'guest', label: 'guest-server', baseUrl: 'https://guest.test', invite: true }] });
    await mount({ variant });
    expect(container.querySelector('[data-action^="group:"]')).toBeNull();
    expect(container.querySelector('button[aria-label$="lista_sessoes_plural"]')).toBeNull();
    const rows = [...container.querySelectorAll<HTMLButtonElement>('button[aria-label^="fixture,"]')];
    expect(rows).toHaveLength(2);
    for (const label of ['fixture-server', 'guest-server']) {
      const row = rows.find((row) => row.getAttribute('aria-label')!.includes(`, ${label}`));
      expect(row?.textContent, `origem ${label} visível na linha`).toContain(label);
    }
  });

  it('cabeçalho recolhível tem alvo de toque de 44pt e conserva a contagem ao recolher', async () => {
    useServers.setState({ servers: [server, { ...server, id: 'srv2', label: 'another-server', baseUrl: 'https://another.test' }] });
    await mount({ variant: 'home' });
    const header = container.querySelector<HTMLButtonElement>('[aria-label="fixture-server, 1 lista_sessoes_plural"]')!;
    expect(Number.parseFloat(header.style.minHeight)).toBeGreaterThanOrEqual(44);
    expect(header.getAttribute('aria-expanded')).toBe('true');
    await act(async () => header.click());
    expect(header.getAttribute('aria-expanded')).toBe('false');
    expect(header.textContent).toBe('fixture-server1');
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();
    await act(async () => header.click());
  });

  it('home preserva grupos recolhidos e posição ao abrir sessão e voltar com remount, separados da gaveta', async () => {
    useServers.setState({ servers: [server, { ...server, id: 'srv2', label: 'another-server', baseUrl: 'https://another.test' }] });
    calls.sessions = [
      { name: 'ready-old', state: 'idle', jsonl: '/tmp/old.jsonl', last_activity: 100 },
      { name: 'ready-new', state: 'idle', jsonl: '/tmp/new.jsonl', last_activity: 300 },
      { name: 'ready-middle', state: 'idle', jsonl: '/tmp/middle.jsonl', last_activity: 200 },
    ];
    await mount({ variant: 'home' });
    // A tela inicial e o chat também retêm o store durante a navegação.
    const release = useSessions.getState().retain();
    const group = (label: string) => container.querySelector<HTMLButtonElement>(`[aria-label="${label}, 3 lista_sessoes_plural"]`)!;
    try {
      await act(async () => group('fixture-server').click());
      const list = container.querySelector<HTMLDivElement>('[data-native-list]')!;
      await act(async () => { list.scrollTop = 480; list.dispatchEvent(new Event('scroll')); });
      await act(async () => container.querySelector<HTMLButtonElement>('[data-open="srv2::ready-old"]')!.click());
      expect(calls.push).toHaveBeenCalledWith('/s/srv2/ready-old');
      await act(async () => root.render(null));
      await mount({ variant: 'drawer' });
      const drawer = container.querySelector<HTMLDivElement>('[data-native-list]')!;
      expect(drawer.scrollTop).toBe(0);
      expect(container.querySelector('[data-row="srv1::ready-old"]'), 'gaveta não herda grupo recolhido da home').not.toBeNull();
      await act(async () => group('another-server').click());
      await act(async () => { drawer.scrollTop = 75; drawer.dispatchEvent(new Event('scroll')); });
      await act(async () => root.render(null));
      await mount({ variant: 'home' });
      expect(container.querySelector('[data-row="srv1::ready-old"]'), 'grupo da home permanece recolhido após remount').toBeNull();
      expect(group('fixture-server').getAttribute('aria-expanded')).toBe('false');
      expect(container.querySelector('[data-row="srv2::ready-old"]'), 'home não herda grupo recolhido da gaveta').not.toBeNull();
      expect(container.querySelector<HTMLDivElement>('[data-native-list]')!.scrollTop, 'posição da home não herda a da gaveta').toBe(480);
    } finally { release(); }
  });

  it.each(['crescimento', 'novo remount', 'gesto'])('home restaura lista longa e grupo recolhido com altura insuficiente: %s', async (scenario) => {
    useServers.setState({ servers: [server, { ...server, id: 'srv2', label: 'another-server', baseUrl: 'https://another.test' }] });
    calls.sessions = Array.from({ length: 40 }, (_, index) => ({
      name: `ready-${index}`, state: 'idle', jsonl: `/tmp/ready-${index}.jsonl`, last_activity: 100 - index,
    }));
    await mount({ variant: 'home' });
    const release = useSessions.getState().retain();
    const list = () => container.querySelector<HTMLDivElement>('[data-native-list]')!;
    try {
      await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="another-server, 40 lista_sessoes_plural"]')!.click());
      await act(async () => { list().scrollTop = 1500; list().dispatchEvent(new Event('scroll')); });
      await act(async () => container.querySelector<HTMLButtonElement>('[data-open="srv1::ready-20"]')!.click());
      expect(calls.push).toHaveBeenCalledWith('/s/srv1/ready-20');
      await act(async () => root.render(null));
      calls.listHeight = 500;
      await mount({ variant: 'home' });
      expect(list().scrollTop, 'altura nativa inicial limita a rolagem').toBe(200);
      expect(container.querySelector('[data-row="srv2::ready-20"]'), 'grupo permanece recolhido durante a restauração').toBeNull();
      if (scenario === 'novo remount') {
        await act(async () => root.render(null));
        calls.listHeight = 2000;
        await mount({ variant: 'home' });
        expect(list().scrollTop, 'scroll intermediário não substitui o alvo guardado').toBe(1500);
      } else {
        await act(async () => calls.listContentSize!(1000));
        expect(list().scrollTop, 'crescimento permite avançar até o novo limite').toBe(700);
        if (scenario === 'gesto') {
          await act(async () => {
            list().dispatchEvent(new Event('touchstart', { bubbles: true }));
            list().scrollTop = 120;
            list().dispatchEvent(new Event('scroll'));
          });
          const attempts = calls.scrollTo.mock.calls.length;
          await act(async () => { calls.listContentSize!(2500); calls.listLayout!(200); });
          expect(calls.scrollTo).toHaveBeenCalledTimes(attempts);
          expect(list().scrollTop, 'gesto cancela reposicionamento automático').toBe(120);
          await act(async () => root.render(null));
          calls.listHeight = 2500;
          await mount({ variant: 'home' });
          expect(list().scrollTop, 'próximo remount guarda a posição escolhida no gesto').toBe(120);
        } else {
          await act(async () => calls.listLayout!(200));
          expect(list().scrollTop, 'layout também retenta a posição pendente').toBe(800);
          await act(async () => calls.listContentSize!(2000));
          expect(list().scrollTop, 'altura suficiente alcança o alvo').toBe(1500);
          const attempts = calls.scrollTo.mock.calls.length;
          await act(async () => { calls.listContentSize!(2500); calls.listLayout!(300); });
          expect(calls.scrollTo).toHaveBeenCalledTimes(attempts);
          expect(list().scrollTop).toBe(1500);
        }
      }
    } finally { release(); }
  });

  it('home recolhe o grupo inteiro sem limitar prontas à mais recente', async () => {
    useServers.setState({ servers: [server, { ...server, id: 'srv2', label: 'another-server', baseUrl: 'https://another.test' }] });
    await mount({ variant: 'home' });
    const row = useSessions.getState().rows[0];
    const rows = [
      { ...row, name: 'antiga', state: 'idle', last_activity: 100 },
      { ...row, name: 'recente', state: 'idle', last_activity: 300 },
      { ...row, name: 'meio', state: 'idle', last_activity: 200 },
    ] as AggSession[];
    await act(async () => useSessions.setState({ rows, loading: false }));
    const names = () => [...container.querySelectorAll('[data-row]')].map((el) => el.textContent);
    expect(names()).toEqual(['antiga', 'meio', 'recente']);
    const group = container.querySelector<HTMLButtonElement>('[aria-label="fixture-server, 3 lista_sessoes_plural"]')!;
    await act(async () => group.click());
    expect(names()).toEqual([]);
    await act(async () => group.click());
    expect(names()).toEqual(['antiga', 'meio', 'recente']);
  });

  it.each(['home', 'drawer'] as const)('%s separa recolhimento por modo mesmo quando servidor e projeto têm o mesmo id', async (variant) => {
    calls.sessions = [{ ...calls.sessions[0], cwd: 'srv1' }];
    useServers.setState({ servers: [server, { ...server, id: 'srv2', label: 'another-server', baseUrl: 'https://another.test' }] });
    await mount({ variant });
    const group = (label: string, n: number) => container.querySelector<HTMLButtonElement>(`[aria-label="${label}, ${n} lista_sessoes_plural"]`)!;
    await act(async () => group('fixture-server', 1).click());
    await act(async () => container.querySelector<HTMLButtonElement>('[data-action="group:project"]')!.click());
    expect(group('srv1', 2).getAttribute('aria-expanded')).toBe('true');
    expect(container.querySelector('[data-row="srv1::fixture"]')).not.toBeNull();
    await act(async () => group('srv1', 2).click());
    await act(async () => container.querySelector<HTMLButtonElement>('[data-action="group:server"]')!.click());
    expect(group('fixture-server', 1).getAttribute('aria-expanded')).toBe('false');
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();
    expect(container.querySelector('[data-row="srv2::fixture"]')).not.toBeNull();
  });

  it('home mantém grupos e filtro ao organizar por conversas sem perder ações', async () => {
    useAparencia.setState({ organizar: 'conversations' });
    useServers.setState({ servers: [server, { ...server, id: 'srv2', label: 'another-machine', baseUrl: 'https://another.test' }] });
    const onClose = vi.fn();
    await mount({ variant: 'home', onClose });
    const row = useSessions.getState().rows[0];
    const rows = [
      { ...row, name: 'ready-a', state: 'idle' },
      { ...row, name: 'waiting-b', state: 'awaiting_input', serverId: 'srv2', serverLabel: 'another-machine' },
      { ...row, name: 'working-a', state: 'working' },
      { ...row, name: 'ready-b', state: 'idle', serverId: 'srv2', serverLabel: 'another-machine' },
      { ...row, name: 'dead-a', state: 'dead' },
    ] as AggSession[];
    await act(async () => useSessions.setState({ rows, loading: false }));
    expect([...container.querySelectorAll('[data-row]')].map((el) => el.textContent))
      .toEqual(['waiting-b', 'working-a', 'ready-b', 'ready-a']);
    expect(container.querySelector('input[aria-label="lista_filtrar"]')).not.toBeNull();
    expect(container.textContent).not.toContain('native_brand');
    expect(container.textContent).not.toContain('native_sidebar_all_sessions');
    expect(container.querySelector('[aria-label="config_modal_titulo"]')).toBeNull();
    const group = container.querySelector<HTMLButtonElement>('[aria-label="home_sessions_working, 1 lista_sessoes_plural"]')!;
    await act(async () => group.click());
    expect(container.querySelector('[data-row="srv1::working-a"]')).toBeNull();
    await act(async () => group.click());
    await act(async () => container.querySelector<HTMLButtonElement>('[data-open="srv2::waiting-b"]')!.click());
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(calls.push).toHaveBeenCalledWith('/s/srv2/waiting-b');
    calls.deleteSession.mockResolvedValueOnce({ ok: true });
    await act(async () => container.querySelector<HTMLButtonElement>('[data-row="srv1::working-a"]')!.click());
    await act(async () => confirm()());
    expect(calls.deleteSession).toHaveBeenCalledWith('working-a');
  });

  it.each(['home', 'drawer'] as const)('%s mantém pares externos após cada âncora e conta todas as linhas do grupo recolhível', async (variant) => {
    useServers.setState({ servers: [server, { ...server, id: 'srv2', label: 'another-machine', baseUrl: 'https://another.test' }] });
    await mount({ variant });
    const row = useSessions.getState().rows[0];
    const external = (name: string, anchor: string): AggSession => ({ ...row, name, state: 'awaiting_input', serverId: 'outside',
      outside: { alias: 'peer', owner: 'owner', localServerId: 'srv1', localSession: anchor } });
    await act(async () => useSessions.setState({ loading: false, rows: [
      { ...row, name: 'working-anchor', state: 'working' }, external('working-outside', 'working-anchor'),
      { ...row, name: 'ready-a', state: 'idle' }, external('ready-a-outside', 'ready-a'),
      { ...row, name: 'ready-b', state: 'idle' }, external('ready-b-outside', 'ready-b'),
    ] }));
    const names = () => [...container.querySelectorAll('[data-section="srv1"] [data-row]')].map((el) => el.textContent);
    expect(names()).toEqual(['ready-a', 'ready-a-outside', 'ready-b', 'ready-b-outside']);
    expect([...container.querySelectorAll('[data-section="state:working"] [data-row]')].map((el) => el.textContent))
      .toEqual(['working-anchor', 'working-outside']);
    expect(container.querySelector('[data-section="outside"]')).toBeNull();
    const group = container.querySelector<HTMLButtonElement>('[aria-label="fixture-server, 4 lista_sessoes_plural"]')!;
    await act(async () => group.click());
    expect(names()).toEqual([]);
    expect(group.getAttribute('aria-label')).toBe('fixture-server, 4 lista_sessoes_plural');
    await act(async () => group.click());
    expect(names()).toEqual(['ready-a', 'ready-a-outside', 'ready-b', 'ready-b-outside']);
  });

  it('home mostra carregamento, vazio e falha identificada por máquina', async () => {
    await mount({ variant: 'home' });
    await act(async () => useSessions.setState({ rows: [], loading: true }));
    expect(container.textContent).toContain('lista_carregando');
    expect(container.textContent).not.toContain('lista_nenhuma_ativa');
    await act(async () => useSessions.setState({ loading: false, byServer: [{ server, sessions: [], loaded: true, error: null }] }));
    expect(container.textContent).toContain('lista_nenhuma_ativa');
    await act(async () => useSessions.setState({ byServer: [{ server, sessions: [], loaded: true, error: 'list-unavailable' }] }));
    expect(container.textContent).toContain('fixture-server: list-unavailable');
    expect(container.textContent).not.toContain('lista_nenhuma_ativa');
  });

  it('home mostra a máquina offline só no começo da queda e some sozinha depois', async () => {
    await mount({ variant: 'home' });
    vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout'] });
    try {
      const offline = { server, sessions: [], loaded: false, error: 'offline', offlineSince: Date.now() };
      await act(async () => useSessions.setState({ rows: [], loading: false, byServer: [offline] }));
      expect(container.textContent).toContain('fixture-server:offline');
      await act(async () => vi.advanceTimersByTime(OFFLINE_NOTICE_MS - 1));
      expect(container.textContent).toContain('fixture-server:offline');
      // O timer pode disparar um pouco antes do fim da janela: o aviso não pode ficar preso.
      vi.setSystemTime(Date.now() - 1);
      await act(async () => vi.advanceTimersByTime(1));
      expect(container.textContent).not.toContain('fixture-server:offline');
      // Sem lista nem aviso: não volta a dizer "carregando" para a máquina desligada.
      expect(container.textContent).not.toContain('fixture-server: lista_carregando');
      // Todas as máquinas fora: lista vazia não diz "nenhuma sessão ativa".
      expect(container.textContent).toContain('home_machines_offline');
      expect(container.textContent).not.toContain('lista_nenhuma_ativa');
    } finally {
      vi.useRealTimers();
    }
  });

  it('Nova conversa usa sua ação própria sem confundir fechamento para navegação', async () => {
    const onClose = vi.fn();
    const onNewConversation = vi.fn();
    await mount({ onClose, onNewConversation });
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="native_new_chat_title"]')!.click());
    expect(onNewConversation).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
  });

  it.each([null, { unknown_warning: true }])('esconde antes do DELETE terminar, bloqueia confirmação duplicada e mantém marca após sucesso (%j)', async (warning) => {
    let resolve!: (value: { ok: boolean; warning: unknown | null }) => void;
    calls.deleteSession.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    await mount();
    const row = container.querySelector<HTMLButtonElement>('[data-row="srv1::fixture"]')!;
    expect(row).not.toBeNull();
    await act(async () => { row.click(); row.click(); });
    expect(calls.alert).toHaveBeenCalledTimes(1);
    const first = confirm();

    await act(async () => { first(); first(); });

    expect(calls.deleteSession).toHaveBeenCalledExactlyOnceWith('fixture');
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();
    expect(useSessions.getState().rows).toEqual([]);
    expect(calls.ok).not.toHaveBeenCalled();
    await act(async () => { emit(calls.sessions); });
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();

    await act(async () => { resolve({ ok: true, warning }); });
    if (warning) expect(calls.error).toHaveBeenCalled();
    else expect(calls.ok).toHaveBeenCalledWith('fixture');
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();
    await act(async () => { emit(calls.sessions); });
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();
    await act(async () => { emit([]); });
    await act(async () => { emit([{ ...calls.sessions[0], jsonl: '/tmp/recreated.jsonl' }]); });
    expect(container.querySelector('[data-row="srv1::fixture"]')).not.toBeNull();
    await act(async () => { first(); });
    expect(calls.deleteSession).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[data-row="srv1::fixture"]')).not.toBeNull();
    await act(async () => { container.querySelector<HTMLButtonElement>('[data-row="srv1::fixture"]')!.click(); });
    expect(calls.alert).toHaveBeenCalledTimes(2);
  });

  it.each(['cancel', 'dismiss'] as const)('%s libera o diálogo e callbacks antigas não confirmam nem liberam o próximo', async (action) => {
    calls.deleteSession.mockResolvedValueOnce({ ok: true, warning: null });
    await mount();
    const row = container.querySelector<HTMLButtonElement>('[data-row="srv1::fixture"]')!;
    await act(async () => { row.click(); });
    const first = confirm();
    const close = action === 'cancel'
      ? calls.alert.mock.calls[0][2].find((button: { style?: string }) => button.style === 'cancel').onPress
      : calls.alert.mock.calls[0][3]?.onDismiss;
    expect(close).toBeTypeOf('function');

    await act(async () => { close(); });
    expect(calls.deleteSession).not.toHaveBeenCalled();
    expect(container.querySelector('[data-row="srv1::fixture"]')).not.toBeNull();
    await act(async () => { row.click(); });
    expect(calls.alert).toHaveBeenCalledTimes(2);

    await act(async () => { first(); close(); row.click(); });
    expect(calls.deleteSession).not.toHaveBeenCalled();
    expect(calls.alert).toHaveBeenCalledTimes(2);
    expect(container.querySelector('[data-row="srv1::fixture"]')).not.toBeNull();
    await act(async () => { confirm(1)(); });
    expect(calls.deleteSession).toHaveBeenCalledExactlyOnceWith('fixture');
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();
  });

  it('cancelamento e dispensa tardios não liberam uma exclusão pendente', async () => {
    let resolve!: (value: { ok: boolean; warning: null }) => void;
    calls.deleteSession.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
    await mount();
    await act(async () => { container.querySelector<HTMLButtonElement>('[data-row="srv1::fixture"]')!.click(); });
    const first = confirm();
    const cancel = calls.alert.mock.calls[0][2].find((button: { style?: string }) => button.style === 'cancel').onPress;
    const dismiss = calls.alert.mock.calls[0][3]?.onDismiss;
    expect(cancel).toBeTypeOf('function');
    expect(dismiss).toBeTypeOf('function');
    await act(async () => { first(); first(); });
    expect(calls.deleteSession).toHaveBeenCalledExactlyOnceWith('fixture');
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();

    await act(async () => { cancel(); dismiss(); emit([]); });
    await act(async () => { emit([{ ...calls.sessions[0], jsonl: '/tmp/recreated.jsonl' }]); });
    expect(container.querySelector('[data-row="srv1::fixture"]')).not.toBeNull();
    await act(async () => { container.querySelector<HTMLButtonElement>('[data-row="srv1::fixture"]')!.click(); first(); });
    expect(calls.alert).toHaveBeenCalledTimes(1);
    expect(calls.deleteSession).toHaveBeenCalledTimes(1);

    await act(async () => { resolve({ ok: true, warning: null }); });
    await act(async () => { container.querySelector<HTMLButtonElement>('[data-row="srv1::fixture"]')!.click(); });
    expect(calls.alert).toHaveBeenCalledTimes(2);
  });

  it('servidor indisponível libera a confirmação sem DELETE e permite abrir outra', async () => {
    vi.mocked(useServers.getState().ensureActive).mockReturnValueOnce(false);
    calls.deleteSession.mockResolvedValueOnce({ ok: true, warning: null });
    await mount();
    const row = container.querySelector<HTMLButtonElement>('[data-row="srv1::fixture"]')!;
    await act(async () => { row.click(); });
    const first = confirm();
    await act(async () => { first(); first(); });
    expect(calls.deleteSession).not.toHaveBeenCalled();
    expect(calls.error).toHaveBeenCalledWith('servidor_nao_existe');
    expect(container.querySelector('[data-row="srv1::fixture"]')).not.toBeNull();

    await act(async () => { row.click(); });
    expect(calls.alert).toHaveBeenCalledTimes(2);
    await act(async () => { confirm(1)(); });
    expect(calls.deleteSession).toHaveBeenCalledExactlyOnceWith('fixture');
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();
  });

  it('falha do DELETE restaura a linha, mostra a causa e permite tentar de novo', async () => {
    let reject!: (error: Error) => void;
    calls.deleteSession.mockReturnValueOnce(new Promise((_, fail) => { reject = fail; }));
    await mount();
    await act(async () => { container.querySelector<HTMLButtonElement>('[data-row="srv1::fixture"]')!.click(); });
    await act(async () => { confirm()(); });
    expect(container.querySelector('[data-row="srv1::fixture"]')).toBeNull();

    await act(async () => { reject(new Error('delete-failed')); });

    expect(container.querySelector('[data-row="srv1::fixture"]')).not.toBeNull();
    expect(calls.error).toHaveBeenCalledWith('delete-failed');
    calls.deleteSession.mockResolvedValueOnce({ ok: true, warning: null });
    await act(async () => { container.querySelector<HTMLButtonElement>('[data-row="srv1::fixture"]')!.click(); });
    await act(async () => { confirm(1)(); });
    expect(calls.deleteSession).toHaveBeenCalledTimes(2);
  });

  it('parar de acompanhar remove só o convite, sem DELETE nem troca da máquina ativa', async () => {
    calls.deleteSession.mockResolvedValueOnce({ ok: true, warning: null });
    useServers.setState({ servers: [server, { ...server, id: 'guest', baseUrl: 'https://guest.test', invite: true }] });
    await mount();
    const row = container.querySelector<HTMLButtonElement>('[data-row="guest::fixture"]');
    expect(row).not.toBeNull();
    await act(async () => row!.click());
    // Também denuncia a tentativa antiga, que abria a confirmação de exclusão.
    if (calls.alert.mock.calls.length) await act(async () => confirm()());
    expect(calls.deleteSession).not.toHaveBeenCalled();
    expect(useServers.getState().ensureActive).not.toHaveBeenCalled();
    expect(useServers.getState().activeId).toBe('srv1');
    expect(useServers.getState().servers.map((s) => s.id)).toEqual(['srv1']);
    expect(container.querySelector('[data-row="guest::fixture"]')).toBeNull();
  });

  it.each(['home', 'drawer'] as const)('%s mostra filtro da lista e aviso de filtro sem resultados', async (variant) => {
    await mount({ variant });
    await act(async () => {
      const input = container.querySelector<HTMLInputElement>('input[aria-label="lista_filtrar"]');
      expect(input, 'filtro da lista sempre visível').not.toBeNull();
      input!.value = 'not-found';
      input!.dispatchEvent(new Event('input', { bubbles: true }));
    });
    expect(container.querySelector('[data-row]')).toBeNull();
    expect(container.textContent).toContain('lista_vazia_filtro');
    expect(container.textContent).not.toContain('lista_nenhuma_ativa');
  });

  it('grupo recolhido não mostra aviso de lista vazia', async () => {
    useAparencia.setState({ agrupar: 'project' });
    await mount();
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label$="lista_sessoes_plural"]')!.click();
    });
    expect(container.querySelector('[data-row]')).toBeNull();
    expect(container.textContent).not.toContain('lista_vazia_filtro');
    expect(container.textContent).not.toContain('lista_nenhuma_ativa');
  });
});
