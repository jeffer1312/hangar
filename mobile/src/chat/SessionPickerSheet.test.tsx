// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot } from 'react-dom/client';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AggSession, ServerBucket } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const calls = vi.hoisted(() => ({
  replace: vi.fn(),
  release: vi.fn(),
  retain: vi.fn(),
  reads: 0,
  loading: false,
  byServer: [] as ServerBucket[],
  scroll: {} as { style?: unknown; contentContainerStyle?: unknown; keyboardShouldPersistTaps?: string; keyboardDismissMode?: string },
  keyboard: {} as { behavior?: string; automaticOffset?: boolean; style?: unknown },
  dismiss: null as null | (() => void),
  rows: [
    { name: 'shared', state: 'idle', serverId: 'srv1', serverLabel: 'first-server', serverColor: '#000' },
    { name: 'shared', state: 'idle', serverId: 'srv2', serverLabel: 'second-server', serverColor: '#000' },
  ] as AggSession[],
}));
vi.mock('../stores/sessions', () => ({
  useSessions: Object.assign((select: (state: { rows: AggSession[]; byServer: ServerBucket[]; loading: boolean }) => unknown) => {
    calls.reads++;
    return select({ rows: calls.rows, byServer: calls.byServer, loading: calls.loading });
  }, {
    getState: () => ({ retain: () => { calls.retain(); return calls.release; } }),
  }),
}));
vi.mock('expo-router', () => ({ useRouter: () => ({ replace: calls.replace }) }));
// Como o TrueSheet: os filhos ficam montados com a folha fechada; o fim do fechamento avisa por onDismiss.
vi.mock('../ui/Sheet', () => ({ Sheet: ({ children, onDismiss }: { children: React.ReactNode; onDismiss: () => void }) => {
  calls.dismiss = onDismiss;
  return children;
} }));
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: React.PropsWithChildren) =>
  React.createElement('div', { 'data-menu': true }, children) }));
vi.mock('../ui/Icon', () => ({ Icon: ({ name, color }: { name: string; color: string }) =>
  React.createElement('span', { 'data-icon': name, style: { color } }) }));
vi.mock('../stores/prefs', () => ({ prefs: { getBoolean: () => undefined, set: () => {} } }));
vi.mock('react-native-safe-area-context',() => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 34, left: 0, right: 0 }) }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children, ...props }: React.PropsWithChildren<typeof calls.keyboard>) => {
  calls.keyboard = props;
  return React.createElement('div', null, children);
} }));
vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return {
  ...native,
  View: ({ testID, ...props }: import('react-native').ViewProps) =>
    React.createElement(native.View, { ...props, ...{ 'data-testid': testID } }),
  ScrollView: ({ children, ...props }: React.PropsWithChildren<typeof calls.scroll>) => {
    calls.scroll = props;
    return React.createElement(native.View, { style: props.style as never }, children);
  },
  TextInput: ({ value, onChangeText, accessibilityLabel, placeholder }: {
    value: string; onChangeText: (text: string) => void; accessibilityLabel: string; placeholder?: string;
  }) => React.createElement('input', { value, placeholder, 'aria-label': accessibilityLabel,
    onInput: (event: React.FormEvent<HTMLInputElement>) => onChangeText(event.currentTarget.value) }),
  Pressable: ({ onPress, accessibilityLabel, accessibilityState, disabled, children, style }: {
    onPress: () => void; accessibilityLabel: string; accessibilityState?: { selected?: boolean; expanded?: boolean }; disabled?: boolean; children: React.ReactNode;
    style?: unknown;
  }) => {
    const raw = typeof style === 'function' ? style({ pressed: false }) : style;
    const flattened = Array.isArray(raw) ? Object.assign({}, ...raw.flat(Infinity).filter(Boolean)) : raw;
    return React.createElement('button', { onClick: onPress, 'aria-label': accessibilityLabel, 'aria-selected': accessibilityState?.selected, 'aria-expanded': accessibilityState?.expanded,
      disabled, style: flattened as React.CSSProperties }, children);
  },
  };
});

import { SessionPickerSheet } from './SessionPickerSheet';
import * as m from '../paraglide/messages';

describe('SessionPickerSheet', () => {
  beforeEach(() => {
    calls.loading = false;
    calls.byServer = [];
    calls.keyboard = {};
    calls.scroll = {};
    calls.rows = [
      { name: 'shared', state: 'idle', serverId: 'srv1', serverLabel: 'first-server', serverColor: '#000' },
      { name: 'shared', state: 'idle', serverId: 'srv2', serverLabel: 'second-server', serverColor: '#000' },
    ];
  });

  it.each([false, true])('reúne o topo como a home e prioriza o servidor atual (atual externa=%s)', async (outsideCurrent) => {
    const row = (name: string, serverId: string, serverLabel: string, state: AggSession['state'] = 'idle'): AggSession =>
      ({ name, serverId, serverLabel, state, serverColor: '#000' });
    calls.rows = [
      row('a-idle', 'srv1', 'z-machine'),
      row('b-remote', 'srv2', 'b-machine'),
      row('z-current', 'srv1', 'z-machine', 'working'),
      row('z-wait', 'srv1', 'z-machine', 'awaiting_input'),
      row('c-remote', 'srv3', 'c-machine', 'awaiting_input'),
      row('dead', 'srv1', 'z-machine', 'dead'),
      { ...row('outside', 'external', 'outside-machine'),
        outside: { alias: 'remote', owner: 'owner', localServerId: 'srv1', localSession: 'z-wait' } },
    ];
    const container = document.createElement('div');
    const root = createRoot(container);
    try {
      await act(async () => root.render(React.createElement(SessionPickerSheet, {
        open: true, atual: outsideCurrent ? 'outside' : 'z-current', serverId: outsideCurrent ? 'external' : 'srv1', onClose: () => {},
      })));
      const labels = Array.from(container.querySelectorAll('button[aria-selected]')).map((button) => button.getAttribute('aria-label')?.split(',')[0]);
      expect(labels).toEqual(['c-remote', 'z-wait', 'outside', 'z-current', 'a-idle', 'b-remote']);
      const headings = Array.from(container.querySelectorAll('[role="header"]')).map((header) => header.textContent);
      expect(headings).toEqual([m.home_sessions_awaiting(), m.home_sessions_working(), 'z-machine', 'b-machine']);
      const groups = container.querySelectorAll('button[aria-expanded]');
      expect(groups[0].textContent).toContain('3');
      expect(groups[1].textContent).toContain('1');
      expect(headings.indexOf('z-machine')).toBeLessThan(headings.indexOf('b-machine'));
      expect(headings).not.toContain('c-machine');
      expect(headings).not.toContain('outside-machine');
      await act(async () => (groups[0] as HTMLButtonElement).click());
      expect(groups[0].getAttribute('aria-expanded')).toBe('false');
      expect(groups[0].textContent).toContain('3');
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(3);
      await act(async () => (groups[0] as HTMLButtonElement).click());
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(6);
      expect(container.querySelectorAll('button[aria-selected="true"]')).toHaveLength(1);
    } finally {
      await act(async () => root.unmount());
    }
  });

  it('distingue visualmente externas homônimas junto à mesma âncora', async () => {
    const anchor = { ...calls.rows[0], name: 'anchor' };
    const outside = { ...calls.rows[0], name: 'shared', provider: 'claude' as const, engine: 'test-engine',
      outside: { alias: 'remote', owner: 'owner', localServerId: 'srv1', localSession: 'anchor' } };
    calls.rows = [anchor,
      { ...outside, serverId: 'external-one', serverLabel: 'first-owner' },
      { ...outside, serverId: 'external-two', serverLabel: 'second-owner' }];
    const container = document.createElement('div');
    const root = createRoot(container);
    try {
      await act(async () => root.render(React.createElement(SessionPickerSheet, {
        open: true, atual: 'anchor', serverId: 'srv1', onClose: () => {},
      })));
      expect(Array.from(container.querySelectorAll('[role="header"]')).map((header) => header.textContent)).toEqual(['first-server']);
      const externalRows = Array.from(container.querySelectorAll('button[aria-selected]')).filter((button) => button.getAttribute('aria-label')?.startsWith('shared,'));
      expect(externalRows).toHaveLength(2);
      expect(externalRows[0].textContent).toContain('first-owner');
      expect(externalRows[1].textContent).toContain('second-owner');
      expect(externalRows[0].textContent).not.toBe(externalRows[1].textContent);
    } finally {
      await act(async () => root.unmount());
    }
  });

  it.each(['srv1', 'srv2'])('marca só a sessão de %s e permite navegar à homônima', async (serverId) => {
    calls.replace.mockClear();
    calls.release.mockClear();
    const onClose = vi.fn();
    const container = document.createElement('div');
    const root = createRoot(container);
    try {
      await act(async () => {
        root.render(React.createElement(SessionPickerSheet, { open: true, atual: 'shared', serverId, onClose }));
      });
      const selected = container.querySelectorAll<HTMLButtonElement>('button[aria-selected="true"]');
      const other = container.querySelector<HTMLButtonElement>('button[aria-selected="false"]')!;
      const current = calls.rows.find((row) => row.serverId === serverId)!;
      const destination = calls.rows.find((row) => row.serverId !== serverId)!;
      expect(selected).toHaveLength(1);
      expect(selected[0].getAttribute('aria-label')).toContain(current.serverLabel);
      expect(selected[0].disabled).toBe(true);
      await act(async () => selected[0].click());
      expect(onClose).not.toHaveBeenCalled();
      expect(calls.replace).not.toHaveBeenCalled();

      await act(async () => other.click());

      expect(onClose).toHaveBeenCalledTimes(1);
      expect(calls.replace).toHaveBeenCalledWith(`/s/${destination.serverId}/shared`);
    } finally {
      await act(async () => root.unmount());
    }
    expect(calls.release).toHaveBeenCalledTimes(1);
  });

  it('destaca a atual com check sem marcar a homônima remota', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    try {
      await act(async () => root.render(React.createElement(SessionPickerSheet, {
        open: true, atual: 'shared', serverId: 'srv1', onClose: () => {},
      })));
      const current = container.querySelector<HTMLButtonElement>('button[aria-selected="true"]')!;
      const other = container.querySelector<HTMLButtonElement>('button[aria-selected="false"]')!;
      expect(current.querySelector(':scope > [data-icon="Check"]')).not.toBeNull();
      expect(other.querySelector(':scope > [data-icon="Check"]')).toBeNull();
      expect(current.getAttribute('aria-label')).toContain(m.switcher_atual());
      expect(current.style.backgroundColor).not.toBe('');
      expect(current.style.backgroundColor).not.toBe(other.style.backgroundColor);
    } finally {
      await act(async () => root.unmount());
    }
  });

  it.each([
    ['working', null, '#00f', false],
    ['idle', 'Check', '#ccc', false],
    ['awaiting_input', null, '#f90', false],
    ['idle', 'Hourglass', '#f90', true],
  ] as const)('reusa a linha da home com estado %s (%s), atividade e metadados', async (state, mark, color, limited) => {
    const clock = vi.spyOn(Date, 'now').mockReturnValue(Date.UTC(2026, 9, 18, 12));
    calls.rows = [{ ...calls.rows[0], state, limited, last_activity: Date.now() / 1000 - 125,
      provider: 'claude', engine: 'test-engine', question: state === 'awaiting_input' ? 'pending-question' : undefined,
      label: 'running-task', branch: 'feat/session-picker', git_ahead: 2, git_behind: 3, git_added: 4, git_removed: 5,
      conta: 'claude:/home/test/.claude-review' },
      { ...calls.rows[1], provider: 'codex', engine: 'not-a-claude-engine' }];
    const container = document.createElement('div');
    const root = createRoot(container);
    try {
      await act(async () => root.render(React.createElement(SessionPickerSheet, {
        open: true, atual: 'shared', serverId: 'srv1', onClose: () => {},
      })));
      const current = container.querySelector<HTMLButtonElement>('button[aria-selected="true"]')!;
      const other = container.querySelector<HTMLButtonElement>('button[aria-selected="false"]')!;
      expect(current.textContent).not.toContain(state === 'working' ? m.estado_em_execucao() : state === 'awaiting_input' ? m.estado_aguardando() : m.estado_pronto());
      expect(current.textContent).toContain(m.tempo_min_atras({ n: 2 }));
      expect(current.getAttribute('aria-label')).toContain('test-engine');
      const avatar = current.querySelector('[data-testid="session-avatar"]');
      expect(avatar?.querySelector<HTMLElement>('[data-testid="session-avatar-ring"]')?.style.borderColor).toBe(color);
      const badge = avatar?.querySelector('[data-testid="session-avatar-badge"]');
      if (mark) expect(badge?.querySelector(`[data-icon="${mark}"]`)).not.toBeNull();
      else expect(badge).toBeNull();
      if (state === 'working') expect(current.textContent).toContain('running-task');
      if (state === 'awaiting_input') expect(current.textContent).toContain('pending-question');
      expect(current.textContent).toContain('test-engine');
      expect(current.textContent).toContain('feat/session-picker');
      expect(current.textContent).toContain('+4');
      expect(current.textContent).toContain('−5');
      expect(current.textContent).toContain('review');
      expect(current.querySelector('[data-icon="ArrowUp"]')).not.toBeNull();
      expect(current.querySelector('[data-icon="ArrowDown"]')).not.toBeNull();
      expect(current.querySelector('[data-icon="Settings"]')).not.toBeNull();
      expect(other.textContent).not.toContain('not-a-claude-engine');
      expect(other.querySelector('[data-icon="Settings"]')).toBeNull();
      expect(current.style.minHeight).toBe('44px');
      expect(container.querySelector('[data-menu]')).toBeNull();
    } finally {
      clock.mockRestore();
      await act(async () => root.unmount());
    }
  });

  it('busca no topo só acima de oito vivas, por nome e cwd, sem sumir durante o filtro', async () => {
    calls.rows = [...calls.rows, ...Array.from({ length: 6 }, (_, index) => ({ ...calls.rows[0], name: `item-${index}` })),
      { ...calls.rows[0], name: 'dead', state: 'dead' }];
    const container = document.createElement('div');
    const root = createRoot(container);
    const render = () => act(async () => root.render(React.createElement(SessionPickerSheet, {
      open: true, atual: 'shared', serverId: 'srv1', onClose: () => {},
    })));
    const search = () => container.querySelector<HTMLInputElement>('input')!;
    const type = async (value: string) => act(async () => {
      search().value = value;
      search().dispatchEvent(new Event('input', { bubbles: true }));
    });
    try {
      await render();
      expect(search()).toBeNull();
      calls.rows = [...calls.rows, { ...calls.rows[0], name: 'outside-target', serverId: 'external', cwd: '/WORK/target',
        outside: { alias: 'remote', owner: 'owner', localServerId: 'srv1', localSession: 'shared' } }];
      await render();
      expect(search(), 'nove vivas têm campo de busca').not.toBeNull();
      expect(search().getAttribute('aria-label')).toBe(m.switcher_buscar_sessao());
      expect(search().compareDocumentPosition(container.querySelector('button[aria-selected]')!) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
      await type('  SHARED  ');
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(2);
      expect(search().value).toBe('  SHARED  ');
      await type(' /work/TARGET ');
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(1);
      expect(container.querySelector('button[aria-selected]')?.textContent).toContain('outside-target');
      expect(container.querySelector('[role="header"]')?.textContent).toBe('first-server');
      await type('no-match');
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(0);
      expect(container.textContent).toContain(m.lista_vazia_filtro());
      expect(search().value).toBe('no-match');
      calls.rows = calls.rows.slice(0, 2);
      await render();
      expect(search().value, 'a lista ao vivo encolher não apaga a busca').toBe('no-match');
      await type('');
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(2);
      expect(container.textContent).not.toContain(m.lista_vazia_filtro());
    } finally {
      await act(async () => root.unmount());
    }
  });

  it('mostra carregamento, vazio confirmado e erros reais por servidor sem esconder as sessões acessíveis', async () => {
    const accessible = calls.rows[1];
    calls.rows = [];
    calls.loading = true;
    const server = { id: 'srv1', label: 'first-server', color: '#000', baseUrl: 'http://machine', token: 'test' };
    const container = document.createElement('div');
    const root = createRoot(container);
    const render = () => act(async () => root.render(React.createElement(SessionPickerSheet, {
      open: true, atual: 'shared', serverId: 'srv1', onClose: () => {},
    })));
    try {
      await render();
      expect(container.textContent).toContain(m.lista_carregando());
      expect(container.textContent).not.toContain(m.lista_nenhuma_ativa());
      calls.loading = false;
      calls.byServer = [{ server, sessions: [], loaded: false, error: null }];
      await render();
      expect(container.textContent).toContain(`${server.label}: ${m.lista_carregando()}`);
      expect(container.textContent).not.toContain(m.lista_nenhuma_ativa());
      calls.byServer = [{ server, sessions: [], loaded: true, error: null }];
      await render();
      expect(container.textContent).toContain(m.lista_nenhuma_ativa());
      calls.rows = [accessible];
      calls.byServer = [
        { server, sessions: [], loaded: true, error: 'producer-failed' },
        { server: { ...server, id: 'srv2', label: 'second-server' }, sessions: [accessible], loaded: true, error: null },
        { server: { ...server, id: 'srv3', label: 'third-server' }, sessions: [], loaded: false, error: null },
      ];
      await render();
      expect(container.querySelector('[role="alert"]')?.textContent).toBe('first-server: producer-failed');
      expect(container.textContent).toContain(`third-server: ${m.lista_carregando()}`);
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(1);
      expect(container.textContent).not.toContain(m.lista_nenhuma_ativa());
      calls.rows = [];
      calls.byServer = [{ server, sessions: [], loaded: false, error: 'offline' }];
      await render();
      expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.lista_servidor_offline({ label: 'first-server' }));
      expect(container.textContent).not.toContain(m.lista_nenhuma_ativa());
      expect(container.textContent).not.toContain(m.lista_carregando());
    } finally {
      await act(async () => root.unmount());
    }
  });

  it('convite encerrado é terminal mesmo com loading global, sem bloquear vazio ou filtro no caso misto', async () => {
    const accessible = calls.rows[1];
    const server = { id: 'srv1', label: 'ended-server', baseUrl: 'http://machine', token: 'test', invite: true, inviteEnded: true };
    const ended: ServerBucket = { server, sessions: [], loaded: false, error: null };
    calls.rows = [];
    calls.loading = true;
    calls.byServer = [ended];
    const container = document.createElement('div');
    const root = createRoot(container);
    const render = () => act(async () => root.render(React.createElement(SessionPickerSheet, {
      open: true, atual: 'shared', serverId: 'srv2', onClose: () => {},
    })));
    try {
      await render();
      expect(container.textContent).toContain(`ended-server: ${m.convite_encerrado()}`);
      expect(container.textContent).not.toContain(m.lista_carregando());
      expect(container.textContent).toContain(m.lista_nenhuma_ativa());
      calls.loading = false;
      calls.rows = Array.from({ length: 9 }, (_, index) => ({ ...accessible, name: `item-${index}` }));
      calls.byServer = [ended, {
        server: { ...server, id: 'srv2', label: 'second-server', invite: false, inviteEnded: false },
        sessions: calls.rows, loaded: true, error: null,
      }];
      await render();
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(9);
      expect(container.textContent).toContain(`ended-server: ${m.convite_encerrado()}`);
      expect(container.textContent).not.toContain(m.lista_carregando());
      const search = container.querySelector<HTMLInputElement>('input')!;
      await act(async () => {
        search.value = 'no-match';
        search.dispatchEvent(new Event('input', { bubbles: true }));
      });
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(0);
      expect(container.textContent).toContain(m.lista_vazia_filtro());
      expect(container.textContent).not.toContain(m.lista_nenhuma_ativa());
      expect(container.textContent).not.toContain(m.lista_carregando());
    } finally {
      await act(async () => root.unmount());
    }
  });

  it('a folha só troca de sessão, sem ações administrativas nem retomada', async () => {
    calls.rows = [{ ...calls.rows[0], provider: 'claude', tracked: false, cwd: '/repo' }, calls.rows[1]];
    const container = document.createElement('div');
    const root = createRoot(container);
    try {
      await act(async () => root.render(React.createElement(SessionPickerSheet, {
        open: true, atual: 'shared', serverId: 'srv1', onClose: () => {},
      })));
      expect(container.querySelector('[data-menu]')).toBeNull();
      expect(container.textContent).not.toContain(m.sessao_retomar());
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(2);
    } finally {
      await act(async () => root.unmount());
    }
  });

  it('configura rolagem limitada, margem inferior segura e teclado sem consumir o toque da sessão', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    const flatten = (style: unknown) => Object.assign({}, ...(Array.isArray(style) ? style.flat(Infinity).filter(Boolean) : [style]));
    try {
      await act(async () => root.render(React.createElement(SessionPickerSheet, {
        open: true, atual: 'shared', serverId: 'srv1', onClose: () => {},
      })));
      expect(calls.scroll.keyboardShouldPersistTaps).toBe('handled');
      expect(calls.scroll.keyboardDismissMode).toBe('on-drag');
      expect(flatten(calls.scroll.style).flex).toBe(1);
      expect(flatten(calls.scroll.contentContainerStyle).paddingBottom).toBeGreaterThan(34);
      expect(calls.keyboard.automaticOffset).toBe(true);
      expect(calls.keyboard.behavior).toBe('padding');
      expect(flatten(calls.keyboard.style).flex).toBe(1);
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(2);
    } finally {
      await act(async () => root.unmount());
    }
  });

  it('retém a lista até dismiss e solta a assinatura em cada fechamento ou desmontagem', async () => {
    calls.retain.mockClear();
    calls.release.mockClear();
    const container = document.createElement('div');
    const root = createRoot(container);
    const render = (open: boolean) => act(async () => root.render(React.createElement(SessionPickerSheet, {
      open, atual: 'shared', serverId: 'srv1', onClose: () => {},
    })));
    try {
      await render(false);
      expect(calls.retain).not.toHaveBeenCalled();
      await render(true);
      expect(calls.retain).toHaveBeenCalledTimes(1);
      await render(false);
      expect(calls.release).not.toHaveBeenCalled();
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(2);
      await act(async () => calls.dismiss!());
      expect(calls.release).toHaveBeenCalledTimes(1);
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(0);
      await render(true);
      expect(calls.retain).toHaveBeenCalledTimes(2);
    } finally {
      await act(async () => root.unmount());
    }
    expect(calls.release).toHaveBeenCalledTimes(2);
  });

  it('fechada não assina a lista nem desenha as linhas; ao fechar, mantém as linhas até a folha sumir', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    const render = (open: boolean) => act(async () => {
      root.render(React.createElement(SessionPickerSheet, { open, atual: 'shared', serverId: 'srv1', onClose: () => {} }));
    });
    try {
      calls.reads = 0;
      await render(false);
      expect(calls.reads).toBe(0);
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(0);
      await render(true);
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(2);
      await render(false);
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(2);
      await act(async () => calls.dismiss!());
      expect(container.querySelectorAll('button[aria-selected]')).toHaveLength(0);
    } finally {
      await act(async () => root.unmount());
    }
  });
});
