// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AggSession, ServerBucket } from '@hangar/core';
import type { SessionsBarPlacement } from '../../stores/tabletPrefs';
import { Alert } from 'react-native';

const state = vi.hoisted(() => ({
  rows: [] as AggSession[], byServer: [] as ServerBucket[], loading: false, keyboardVisible: false,
  retain: vi.fn(), release: vi.fn(), collapsed: false, toggleCollapsed: vi.fn(),
  tabsDetail: 'compact', tabsWorkingFirst: false, tabsRecentFirst: false,
}));
vi.mock('../../stores/sessions', () => ({ useSessions: Object.assign(
  (selector: (s: typeof state) => unknown) => selector(state), { getState: () => state },
) }));
vi.mock('../../stores/servers', () => ({ useServers: (selector: (s: { servers: [] }) => unknown) => selector({ servers: [] }) }));
vi.mock('../../stores/tabletPrefs', () => ({ useTabletPrefs: (selector: (s: typeof state) => unknown) => selector(state) }));
vi.mock('../pair/identities', () => ({ useIdentities: () => ({ identities: new Map() }) }));
vi.mock('react-native-keyboard-controller', () => ({
  useKeyboardState: (selector: (s: { isVisible: boolean }) => unknown) => selector({ isVisible: state.keyboardVisible }),
}));
vi.mock('../../ui/Glass', () => ({ Glass: ({ children, style }: { children: ReactNode; style: unknown }) =>
  createElement('div', { 'data-glass': true, style: Object.assign({}, ...[style].flat(Infinity).filter(Boolean)) }, children) }));
vi.mock('../../ui/Icon', () => ({ Icon: ({ name }: { name: string }) => createElement('i', { 'data-icon': name }) }));
vi.mock('../sessions/SessionAvatar', () => ({ SessionAvatar: ({ session, size }: { session: Partial<AggSession>; size: number }) =>
  createElement('i', { 'data-avatar': session.provider, 'data-state': session.state, 'data-engine': session.engine, 'data-size': size }) }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children, open }: { children: ReactNode; open: boolean }) =>
  open ? createElement('div', { role: 'dialog' }, children) : null }));
vi.mock('../sessions/SessionRow', () => ({ SessionRow: ({ session, onPress, mostrarServidor }: {
  session: AggSession; onPress: (s: AggSession) => void; mostrarServidor: boolean;
}) => createElement('button', { onClick: () => onPress(session) }, session.name, mostrarServidor ? session.serverLabel : '') }));
vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return { ...native,
    View: ({ children, style, testID, accessibilityLabel }: { children?: ReactNode; style?: unknown; testID?: string; accessibilityLabel?: string }) => createElement('div', { 'data-testid': testID, 'aria-label': accessibilityLabel, style: Object.assign({}, ...[style].flat(Infinity).filter(Boolean)) }, children),
    Text: ({ children, style }: { children?: ReactNode; style?: unknown }) => createElement('span', {
      style: Object.assign({}, ...[style].flat(Infinity).filter(Boolean)),
    }, children),
    Pressable: ({ children, onPress, onLongPress, style, testID, accessibilityLabel, accessibilityState }: {
      children?: ReactNode; onPress?: () => void; accessibilityLabel?: string; accessibilityState?: { selected?: boolean };
      onLongPress?: () => void; style?: unknown; testID?: string;
    }) => createElement('button', { onClick: onPress, onContextMenu: onLongPress, 'data-testid': testID,
      style: Object.assign({}, ...[typeof style === 'function' ? style({ pressed: false }) : style].flat(Infinity).filter(Boolean)),
      'aria-label': accessibilityLabel, 'aria-selected': accessibilityState?.selected }, children),
  };
});

import { SessionsBar } from './SessionsBar';
import { SessionChip } from './SessionChip';
import { useUnistyles } from 'react-native-unistyles';
import * as m from '../../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const session: AggSession = { name: 'api', serverId: 'nb', serverLabel: 'Notebook', serverColor: '#999', state: 'idle', last_activity: 1 };
let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  vi.clearAllMocks();
  state.rows = [session]; state.byServer = []; state.loading = false; state.keyboardVisible = false; state.collapsed = false;
  state.tabsDetail = 'compact';
  state.tabsWorkingFirst = false; state.tabsRecentFirst = false;
  state.retain.mockReturnValue(state.release);
  container = document.createElement('div'); root = createRoot(container);
});
afterEach(() => { act(() => root.unmount()); });
const render = (placement: SessionsBarPlacement, onPick = vi.fn(), onNew = vi.fn()) => {
  act(() => root.render(createElement(SessionsBar, { placement, selected: { serverId: 'nb', name: 'api' }, onPick, onNew })));
  return { onPick, onNew };
};
const click = (label: string) => {
  const button = [...container.querySelectorAll('button')].find(b => b.getAttribute('aria-label') === label || b.textContent === label);
  expect(button).toBeDefined();
  act(() => button!.click());
};

describe('SessionsBar', () => {
  it.each(['left', 'bottom', 'top'] as const)('%s aplica as preferências de ordem ao vivo', placement => {
    state.rows = [
      { ...session, name: 'pergunta', state: 'awaiting_input', last_reply_at: 2 },
      { ...session, name: 'ocupada', state: 'working', last_reply_at: 1 },
      { ...session, name: 'limite', state: 'working', limited: true, last_reply_at: 50 },
      { ...session, name: 'recente', last_reply_at: 30 },
    ];
    const names = () => [...container.querySelectorAll('button')].flatMap(button => {
      const label = `${button.getAttribute('aria-label') ?? ''} ${button.textContent}`;
      return state.rows.filter(s => label.includes(s.name)).map(s => s.name);
    });
    render(placement);
    expect(names()[0]).toBe('pergunta');
    state.tabsWorkingFirst = true;
    render(placement);
    expect(names()).toEqual(['ocupada', 'pergunta', 'limite', 'recente']);
    state.tabsWorkingFirst = false; state.tabsRecentFirst = true;
    render(placement);
    expect(names()).toEqual(['ocupada', 'limite', 'recente', 'pergunta']);
  });

  it('não monta a barra de baixo com o teclado visível e volta ao fechar', () => {
    state.keyboardVisible = true;
    render('bottom'); expect(container.innerHTML).toBe('');
    state.keyboardVisible = false;
    render('bottom'); expect(container.textContent).toContain(session.name);
  });
  it('conserva a barra de cima com o teclado aberto', () => {
    state.keyboardVisible = true;
    render('top'); expect(container.textContent).toContain(session.name);
  });
  it('mostra vazio com nova sessão disponível', () => {
    state.rows = [];
    const { onNew } = render('bottom');
    expect(container.textContent).toContain(m.tablet_sessions_empty());
    click(m.tablet_new_session()); expect(onNew).toHaveBeenCalledOnce();
  });
  it('mostra carregamento antes de declarar a lista vazia', () => {
    state.rows = []; state.loading = true;
    render('left');
    expect(container.textContent).toContain('loading');
    expect(container.textContent).not.toContain(m.tablet_sessions_empty());
  });
  it('com a lista vazia mostra a falha do servidor sem tirar nova sessão', () => {
    state.rows = [];
    state.byServer = [{ server: { id: 'nb', label: 'Notebook', baseUrl: '', token: '' }, sessions: [], loaded: false, error: 'falha-conexao' }];
    render('bottom');
    expect(container.textContent).toContain('falha-conexao');
    expect(container.textContent).toContain(m.tablet_sessions_empty());
    expect(container.querySelector(`[aria-label="${m.tablet_new_session()}"]`)).not.toBeNull();
  });
  it('máquina desligada há tempo não fica avisando, e a lista vazia diz que as máquinas estão fora', () => {
    state.rows = [];
    state.byServer = [{ server: { id: 'nb', label: 'Notebook', baseUrl: '', token: '' }, sessions: [], loaded: false,
      error: 'offline', offlineSince: Date.now() - 3_600_000 }];
    render('bottom');
    expect(container.textContent).not.toContain(m.lista_servidor_offline({ label: 'Notebook' }));
    expect(container.textContent).toContain(m.home_machines_offline());
    expect(container.textContent).not.toContain(m.tablet_sessions_empty());
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });
  it('máquina que acabou de cair avisa em vermelho, sem o texto cinza de todas fora', () => {
    state.rows = [];
    state.byServer = [{ server: { id: 'nb', label: 'Notebook', baseUrl: '', token: '' }, sessions: [], loaded: false,
      error: 'offline', offlineSince: Date.now() }];
    render('bottom');
    expect(container.textContent).toContain(m.lista_servidor_offline({ label: 'Notebook' }));
    expect(container.textContent).not.toContain(m.home_machines_offline());
    expect(container.textContent).not.toContain(m.tablet_sessions_empty());
  });
  it.each(['left', 'bottom', 'top'] as const)('%s troca a sessão pela identidade completa', placement => {
    state.rows = [session, { ...session, serverId: 'casa', serverLabel: 'Casa' }];
    const { onPick } = render(placement);
    const buttons = [...container.querySelectorAll('button')];
    const other = buttons.find(b => `${b.getAttribute('aria-label')} ${b.textContent}`.includes('Casa'))!;
    expect(other).toBeDefined();
    act(() => other.click());
    expect(onPick).toHaveBeenCalledWith(state.rows[1]);
  });
  it.each(['left', 'bottom', 'top'] as const)('%s recolhe pelo botão sem escolher posição', placement => {
    render(placement); click(m.tablet_bar_collapse());
    expect(state.toggleCollapsed).toHaveBeenCalledOnce();
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(container.textContent).not.toContain(m.tablet_bar_position());
  });
  it.each(['top', 'bottom'] as const)('%s recolhida não monta vidro nem cartões', placement => {
    state.collapsed = true; render(placement);
    expect(container.innerHTML).toBe('');
  });
  it('esquerda recolhida mantém um trilho de 64 pt com sessões por ícone e botão para expandir', () => {
    state.collapsed = true;
    state.rows = [session, { ...session, serverId: 'casa', serverLabel: 'Casa' }];
    const { onPick } = render('left');
    expect((container.querySelector('[data-glass]') as HTMLElement).style.width).toBe('64px');
    expect(container.textContent).not.toContain(session.name);
    act(() => container.querySelector<HTMLButtonElement>('button[aria-label*="Casa"]')!.click());
    expect(onPick).toHaveBeenCalledWith(state.rows[1]);
    click(m.tablet_bar_expand()); expect(state.toggleCollapsed).toHaveBeenCalledOnce();
    state.collapsed = false; render('left');
    expect((container.querySelector('[data-glass]') as HTMLElement).style.width).toBe('250px');
    expect(container.textContent).toContain(session.name);
  });
  it('retém a lista compartilhada uma vez e libera ao desmontar', () => {
    render('bottom'); render('left');
    expect(state.retain).toHaveBeenCalledOnce();
    act(() => root.render(null)); expect(state.release).toHaveBeenCalledOnce();
  });
  it.each(['top', 'bottom'] as const)('%s reúne o grupo numa aba com a tarefa acessível e abre cada membro pelo toque', placement => {
    state.rows = [
      { ...session, pair_gid: 'g', pair_task: '  PM-123 tela  ' },
      { ...session, name: 'filha', pair_gid: 'g', linked_by: 'api', state: 'working' },
    ];
    const { onPick } = render(placement);
    expect(container.querySelectorAll('[data-testid="session-tab"]')).toHaveLength(1);
    const parent = container.querySelector<HTMLButtonElement>('button[aria-label^="api,"]')!;
    expect(parent.getAttribute('aria-label')).toContain('PM-123 tela');
    act(() => parent.click());
    expect(onPick).toHaveBeenLastCalledWith(state.rows[0]);
    const child = container.querySelector<HTMLButtonElement>('button[aria-label^="filha,"]')!;
    act(() => child.click());
    expect(onPick).toHaveBeenLastCalledWith(state.rows[1]);
  });
  it.each(['top', 'bottom'] as const)('%s sem tarefa não inventa rótulo com nomes das sessões', placement => {
    state.rows = [{ ...session, pair_gid: 'g' }, { ...session, name: 'filha', pair_gid: 'g' }];
    render(placement);
    expect([...container.querySelectorAll('span')].some(el => el.textContent === 'api, filha')).toBe(false);
  });
  it('preserva o bloco montado pelo gid quando os estados dos membros mudam', () => {
    state.rows = [{ ...session, pair_gid: 'g', state: 'working' }, { ...session, name: 'web', pair_gid: 'g' }];
    render('bottom');
    const before = container.querySelector('button[aria-label^="api,"]')!.closest('[data-testid="session-tab"]');
    expect(before).not.toBeNull();
    state.rows = [{ ...state.rows[0], state: 'idle' }, { ...state.rows[1], state: 'awaiting_input' }];
    render('bottom');
    const after = container.querySelector('button[aria-label^="api,"]')!.closest('[data-testid="session-tab"]');
    expect(after).toBe(before);
  });
});

it('SessionChip identifica a máquina com o ícone e o nome dela', () => {
  const onPick = vi.fn();
  act(() => root.render(createElement(SessionChip, { session, selected: session, onPick })));
  expect(container.textContent).toContain('Notebook');
  expect(container.querySelector('[data-icon="Monitor"]')).not.toBeNull();
  expect(container.querySelector('button')?.getAttribute('aria-selected')).toBe('true');
  act(() => container.querySelector('button')!.click());
  expect(onPick).toHaveBeenCalledWith(session);
});

it.each(['top', 'bottom'] as const)('%s tem abas de 240 pt, máquina na borda e metadados só no modo completo', placement => {
  state.rows = [{ ...session, cwd: '/work/hangar', branch: 'feat/tablet', git_ahead: 2, git_behind: 3 },
    { ...session, name: 'muito-maior-que-api', serverId: 'vm' }];
  render(placement);
  const tabs = [...container.querySelectorAll<HTMLElement>('[data-testid="session-tab"]')];
  expect(tabs).toHaveLength(2);
  expect(tabs.map(tab => tab.style.width)).toEqual(['240px', '240px']);
  expect(container.querySelector<HTMLElement>('[data-glass]')!.style.height).toBe('74px');
  expect(tabs[0].firstElementChild!.getAttribute('style')).toContain('height: 44px');
  expect(tabs[0].style.paddingTop).toBe('8px');
  expect(tabs[0].style.paddingBottom).toBe('22px');
  expect(container.querySelector('[data-size="22"]')).not.toBeNull();
  expect([...tabs[0].querySelectorAll('span')].find(el => el.textContent === session.name)!.style.fontSize)
    .toBe(`${useUnistyles().theme.base.text.sm}px`);
  expect(container.querySelector<HTMLElement>('[data-testid="machine-legend"]')!.style.position).toBe('absolute');
  expect(container.textContent).not.toContain('feat/tablet');
  state.tabsDetail = 'full'; render(placement);
  expect(container.querySelector<HTMLElement>('[data-glass]')!.style.height).toBe('82px');
  expect(tabs[0].firstElementChild!.getAttribute('style')).toContain('height: 52px');
  for (const text of ['hangar', 'feat/tablet', '↑2', '↓3']) expect(container.textContent).toContain(text);
  expect(container.querySelector('button[aria-label^="api,"]')!.getAttribute('aria-label')).toContain(m.git_sync_titulo({ ahead: 2, behind: 3 }));
});

it.each(['compact', 'full'])('%s mostra perguntas pendentes da principal e filhas e o selo só no completo', detail => {
  state.tabsDetail = detail;
  state.rows = [
    { ...session, headless: true, pending_questions: 2 },
    { ...session, name: 'api-question', linked_by: 'api', pending_questions: 3 },
    { ...session, name: 'api-waiting', linked_by: 'api', state: 'awaiting_input' },
    { ...session, name: 'api-limited', linked_by: 'api', limited: true },
    { ...session, name: 'api-idle', linked_by: 'api' },
  ];
  render('bottom');
  expect(container.textContent).toContain('? 5');
  const parent = container.querySelector('button[aria-label^="api,"]')!;
  expect(parent.getAttribute('aria-label')).toContain(`${m.ask_perguntas()}: 5`);
  expect(container.querySelector(`[aria-label="${m.criar_modo_exec_headless()}"]`) !== null).toBe(detail === 'full');
  if (detail === 'full') expect(parent.getAttribute('aria-label')).toContain(m.criar_modo_exec_headless());
  state.rows = [{ ...session, headless: false, pending_questions: 0 }];
  render('bottom');
  expect(container.textContent).not.toContain('? ');
  expect(container.querySelector(`[aria-label="${m.criar_modo_exec_headless()}"]`)).toBeNull();
});

it('oferece três filhas com prioridade a perguntas e acesso a todas pelo contador por agente', () => {
  state.rows = [session, ...['api-backend', 'api-front', 'api-tools', 'api-question'].map((name, i) => ({
    ...session, name, linked_by: 'api', provider: i < 2 ? 'claude' as const : 'codex' as const,
    state: i === 3 ? 'awaiting_input' as const : 'working' as const,
  }))];
  const { onPick } = render('bottom');
  const chips = [...container.querySelectorAll<HTMLButtonElement>('[data-testid="child-chip"]')];
  expect(chips).toHaveLength(3);
  expect(chips[0].getAttribute('aria-label')).toContain('api-question');
  expect(chips[0].querySelector('[data-avatar="codex"][data-state="awaiting_input"]')).not.toBeNull();
  for (const chip of chips) expect(parseFloat(chip.style.minHeight)).toBeGreaterThanOrEqual(44);
  const counter = container.querySelector<HTMLButtonElement>('[data-testid="agent-count"]')!;
  expect(counter.getAttribute('aria-label')).toContain('Claude: 2');
  expect(counter.getAttribute('aria-label')).toContain('Codex: 2');
  expect(container.querySelector('[role="dialog"]')).toBeNull();
  act(() => counter.click());
  const dialog = container.querySelector('[role="dialog"]')!;
  expect(dialog).not.toBeNull();
  const hidden = dialog.querySelector<HTMLButtonElement>('button[aria-label^="api-tools,"]')!;
  expect(hidden).not.toBeNull();
  act(() => hidden.click());
  expect(onPick).toHaveBeenCalledWith(state.rows[3]);
  expect(container.querySelector('[role="dialog"]')).toBeNull();
});

it.each(['compact', 'full'])('%s conserva os alvos das filhas dentro do slot e da barra com o visual na borda', detail => {
  state.tabsDetail = detail;
  state.rows = [session, { ...session, name: 'api-child', linked_by: 'api' }];
  const { onPick } = render('bottom');
  const slot = container.querySelector<HTMLElement>('[data-testid="session-tab"]')!;
  const tab = slot.firstElementChild as HTMLElement;
  const child = slot.querySelector<HTMLButtonElement>('[data-testid="child-chip"]')!;
  const targets = child.parentElement!;
  const slotHeight = parseFloat(slot.style.paddingTop) + parseFloat(tab.style.height) + parseFloat(slot.style.paddingBottom);
  const targetHeight = parseFloat(child.style.minHeight);
  const bottom = parseFloat(targets.style.bottom);
  expect(targetHeight).toBeGreaterThanOrEqual(44);
  expect(parseFloat(child.style.width)).toBeGreaterThanOrEqual(44);
  expect(bottom).toBeGreaterThanOrEqual(0);
  expect(slotHeight - bottom - targetHeight).toBeGreaterThanOrEqual(0);
  expect(slotHeight).toBeLessThanOrEqual(parseFloat(container.querySelector<HTMLElement>('[data-glass]')!.style.height));
  expect(slotHeight - bottom - targetHeight / 2).toBe(parseFloat(slot.style.paddingTop) + parseFloat(tab.style.height));
  act(() => child.click());
  expect(onPick).toHaveBeenCalledExactlyOnceWith(state.rows[1]);
});

it('toque longo revela nome completo, máquina e estado; selecionar uma filha realça a aba do grupo', () => {
  const alert = vi.spyOn(Alert, 'alert');
  state.rows = [session, { ...session, name: 'api-filha-com-nome-inteiro-muito-longo', linked_by: 'api', state: 'working' }];
  const onPick = vi.fn();
  act(() => root.render(createElement(SessionsBar, { placement: 'top', selected: { serverId: 'nb', name: state.rows[1].name }, onPick, onNew: vi.fn() })));
  const parent = container.querySelector<HTMLButtonElement>('button[aria-label^="api,"]')!;
  const child = container.querySelector<HTMLButtonElement>('[data-testid="child-chip"]')!;
  expect(parent.getAttribute('aria-selected')).toBe('true');
  expect(child.getAttribute('aria-selected')).toBe('true');
  expect(child.textContent).not.toContain(state.rows[1].name);
  act(() => child.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true })));
  expect(alert).toHaveBeenCalledWith(state.rows[1].name, expect.stringContaining('Notebook'));
  expect(onPick).not.toHaveBeenCalled();
  alert.mockRestore();
});
