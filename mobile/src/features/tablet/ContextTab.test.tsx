// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AggSession, Server, StateEvent } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { ContextTab } from './ContextTab';
import { ContextPanel } from './ContextPanel';
import { usePanelTabRequest } from './panelTab';
import { getLocale, overwriteGetLocale } from '../../paraglide/runtime';

const state = vi.hoisted(() => ({ statusLine: null as string | null, stateEvent: null as StateEvent | null, rows: [] as AggSession[] }));
const servers = vi.hoisted(() => ({ ready: true, servers: [
  { id: 'srv', label: 'Casa', baseUrl: 'https://casa.test', token: 'test' },
] as Server[] }));
const groupHooks = vi.hoisted(() => ({ servers: vi.fn(), identities: vi.fn() }));
vi.mock('../pair/identities', () => ({ useIdentities: () => {
  groupHooks.identities();
  return { identities: new Map([['srv', 'casa'], ['remote', 'escritorio']]) };
} }));
vi.mock('../../stores/servers', () => ({
  useServers: (select: (s: typeof servers) => unknown) => { groupHooks.servers(); return select(servers); },
}));
vi.mock('../../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('../files/FilesPanel', () => ({ FilesPanel: ({ name }: { name: string }) => createElement('div', null, `files-${name}`) }));
vi.mock('../activity/ActivityPanel', () => ({ ActivityPanel: ({ name, focused }: { name: string; focused: boolean }) =>
  createElement('div', { 'data-activity-focused': focused }, `background-${name}`) }));
vi.mock('../git/GitPanel', () => ({ GitPanel: ({ server, name }: { server: Server; name: string }) => createElement('div', null, `git-${server.id}-${name}`) }));
vi.mock('../../stores/chat', () => ({ chatStore: () => ({
  use: (select: (s: typeof state) => unknown) => select(state),
}) }));
vi.mock('../../stores/sessions', () => ({
  useSessions: (select: (s: typeof state) => unknown) => select(state),
}));
vi.mock('./ActionsGrid', () => ({ ActionsGrid: ({ serverId, name }: { serverId: string; name: string }) =>
  createElement('button', null, `action-${serverId}-${name}`),
}));
const reviewState = vi.hoisted(() => ({ summary: null as null | { state: string; badge: null | { text: string; tone: string } },
  review: null as null | { state: string; decisions: { id: string; title: string; choices: { id: string; label: string }[] }[] },
  choices: {} as Record<string, { choice: string; reason: string }> }));
vi.mock('../workReview/useWorkReview', () => ({ useWorkReview: () => reviewState }));
vi.mock('../workReview/WorkReviewSection', () => ({
  WorkReviewSection: () => createElement('div', null, 'work-review-section'),
  WorkReviewView: ({ name, tall }: { name: string; tall?: boolean }) => createElement('div', { 'data-tall': tall }, `work-review-${name}`),
  hasWorkReview: (s: typeof reviewState) => !!s.summary || !!s.review,
}));

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  state.statusLine = null;
  state.stateEvent = null;
  state.rows = [];
  servers.ready = true;
  servers.servers = [{ id: 'srv', label: 'Casa', baseUrl: 'https://casa.test', token: 'test' }];
  groupHooks.servers.mockClear();
  groupHooks.identities.mockClear();
  reviewState.summary = null;
  reviewState.review = null;
  reviewState.choices = {};
  usePanelTabRequest.setState({ request: null });
  container = document.createElement('div');
  root = createRoot(container);
});
afterEach(() => act(() => root.unmount()));
const onPick = vi.fn();
const render = (name = 'sess') => act(() => root.render(createElement(ContextTab, { serverId: 'srv', name, onPick })));
const session = (name: string, extra: Partial<AggSession> = {}): AggSession => ({
  name, serverId: 'srv', serverLabel: 'Casa', serverColor: '#999', state: 'idle', last_activity: 1, ...extra,
});

const renderPanel = (name = 'sess', serverId = 'srv', focused = true) => act(() => root.render(createElement(ContextPanel, { serverId, name, focused })));
const selectTab = (label: string) => {
  const tab = container.querySelector<HTMLButtonElement>(`[role="tab"][aria-label="${label}"]`);
  expect(tab).not.toBeNull();
  act(() => tab!.click());
};

describe('ContextPanel', () => {
  it('repassa o foco à atividade só na aba ativa e conserva a aba quando a conversa é coberta', () => {
    renderPanel();
    expect(container.querySelector('[data-activity-focused]')).toBeNull();
    selectTab(m.tablet_tab_background());
    const activity = container.querySelector('[data-activity-focused]')!;
    expect(activity.getAttribute('data-activity-focused')).toBe('true');
    renderPanel('sess', 'srv', false);
    expect(activity.getAttribute('data-activity-focused')).toBe('false');
    renderPanel();
    expect(container.querySelector('[data-activity-focused]')).toBe(activity);
    expect(activity.getAttribute('data-activity-focused')).toBe('true');
    selectTab(m.tablet_tab_context());
    expect(container.querySelector('[data-activity-focused]')).toBeNull();
  });

  it('monta só a aba ativa e deixa as outras abas acessíveis', () => {
    renderPanel();
    expect(container.textContent).toContain('action-srv-sess');
    expect(container.textContent).not.toContain('files-sess');
    selectTab(m.tablet_tab_files());
    expect(container.textContent).toContain('files-sess');
    expect(container.textContent).not.toContain('action-srv-sess');
    selectTab(m.tablet_tab_background());
    expect(container.textContent).toContain('background-sess');
    expect(container.textContent).not.toContain('files-sess');
    selectTab(m.tablet_tab_git());
    expect(container.textContent).toContain('git-srv-sess');
    expect(container.textContent).not.toContain('background-sess');
  });

  it('volta a Contexto ao trocar o nome da sessão ou o servidor', () => {
    renderPanel();
    selectTab(m.tablet_tab_files());
    renderPanel('outra');
    expect(container.textContent).toContain('action-srv-outra');
    expect(container.textContent).not.toContain('files-outra');
    selectTab(m.tablet_tab_background());
    renderPanel('outra', 'outro');
    expect(container.textContent).toContain('action-outro-outra');
  });

  // O mock do react-native repassa o `testID` como atributo cru.
  const badge = (id: string) => container.querySelector(`[testid="${id}"]`);

  it('a aba Revisão só existe com revisão anunciada, depois de Arquivos, e mostra a revisão inteira', () => {
    renderPanel();
    expect(container.querySelector(`[role="tab"][aria-label="${m.work_review_tab()}"]`)).toBeNull();
    reviewState.summary = { state: 'running', badge: null };
    renderPanel('outra');
    const labels = [...container.querySelectorAll('[role="tab"]')].map((tab) => tab.getAttribute('aria-label'));
    expect(labels).toEqual([m.tablet_tab_context(), m.tablet_tab_files(), m.work_review_tab(), m.tablet_tab_background(), m.tablet_tab_git()]);
    expect(badge('review-tab-dot')).not.toBeNull();
    selectTab(m.work_review_tab());
    expect(container.querySelector('[data-tall]')?.textContent).toBe('work-review-outra');
    expect(container.querySelector('[data-tall]')?.getAttribute('data-tall')).toBe('true');
    expect(container.textContent).not.toContain('action-srv-outra');
  });

  it('a marca da aba conta as pendentes com as escolhas e mostra ✓ quando concluída', () => {
    const choices = [{ id: 'ok', label: 'Igualar' }];
    reviewState.summary = { state: 'waiting', badge: { text: 'mock: 2 para você', tone: 'warning' } };
    renderPanel();
    expect(badge('review-tab-dot')).not.toBeNull();
    reviewState.review = { state: 'waiting', decisions: [{ id: '1', title: 'a', choices }, { id: '2', title: 'b', choices }] };
    reviewState.choices = { 1: { choice: 'ok', reason: '' } };
    renderPanel('outra');
    expect(badge('review-tab-count')?.textContent).toBe('1');
    reviewState.review = { state: 'done', decisions: [] };
    renderPanel('mais');
    expect(badge('review-tab-done')?.textContent).toBe('✓');
  });

  it('a revisão sumiu com a aba aberta: volta ao Contexto', () => {
    reviewState.summary = { state: 'running', badge: null };
    renderPanel();
    selectTab(m.work_review_tab());
    expect(container.textContent).toContain('work-review-sess');
    reviewState.summary = null;
    renderPanel();
    expect(container.textContent).toContain('action-srv-sess');
    expect(container.querySelector(`[role="tab"][aria-label="${m.work_review_tab()}"]`)).toBeNull();
  });

  it('o chip da revisão abre a sessão já na aba Revisão e o pedido vale uma vez', () => {
    reviewState.summary = { state: 'waiting', badge: null };
    act(() => usePanelTabRequest.getState().open('srv', 'outra', 'review'));
    renderPanel();
    expect(container.textContent).toContain('action-srv-sess');
    renderPanel('outra');
    expect(container.textContent).toContain('work-review-outra');
    expect(usePanelTabRequest.getState().request).toBeNull();
    // Com o painel já na sessão, o pedido troca a aba na hora.
    selectTab(m.tablet_tab_files());
    act(() => usePanelTabRequest.getState().open('srv', 'outra', 'review'));
    expect(container.textContent).toContain('work-review-outra');
  });

  it('espera os servidores carregarem e mostra erro claro quando o servidor sumiu', () => {
    servers.servers = [];
    servers.ready = false;
    renderPanel();
    selectTab(m.tablet_tab_git());
    expect(container.textContent).toContain('loading');
    servers.ready = true;
    renderPanel();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(m.chat_servidor_removido());
    expect(container.textContent).not.toContain('git-srv-sess');
    selectTab(m.tablet_tab_context());
    expect(container.textContent).toContain('loading');
  });
});
describe('ContextTab', () => {
  it('sem navegação mostra os membros como texto, sem botão ou anúncio de abrir', () => {
    state.rows = [session('sess', { pair_gid: 'g' }), session('par', { pair_gid: 'g' })];
    act(() => root.render(createElement(ContextTab, { serverId: 'srv', name: 'sess' })));
    expect(container.textContent).toContain(m.native_group_panel_title({ count: 2 }));
    expect(container.textContent).toContain('par');
    expect(container.querySelector('[role="button"]')).toBeNull();
    expect(container.querySelector('[aria-label^="' + m.linked_children_open() + '"]')).toBeNull();
  });

  it('só assina servidores e identidades enquanto o cartão de grupo está montado', () => {
    state.rows = [session('sess')];
    render();
    expect(groupHooks.servers).not.toHaveBeenCalled();
    expect(groupHooks.identities).not.toHaveBeenCalled();
    state.rows = [session('sess', { pair_gid: 'g', pair_peers: ['desligado::ausente'] })];
    render();
    expect(groupHooks.servers).toHaveBeenCalled();
    expect(groupHooks.identities).toHaveBeenCalled();
    groupHooks.servers.mockClear();
    groupHooks.identities.mockClear();
    state.rows = [session('sess')];
    render();
    expect(groupHooks.servers).not.toHaveBeenCalled();
    expect(groupHooks.identities).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain('desligado::ausente');
  });

  it.each(['pt', 'en'] as const)('mostra grupo entre máquinas, vínculos locais e membros sem acesso (%s)', locale => {
    const resolveLocale = getLocale;
    overwriteGetLocale(() => locale);
    onPick.mockClear();
    try {
      const pair = { pair_gid: 'g', pair_owner: 'casa', pair_task: 'PM-123 tela',
        pair_peers: ['desligado::ausente'], pair_external: { alias: 'visitante', owner: 'Outra pessoa', session: 'externa' } };
      state.rows = [
        session('sess', { ...pair, state: 'working' }),
        session('filha', { ...pair, linked_by: 'sess', state: 'working', provider: 'codex', model: 'gpt-6', last_reply: 'resposta-filha' }),
        session('neta', { ...pair, linked_by: 'filha', state: 'working', limited: true }),
        session('remota', { ...pair, serverId: 'remote', serverLabel: 'Escritório', linked_by: 'sess', state: 'working' }),
      ];
      render();
      expect(container.textContent).toContain(m.native_group_panel_title({ count: 6 }));
      expect(container.textContent).toContain(m.native_group_panel_working({ count: 2 }));
      expect(container.textContent).toContain('PM-123 tela');
      expect(container.textContent).toContain('Escritório');
      expect(container.textContent).toContain('resposta-filha');
      expect(container.textContent).toContain('gpt-6');
      expect(container.textContent).toContain(m.native_group_panel_child_of_here());
      expect(container.textContent).toContain(m.native_group_panel_child_of({ name: 'filha' }));
      expect(container.textContent).toContain('desligado::ausente');
      expect(container.textContent).toContain('visitante::externa');
      const members = [...container.querySelectorAll<HTMLButtonElement>('button[aria-label]')];
      expect(members).toHaveLength(3);
      expect(members.map(button => button.textContent)).toEqual([
        expect.stringContaining('filha'), expect.stringContaining('neta'), expect.stringContaining('remota'),
      ]);
      expect(members[2].textContent).not.toContain(m.native_group_panel_child_of_here());
      act(() => members[2].click());
      expect(onPick).toHaveBeenCalledWith(state.rows[3]);
      render('filha');
      expect(container.textContent).toContain(m.native_group_panel_parent_of_here());
      expect(container.textContent).toContain(m.native_group_panel_child_of_here());
    } finally {
      overwriteGetLocale(resolveLocale);
    }
  });

  it('não mostra painel sem grupo e conta os inacessíveis mesmo sem outro membro abrível', () => {
    state.rows = [session('sess')];
    render();
    expect(container.textContent).not.toContain(m.native_group_panel_title({ count: 2 }));
    state.rows = [session('sess', { pair_gid: 'g', pair_peers: ['desligado::ausente'] })];
    render();
    expect(container.textContent).toContain(m.native_group_panel_title({ count: 2 }));
    expect(container.textContent).toContain('desligado::ausente');
    expect(container.textContent).not.toContain(m.native_group_panel_working({ count: 0 }));
    expect(container.querySelector('button[aria-label]')).toBeNull();
    state.rows = [session('sess', { pair_gid: 'g' })];
    render();
    expect(container.textContent).not.toContain(m.native_group_panel_title({ count: 1 }));
  });

  it('o painel passa ao grupo a mesma navegação da barra e desaparece ao trocar para sessão sem grupo', () => {
    state.rows = [session('sess', { pair_gid: 'g' }), session('par', { pair_gid: 'g' }), session('sozinha')];
    act(() => root.render(createElement(ContextPanel, { serverId: 'srv', name: 'sess', focused: true, onPick })));
    const member = container.querySelector<HTMLButtonElement>('button[aria-label^="' + m.linked_children_open() + '"]')!;
    act(() => member.click());
    expect(onPick).toHaveBeenCalledWith(state.rows[1]);
    renderPanel('sozinha');
    expect(container.textContent).not.toContain(m.native_group_panel_title({ count: 2 }));
  });

  it('mostra o contexto, custo, cotas, repositório e branch da statusline', () => {
    state.statusLine = '💬 ctx 940k/1M │ 💵 $12.34 │ ⚡5h:23% ↺1h55m │ 📅7d:74% │ 📁 hangar [tab/t7]';
    render();
    expect(container.textContent).toContain('94%');
    expect(container.textContent).toContain('tab/t7');
    expect(container.textContent).toContain('hangar');
    expect(container.textContent).toContain('$12.34');
    expect(container.textContent).toContain('23%');
    expect(container.textContent).toContain('74%');
    expect(container.textContent).toContain(m.uso_reset({ quando: '1h55m' }));
    expect(container.querySelector('svg')).toBeNull();
    expect(container.querySelector(`[aria-label="${m.tablet_actions()}"]`)).toBeNull();
  });

  it('a revisão do trabalho não fica no Contexto: tem aba própria', () => {
    reviewState.summary = { state: 'waiting', badge: null };
    render();
    expect(container.textContent).not.toContain('work-review');
    expect(container.textContent).toContain('action-srv-sess');
  });

  it('antes do primeiro estado mostra carregamento e conserva as ações', () => {
    render();
    expect(container.textContent).toContain('loading');
    expect(container.textContent).not.toContain(m.tablet_context_empty());
    expect(container.textContent).toContain('action-srv-sess');
  });

  it('ao receber estado sem statusline mostra o vazio e conserva as ações', () => {
    render();
    expect(container.textContent).toContain('loading');
    state.stateEvent = { session: 'sess', state: 'idle' };
    render();
    expect(container.textContent).toContain(m.tablet_context_empty());
    expect(container.textContent).not.toContain('loading');
    expect(container.textContent).toContain(m.tablet_actions());
    expect(container.textContent).toContain('action-srv-sess');
  });

  it.each(['', '💵 $0.00'])('com statusline %j sem contexto mostra o vazio e conserva as ações', (statusLine) => {
    state.statusLine = statusLine;
    state.stateEvent = { session: 'sess', state: 'idle', status_line: statusLine };
    render();
    expect(container.textContent).toContain(m.tablet_context_empty());
    expect(container.textContent).toContain(m.tablet_actions());
    expect(container.textContent).toContain('action-srv-sess');
  });
});
