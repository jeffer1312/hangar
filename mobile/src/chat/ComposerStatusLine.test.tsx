// @vitest-environment happy-dom
import { act, createElement, forwardRef, useImperativeHandle, useState, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { StateEvent, SessionInfo } from '@hangar/core';
import type { LayoutChangeEvent, ViewProps, TextProps, PressableProps } from 'react-native';
import { overwriteGetLocale } from '../paraglide/runtime';
import { fitComposerItems } from './usage';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const native = vi.hoisted(() => ({ layouts: new Map<string, NonNullable<ViewProps['onLayout']>>(),
  props: new Map<string, ViewProps>(), fontScale: 1 }));
const state = vi.hoisted(() => ({ chat: { statusLine: null as string | null, stateEvent: null as StateEvent | null, stats: null },
  sessions: { rows: [] as (SessionInfo & { serverId: string })[], byServerRecord: {} as Record<string, SessionInfo[]> } }));
vi.mock('../stores/chat', () => ({ chatStore: () => ({ use: (select: (s: typeof state.chat) => unknown) => select(state.chat) }) }));
vi.mock('../stores/sessions', () => ({ useSessions: (select: (s: typeof state.sessions) => unknown) => select(state.sessions) }));
const credentials = vi.hoisted(() => ({ rows: [] as unknown[], calls: 0 }));
vi.mock('../stores/servers', () => ({ useServers: (select: (s: { servers: { id: string }[] }) => unknown) => select({ servers: [{ id: 's1' }] }) }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(),
  getCredentialsForServer: async () => { credentials.calls += 1; return credentials.rows; } }));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('@lodev09/react-native-true-sheet', () => ({ TrueSheet: forwardRef(function NativeSheet({ children, onDidDismiss }: {
  children: ReactNode; onDidDismiss?: () => void;
}, ref) {
  const [visible, setVisible] = useState(false);
  useImperativeHandle(ref, () => ({ present: async () => { setVisible(true); }, dismiss: async () => { setVisible(false); } }));
  return visible ? createElement('section', { 'data-testid': 'usage-sheet' }, children,
    createElement('button', { 'aria-label': 'close-sheet', onClick: () => { setVisible(false); onDidDismiss?.(); } })) : null;
}) }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {} }) }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {} }));
vi.mock('expo-blur', async () => ({ BlurView: (await import('react-native')).View }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('sonner-native', () => ({ toast: Object.assign(() => {}, { success: () => {}, error: () => {}, dismiss: () => {} }), Toaster: () => null }));
vi.mock('react-native', async (original) => {
  const actual = await original<typeof import('react-native')>();
  const props = (p: ViewProps & { children?: ReactNode }) => {
    if (p.testID) {
      native.props.set(p.testID, p);
      if (p.onLayout) native.layouts.set(p.testID, p.onLayout);
    }
    return { 'data-testid': p.testID, 'aria-label': p.accessibilityLabel,
      'aria-hidden': p.accessibilityElementsHidden || p.importantForAccessibility === 'no-hide-descendants' || undefined,
      style: Object.assign({}, ...([p.style] as unknown[]).flat(Infinity).filter(part => part && typeof part === 'object')) };
  };
  return { ...actual,
    useWindowDimensions: () => ({ width: 9999, height: 844, scale: 3, fontScale: native.fontScale }),
    View: (p: ViewProps) => createElement('div', props(p), p.children),
    Text: (p: TextProps) => createElement('span', { ...props(p), 'data-lines': p.numberOfLines, 'data-ellipsize': p.ellipsizeMode }, p.children),
    Pressable: ({ children, ...p }: PressableProps) => createElement('button', { ...props({ ...p, style: typeof p.style === 'function'
      ? p.style({ pressed: false }) : p.style }), onClick: () => p.onPress?.({} as import('react-native').GestureResponderEvent), disabled: p.disabled }, children as ReactNode),
    ScrollView: (p: ViewProps) => createElement('div', props(p), p.children),
  };
});

import { ComposerStatusLine } from './ComposerStatusLine';
const roots: Root[] = [];
const base = { serverId: 's1', name: 'sess' };
const widths: Record<string, number> = { folder: 400, branch: 350, diff: 62, time: 48, mode: 240, account: 280,
  ctx: 44, '5h': 44, '7d': 44, '30d': 52, cost: 44, usage: 100 };
const layout = (id: string, width: number) => native.layouts.get(id)?.({ nativeEvent: { layout: { x: 0, y: 0, width, height: 44 } } } as LayoutChangeEvent);
async function measure(available = 343, scale = 1) {
  await act(async () => {
    layout('composer-status-items', available);
    for (const [key, width] of Object.entries(widths)) layout(`composer-status-measure-${key}`, width * scale);
  });
}
async function render(props: Partial<Parameters<typeof ComposerStatusLine>[0]> = {}) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(createElement(ComposerStatusLine, { ...base, ...props })));
  await measure();
  return { container, root };
}
const item = (container: HTMLElement, key: string) => container.querySelector<HTMLElement>(`[data-testid="composer-status-${key}"]`);
const keys = (container: HTMLElement) => [...container.querySelectorAll('[data-testid="composer-status-items"] > [data-testid]')]
  .map(el => el.getAttribute('data-testid')!.replace('composer-status-', ''));
beforeEach(() => {
  overwriteGetLocale(() => 'pt');
  native.layouts.clear(); native.props.clear(); native.fontScale = 1;
  state.chat = { statusLine: '💬 ctx 0/100k │ ⚡5h:0% │ 📅7d:0% │ 🗓30d:0% │ 💵 $0.00 │ ⏱ 1m',
    stateEvent: { session: 'sess', state: 'idle', claude_permission_mode: 'bypassPermissions' }, stats: null };
  state.sessions = { rows: [{ name: 'sess', serverId: 's1', provider: 'claude', cwd: '/x/pasta-muito-longa',
    branch: 'branch-muito-longa', git_added: 7, git_removed: 2 } as SessionInfo & { serverId: string }], byServerRecord: {} };
});
afterEach(() => { act(() => roots.splice(0).forEach(root => root.unmount())); });

describe('conta da lista no rodapé', () => {
  it.each(['rows', 'byServerRecord'] as const)('usa conta da rota em %s e atualiza com a lista sem prop calculada pela tela', async (source) => {
    const routeSession = { ...state.sessions.rows[0], conta: 'claude:/home/test/.claude-route-account' };
    state.sessions.rows = [
      { ...routeSession, serverId: 's2', conta: 'claude:/home/test/.claude-other-server' },
      { ...routeSession, name: 'other', conta: 'claude:/home/test/.claude-other-session' },
      routeSession,
    ];
    if (source === 'byServerRecord') {
      state.sessions.byServerRecord = { s1: [state.sessions.rows[1], routeSession], s2: [state.sessions.rows[0]] };
      state.sessions.rows[2] = { ...routeSession, conta: 'claude:/home/test/.claude-stale-row' };
    }
    const { container, root } = await render({ onOpenSettings: vi.fn() });
    expect(item(container, 'account')?.textContent).toBe('route-account');
    const updated = { ...routeSession, conta: 'claude:/home/test/.claude-engine-account', engine: 'local-gpt' };
    if (source === 'byServerRecord') state.sessions.byServerRecord.s1 = [state.sessions.rows[1], updated];
    else state.sessions.rows = [...state.sessions.rows.slice(0, 2), updated];
    await act(async () => root.render(createElement(ComposerStatusLine, { ...base, onOpenSettings: vi.fn() })));
    await measure();
    expect(item(container, 'account')?.textContent).toBe('local-gpt · engine-account');
    expect(item(container, 'account')?.getAttribute('aria-label')).toBe('local-gpt · engine-account');
  });
});

describe('Antigravity no rodapé', () => {
  it('mostra a conta Google e a cota do grupo do modelo, lidas da conta', async () => {
    state.chat.statusLine = null;
    state.sessions.rows = [{ name: 'sess', serverId: 's1', provider: 'antigravity', conta: 'antigravity:/home/u/.gemini',
      model: 'gemini-pro-orchestrator-neon', cwd: '/x/hangar' } as SessionInfo & { serverId: string }];
    credentials.rows = [{ id: 'antigravity:/home/u/.gemini', tipo: 'antigravity', nome: 'Google (padrão)', cota: { estado: 'lida', janelas: [
      { rotulo: '7d', pct: 1.1, reset_ts: null, por_modelo: false }, { rotulo: '5h', pct: 4.5, reset_ts: null, por_modelo: false },
      { rotulo: 'Claude/GPT 7d', pct: 50, reset_ts: null, por_modelo: true }, { rotulo: 'Claude/GPT 5h', pct: 60, reset_ts: null, por_modelo: true }] } }];
    const { container } = await render();
    await measure();
    expect(item(container, 'account')?.textContent).toBe('Google (padrão)');
    expect(item(container, '5h')?.textContent).toBe('5h 5%');
    expect(item(container, '7d')?.textContent).toBe('7d 1%');
  });

  it('relê a cota quando o turno termina', async () => {
    state.chat.statusLine = null;
    state.chat.stateEvent = { session: 'sess', state: 'working' };
    state.sessions.rows = [{ name: 'sess', serverId: 's1', provider: 'antigravity', conta: 'antigravity:/home/u/.gemini',
      model: 'gemini-pro-orchestrator-neon', cwd: '/x/hangar' } as SessionInfo & { serverId: string }];
    credentials.rows = [];
    credentials.calls = 0;
    const { root } = await render();
    const before = credentials.calls;
    state.chat.stateEvent = { session: 'sess', state: 'idle' };
    await act(async () => root.render(createElement(ComposerStatusLine, base)));
    expect(credentials.calls).toBe(before + 1);
  });

  it('sessão do Claude não lê as contas: a cota vem da linha de status', async () => {
    credentials.calls = 0;
    await render();
    expect(credentials.calls).toBe(0);
  });
});

describe('ações do rodapé', () => {
  it('contexto e cota pedem a aba Uso da mesma folha de ajustes', async () => {
    const onOpenSettings = vi.fn();
    const { container } = await render({ onOpenSettings });
    await act(async () => item(container, 'ctx')!.click());
    expect(onOpenSettings).toHaveBeenLastCalledWith('usage');
    expect(container.querySelector('[data-testid="usage-sheet"]')).toBeNull();
    await act(async () => item(container, '7d')!.click());
    expect(onOpenSettings.mock.calls).toEqual([['usage'], ['usage']]);
  });

  it('modo conta contexto cotas e custo abrem a aba correspondente nos ajustes', async () => {
    const onOpenSettings = vi.fn();
    state.sessions.rows[0].conta = 'claude:/home/test/.claude-conta-muito-longa';
    const { container } = await render({ onOpenSettings });
    expect(item(container, 'mode')?.tagName).toBe('BUTTON');
    expect(item(container, 'account')?.tagName).toBe('BUTTON');
    expect(container.querySelector('[data-testid="usage-sheet"]')).toBeNull();
    await act(async () => { item(container, 'mode')!.click(); item(container, 'account')!.click(); });
    expect(onOpenSettings.mock.calls).toEqual([['mode'], ['account']]);
    expect(container.querySelector('[data-testid="usage-sheet"]')).toBeNull();
    for (const key of ['ctx', '5h', '7d', '30d', 'cost']) {
      await act(async () => item(container, key)!.click());
      expect(onOpenSettings).toHaveBeenLastCalledWith('usage');
      expect(container.querySelector('[data-testid="usage-sheet"]')).toBeNull();
    }
    expect(item(container, '5h')!.textContent).toBe('5h 0%');
    expect(item(container, '7d')!.textContent).toBe('7d 0%');
    expect(item(container, '30d')!.textContent).toBe('30d 0%');
    expect(item(container, 'ctx')!.textContent).toBe('0%');
    expect(item(container, 'cost')!.textContent).toBe('$0.00');
  });

  it('sem callback modo e conta são texto, não botões mortos, e a folha nem lê o store enquanto fechada', async () => {
    let statsReads = 0;
    Object.defineProperty(state.chat, 'stats', { configurable: true, get: () => { statsReads++; return null; } });
    state.sessions.rows[0].conta = 'claude:/home/test/.claude-conta';
    const { container } = await render();
    expect(item(container, 'mode')?.tagName).toBe('DIV');
    expect(item(container, 'account')?.tagName).toBe('DIV');
    expect(statsReads).toBe(0);
    await act(async () => item(container, 'cost')!.click());
    expect(statsReads).toBeGreaterThan(0);
  });

  it('sem statusline conserva pasta e branch da lista e permite abrir Uso por elas', async () => {
    state.chat.statusLine = null;
    const { container } = await render();
    expect(item(container, 'folder')?.textContent).toBe('pasta-muito-longa');
    expect(item(container, 'branch')?.textContent).toBe('branch-muito-longa');
    expect(item(container, 'diff')?.textContent).toBe('+7 −2');
    for (const key of ['folder', 'branch', 'diff']) {
      await act(async () => item(container, key)!.click());
      expect(container.querySelector('[data-testid="usage-sheet"]')).not.toBeNull();
      await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="close-sheet"]')!.click());
    }
  });

  it.each([
    ['claude', null, 'plan', 'Modo'], ['claude', 'acceptEdits', 'plan', 'Aceitar edições'],
    ['codex', 'bypassPermissions', null, 'Modo'], ['codex', 'bypassPermissions', 'plan', 'Planejar'],
    ['codex', 'plan', 'default', 'Normal'],
    ['antigravity', 'auto', null, 'Automático'], ['antigravity', null, null, 'Modo'],
  ] as const)('provider %s usa só seu estado de modo (%s, %s)', async (provider, claude, codex, label) => {
    state.sessions.rows[0].provider = provider;
    state.chat.stateEvent = { session: 'sess', state: 'idle', claude_permission_mode: claude,
      codex_mode: codex as StateEvent['codex_mode'] };
    const { container } = await render({ onOpenSettings: vi.fn() });
    expect(item(container, 'mode')?.textContent).toBe(label);
  });

  it('Antigravity sem statusline do Hangar mostra o contexto da lista', async () => {
    state.chat.statusLine = 'auto · Gemini 4 Neon · low';
    state.sessions.rows[0] = { ...state.sessions.rows[0], provider: 'antigravity', model: 'gemini-4-neon-low',
      context: { used: 250_000, 'window': 1_000_000 } };
    const { container } = await render({ onOpenSettings: vi.fn() });
    expect(item(container, 'ctx')?.textContent).toBe('25%');
  });

  it('outro provider não ganha modo de Claude, e conta ausente não cria item', async () => {
    state.sessions.rows[0].provider = 'kimi';
    const { container } = await render({ onOpenSettings: vi.fn() });
    expect(item(container, 'mode')).toBeNull();
    expect(item(container, 'account')).toBeNull();
  });
});

describe('continuidade do rodapé durante medição', () => {
  it.each([
    { change: 'cotas visíveis', available: 343, scale: 1,
      before: ['mode', 'account', 'ctx', '5h', '7d', '30d', 'cost'], after: ['mode', 'account', 'ctx', '5h', '7d', '30d', 'cost'] },
    { change: 'tempo e custo escondidos', available: 290, scale: 1,
      before: ['mode', 'account', '5h', '7d', '30d'], after: ['mode', 'account', '5h', '7d', '30d'] },
    { change: 'fonte maior', available: 343, scale: 2,
      before: ['mode', 'account', 'ctx', '5h', '7d', '30d', 'cost'], after: ['mode', 'account', '5h', '7d'] },
    { change: 'cota nova sem medida', available: 343, scale: 1,
      before: ['branch', 'mode', 'account', 'ctx', '5h', '7d', 'cost'], after: ['mode', 'account', 'ctx', '5h', '7d', '30d', 'cost'] },
  ])('preserva os mesmos botões antes de onLayout com $change e ajusta depois', async ({ change, available, scale, before, after }) => {
    const onOpenSettings = vi.fn();
    state.sessions.rows[0].conta = 'claude:/home/test/.claude-conta';
    if (change === 'cota nova sem medida') state.chat.statusLine = state.chat.statusLine!.replace(' │ 🗓30d:0%', '');
    const { container, root } = await render({ onOpenSettings });
    await measure(available);
    expect(keys(container)).toEqual(before);
    const buttons = new Map(before.map(key => [key, item(container, key)!]));
    if (change === 'tempo e custo escondidos') {
      state.chat.statusLine = state.chat.statusLine!.replace('$0.00', '$12.34').replace('1m', '2m');
    } else if (change === 'cota nova sem medida') {
      state.chat.statusLine += ' │ 🗓30d:25%';
    } else {
      state.chat.statusLine = state.chat.statusLine!.replace('5h:0%', '5h:25%');
    }
    native.fontScale = scale;
    await act(async () => root.render(createElement(ComposerStatusLine, { ...base, onOpenSettings })));
    // Antes de qualquer onLayout novo, a atualização não pode tirar o alvo sob o dedo/foco.
    expect(keys(container)).toEqual(before);
    for (const [key, node] of buttons) expect(item(container, key)).toBe(node);
    if (change === 'cota nova sem medida') expect(item(container, '30d')).toBeNull();
    else if (change !== 'tempo e custo escondidos') expect(item(container, '5h')!.textContent).toBe('5h 25%');
    await act(async () => { buttons.get('mode')!.click(); buttons.get('account')!.click(); });
    expect(onOpenSettings.mock.calls).toEqual([['mode'], ['account']]);
    await measure(available, scale);
    expect(keys(container)).toEqual(after);
    for (const key of after) if (buttons.has(key)) expect(item(container, key)).toBe(buttons.get(key));
    const visible = after.map(key => item(container, key)!);
    expect(visible.reduce((sum, node) => sum + parseFloat(node.style.width), 0) + (visible.length - 1) * 4).toBeLessThanOrEqual(available);
  });
});

describe('medidas e seleção do rodapé', () => {
  it.each([343, 398])('usa onLayout real de %s pt, uma linha e alvos sem sobreposição', async (available) => {
    state.sessions.rows[0].conta = 'claude:/home/test/.claude-conta-muito-longa';
    const { container } = await render({ onOpenSettings: vi.fn() });
    await measure(available);
    const expected = fitComposerItems(Object.entries(widths).filter(([key]) => key !== 'usage')
      .map(([key, width]) => ({ key, width, shrink: ['folder', 'branch', 'mode', 'account'].includes(key) })), available);
    expect(keys(container)).toEqual(Object.keys(expected));
    const buttons = [...container.querySelectorAll<HTMLElement>('[data-testid="composer-status-items"] > button')];
    expect(buttons.length).toBeGreaterThan(0);
    expect(buttons.reduce((sum, el) => sum + parseFloat(el.style.width), 0) + (buttons.length - 1) * 4).toBeLessThanOrEqual(available);
    for (const el of buttons) {
      expect(parseFloat(el.style.width)).toBe(expected[el.dataset.testid!.replace('composer-status-', '')]);
      expect(parseFloat(el.style.width)).toBeGreaterThanOrEqual(44);
      expect(parseFloat(el.style.minHeight)).toBeGreaterThanOrEqual(44);
      expect(native.props.get(el.dataset.testid!)!).not.toHaveProperty('hitSlop');
    }
    for (const text of container.querySelectorAll('[data-testid="composer-status-items"] span')) {
      expect(text.getAttribute('data-lines')).toBe('1');
      expect(text.getAttribute('data-ellipsize')).toBe('tail');
    }
    const line = container.querySelector<HTMLElement>('[data-testid="composer-status-items"]')!;
    expect(line.style.flexWrap).toBe('nowrap');
    expect(parseFloat(line.style.minWidth)).toBe(0);
    const measuring = container.querySelector<HTMLElement>('[data-testid="composer-status-measuring"]')!;
    expect(measuring.style.position).toBe('absolute');
    expect(measuring.style.opacity).toBe('0');
    expect(measuring.getAttribute('aria-hidden')).toBe('true');
    expect(native.props.get('composer-status-measuring')?.pointerEvents).toBe('none');
    expect(measuring.querySelector('button')).toBeNull();
  });

  it('fonte e rótulo novos refazem medidas; repetir a medida não altera a seleção', async () => {
    state.sessions.rows[0].conta = 'claude:/home/test/.claude-conta';
    const { container, root } = await render({ onOpenSettings: vi.fn() });
    native.fontScale = 2;
    state.sessions.rows = [{ ...state.sessions.rows[0], conta: 'claude:/home/test/.claude-outra-conta-ainda-maior' }];
    await act(async () => root.render(createElement(ComposerStatusLine, { ...base, onOpenSettings: vi.fn() })));
    await measure(343, 2);
    expect(keys(container)).toEqual(['mode', 'account', '5h', '7d']);
    expect(item(container, 'account')!.textContent).toBe('outra-conta-ainda-maior');
    const before = container.innerHTML;
    await measure(343, 2);
    expect(container.innerHTML).toBe(before);
  });
});
