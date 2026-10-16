// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AggSession, Server, SessionInfo } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const calls = vi.hoisted(() => ({
  alert: vi.fn(),
  deleteSession: vi.fn(),
  error: vi.fn(),
  ok: vi.fn(),
  push: vi.fn(),
  organizar: 'sessions',
  listeners: new Map<string, (event: { data: string }) => void>(),
  stream: { addEventListener: vi.fn(), close: vi.fn(), onerror: null },
  sessions: [{ name: 'fixture', state: 'idle', jsonl: '/tmp/fixture.jsonl' }] as SessionInfo[],
}));

vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  deleteSession: calls.deleteSession,
  rotaDecidida: () => true,
  probeServerResponse: async (server: Server) => ({ ok: true, status: 200,
    json: async () => server.invite ? calls.sessions.map((s) => ({ ...s, jsonl: '/tmp/guest.jsonl' })) : calls.sessions }),
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
vi.mock('../../stores/aparencia', () => ({
  useAparencia: (select: (state: { agrupar: string; organizar: string }) => unknown) => select({ agrupar: 'project', organizar: calls.organizar }),
  ehGroupBy: () => false,
}));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: calls.push }) }));
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('../../ui/HangarMark', () => ({ HangarMark: () => null }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { erro: calls.error, ok: calls.ok } }));
vi.mock('./SessionRow', () => ({
  SessionRow: ({ session, onExcluir, onPress, compact }: { session: AggSession; onExcluir: (session: AggSession) => void; onPress: (session: AggSession) => void; compact?: boolean }) =>
    React.createElement('div', null,
      React.createElement('button', { 'data-row': `${session.serverId}::${session.name}`, 'data-compact': String(!!compact), onClick: () => onExcluir(session) }, session.name),
      React.createElement('button', { 'data-open': `${session.serverId}::${session.name}`, onClick: () => onPress(session) })),
}));
vi.mock('../pair/PairInviteSheet', () => ({ PairInviteSheet: () => null }));
vi.mock('../voice/LiveVoiceChatButton', () => ({ LiveVoiceChatButton: () => null }));
vi.mock('../pair/PairAcceptSheet', () => ({ PairAcceptSheet: () => null }));
vi.mock('../pair/leaveExternal', () => ({ leaveExternal: vi.fn() }));
vi.mock('./ConversationList', () => ({ ConversationList: () => null }));
vi.mock('./RenameSheet', () => ({ RenameSheet: () => null }));
vi.mock('../worktrees/WorktreeSheet', () => ({ WorktreeSheet: () => null }));
vi.mock('../../paraglide/messages', () => ({ ...Object.fromEntries((
  'lista_agrupar_nenhum lista_agrupar_servidor lista_agrupar_projeto lista_agrupar ' +
  'native_brand lista_buscar lista_filtrar lista_filtro_limpar native_connected config_modal_titulo ' +
  'native_new_chat_title native_sidebar_all_sessions lista_sessoes_plural sessao_expandir sessao_recolher ' +
  'lista_carregando lista_vazia_filtro lista_nenhuma_ativa comum_cancelar sessao_excluir sessao_excluir_curto servidor_nao_existe ' +
  'comum_carregando lista_nenhum_servidor lista_pareie_qr native_sidebar_awaiting home_sessions_awaiting home_sessions_working home_sessions_ready sessao_recolher'
).split(' ').map((key) => [key, () => key])),
  home_sessions_more_ready: ({ n }: { n: number }) => `home_sessions_more_ready:${n}`,
  lista_servidor_offline: ({ label }: { label: string }) => `${label}:offline`,
}));

interface Section { id: string; data: AggSession[] }
vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(),
  Alert: { alert: calls.alert },
  RefreshControl: () => null,
  TextInput: ({ value, onChangeText, accessibilityLabel }: { value: string; onChangeText: (value: string) => void; accessibilityLabel: string }) =>
    React.createElement('input', { value, 'aria-label': accessibilityLabel, onInput: (event: React.FormEvent<HTMLInputElement>) => onChangeText(event.currentTarget.value) }),
  SectionList: ({ sections, ListHeaderComponent, ListEmptyComponent, renderSectionHeader, renderSectionFooter, renderItem }: {
    sections: Section[]; ListHeaderComponent: React.ReactNode; ListEmptyComponent: React.ReactNode;
    renderSectionFooter?: (props: { section: Section }) => React.ReactNode;
    renderSectionHeader: (props: { section: Section }) => React.ReactNode;
    renderItem: (props: { item: AggSession; section: Section }) => React.ReactNode;
  }) => React.createElement('div', null, ListHeaderComponent, sections.length ? sections.map((section) =>
    React.createElement('div', { key: section.id, 'data-section': section.id }, renderSectionHeader({ section }),
      section.data.map((item) => React.createElement(React.Fragment, { key: `${item.serverId}::${item.name}` }, renderItem({ item, section }))),
      renderSectionFooter?.({ section }),
    )) : ListEmptyComponent),
}));

import { useServers } from '../../stores/servers';
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
  calls.organizar = 'sessions';
  calls.sessions = [{ name: 'fixture', state: 'idle', jsonl: '/tmp/fixture.jsonl' }];
  calls.listeners.clear();
  calls.stream.addEventListener.mockImplementation((event: string, listener: (event: { data: string }) => void) => calls.listeners.set(event, listener));
  useServers.setState({ servers: [server], ready: true, activeId: server.id });
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  _resetSessionsForTests();
  container.remove();
});

describe('SessionList', () => {
  it('home reúne máquinas por estado, ignora organizar e expande prontas sem perder ações', async () => {
    calls.organizar = 'conversations';
    const onClose = vi.fn();
    await mount({ variant: 'home', onClose });
    const row = useSessions.getState().rows[0];
    const rows = [
      { ...row, name: 'ready-a', state: 'idle' },
      { ...row, name: 'waiting-b', state: 'awaiting_input', serverId: 'srv2', serverLabel: 'another-machine' },
      { ...row, name: 'working-a', state: 'working' },
      { ...row, name: 'ready-b', state: 'idle', serverId: 'srv2' },
      { ...row, name: 'dead-a', state: 'dead' },
    ] as AggSession[];
    await act(async () => useSessions.setState({ rows, loading: false }));
    expect([...container.querySelectorAll('[data-row]')].map((el) => el.textContent))
      .toEqual(['waiting-b', 'working-a', 'ready-a']);
    expect(container.textContent).toContain('home_sessions_more_ready:1');
    expect(container.querySelector('input')).toBeNull();
    expect(container.textContent).not.toContain('native_brand');
    expect(container.textContent).not.toContain('native_sidebar_all_sessions');
    expect(container.querySelector('[aria-label="config_modal_titulo"]')).toBeNull();
    await act(async () => [...container.querySelectorAll('button')].find((b) => b.textContent === 'home_sessions_more_ready:1')!.click());
    expect(container.querySelector('[data-row="srv2::ready-b"]')).not.toBeNull();
    await act(async () => container.querySelector<HTMLButtonElement>('[data-open="srv2::waiting-b"]')!.click());
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(calls.push).toHaveBeenCalledWith('/s/srv2/waiting-b');
    calls.deleteSession.mockResolvedValueOnce({ ok: true });
    await act(async () => container.querySelector<HTMLButtonElement>('[data-row="srv1::working-a"]')!.click());
    await act(async () => confirm()());
    expect(calls.deleteSession).toHaveBeenCalledWith('working-a');
  });

  it('home mantém pares externos junto da âncora e conta só âncoras ao expandir prontas', async () => {
    await mount({ variant: 'home' });
    const row = useSessions.getState().rows[0];
    const external = (name: string, anchor: string): AggSession => ({ ...row, name, state: 'awaiting_input', serverId: 'outside',
      outside: { alias: 'peer', owner: 'owner', localServerId: 'srv1', localSession: anchor } });
    await act(async () => useSessions.setState({ loading: false, rows: [
      { ...row, name: 'working-anchor', state: 'working' }, external('working-outside', 'working-anchor'),
      { ...row, name: 'ready-a', state: 'idle' }, external('ready-a-outside', 'ready-a'),
      { ...row, name: 'ready-b', state: 'idle' }, external('ready-b-outside', 'ready-b'),
    ] }));
    const names = (section: string) => [...container.querySelectorAll(`[data-section="${section}"] [data-row]`)].map((el) => el.textContent);
    expect(names('working')).toEqual(['working-anchor', 'working-outside']);
    expect(container.querySelector('[data-section="awaiting_input"]')).toBeNull();
    expect(names('idle')).toEqual(['ready-a', 'ready-a-outside']);
    expect(container.textContent).toContain('home_sessions_more_ready:1');
    await act(async () => [...container.querySelectorAll('button')].find((b) => b.textContent === 'home_sessions_more_ready:1')!.click());
    expect(names('idle')).toEqual(['ready-a', 'ready-a-outside', 'ready-b', 'ready-b-outside']);
    await act(async () => [...container.querySelectorAll('button')].find((b) => b.textContent === 'sessao_recolher')!.click());
    expect(names('idle')).toEqual(['ready-a', 'ready-a-outside']);
    expect(names('working')).toEqual(['working-anchor', 'working-outside']);
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

  it('filtro sem resultados mostra o aviso existente', async () => {
    await mount();
    await act(async () => {
      const input = container.querySelector<HTMLInputElement>('input[aria-label="lista_filtrar"]')!;
      input.value = 'not-found';
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    expect(container.querySelector('[data-row]')).toBeNull();
    expect(container.textContent).toContain('lista_vazia_filtro');
    expect(container.textContent).not.toContain('lista_nenhuma_ativa');
  });

  it('grupo recolhido não mostra aviso de lista vazia', async () => {
    await mount();
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label$="lista_sessoes_plural"]')!.click();
    });
    expect(container.querySelector('[data-row]')).toBeNull();
    expect(container.textContent).not.toContain('lista_vazia_filtro');
    expect(container.textContent).not.toContain('lista_nenhuma_ativa');
  });
});
