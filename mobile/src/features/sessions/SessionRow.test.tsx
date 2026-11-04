// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { Platform } from 'react-native';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { rotuloEstado, type AggSession } from '@hangar/core';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// O menu expõe os ids das ações: são o único caminho visível pro Git, Renomear e Excluir.
vi.mock('@react-native-menu/menu', () => ({
  MenuView: ({ children, actions, onPressAction }: { children: ReactNode; actions: { id: string; title: string }[];
    onPressAction: (e: { nativeEvent: { event: string } }) => void }) =>
    createElement('div', { 'data-menu': actions.map((a) => a.id).join(' ') }, children,
      ...actions.map((a) => createElement('button', { key: a.id, 'data-action': a.id,
        onClick: () => onPressAction({ nativeEvent: { event: a.id } }) }, a.title))),
}));
// O mock comum passa `style` direto ao DOM, e o da linha é função de `pressed`.
vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return {
    ...native,
    View: ({ testID, accessibilityElementsHidden, importantForAccessibility, ...props }: import('react-native').ViewProps) =>
      createElement(native.View, { ...props, ...{ 'data-testid': testID, 'aria-hidden': accessibilityElementsHidden, 'data-important-for-accessibility': importantForAccessibility } }),
    // O DOM não compõe linhas nativas; preserva o contrato de truncamento recebido pelo Text.
    Text: (props: import('react-native').TextProps) =>
      createElement(native.Text, { ...props, ...{ 'data-number-of-lines': props.numberOfLines } }),
    Pressable: (props: { accessibilityLabel?: string; onPress?: () => void; disabled?: boolean; children?: ReactNode }) =>
      createElement('button', { 'aria-label': props.accessibilityLabel, onClick: props.onPress, disabled: props.disabled }, props.children),
  };
});
vi.mock('expo-haptics', () => ({ impactAsync: () => Promise.resolve(), ImpactFeedbackStyle: { Medium: 'medium' } }));

vi.mock('react-native-svg', async (original) => ({
  ...await original<typeof import('react-native-svg')>(),
  Svg: ({ children, ...props }: { children?: ReactNode }) => createElement('svg', props, children),
  default: ({ children, ...props }: { children?: ReactNode }) => createElement('svg', props, children),
  Path: (props: { d: string }) => createElement('path', props),
}));
vi.mock('../../paraglide/messages', () => ({
  criar_modo_exec_headless: () => 'Sem terminal',
  ask_perguntas: () => 'ask_perguntas',
  conta_padrao: () => 'conta_padrao',
  sessao_conta: (p: { n: string }) => `conta ${p.n}`,
  orq_row_badge: () => 'orq_row_badge',
  sessao_excluir_curto: () => 'sessao_excluir_curto',
  sessao_git: () => 'sessao_git',
  convite_parar: () => 'convite_parar',
  sessao_renomear: () => 'sessao_renomear',
  sessao_retomar: () => 'sessao_retomar',
  sessao_sem_id: () => 'sessao_sem_id',
  sessao_worktree: () => 'sessao_worktree',
  sessao_worktree_abrir: () => 'sessao_worktree_abrir',
  sessao_limite: () => 'sessao_limite',
  git_sync_titulo: (p: { ahead: number; behind: number }) => `sync ${p.ahead}/${p.behind}`,
  sessao_limite_volta: (p: { n: string }) => `sessao_limite_volta ${p.n}`,
  worktree_apagada: () => 'worktree_apagada',
  native_par_convidar: () => 'native_par_convidar',
  native_par_menu: () => 'native_par_menu',
  pair_from_owner: (p: { dono: string }) => `de ${p.dono}`,
  pair_leave_outside: (p: { dono: string }) => `sair ${p.dono}`,
  pair_undo_own: (p: { dono: string }) => `desfazer ${p.dono}`,
  pair_full_pending: () => 'pair_full_pending',
  pair_release: (p: { dono: string }) => `liberar ${p.dono}`,
  pair_level_view: () => 'pair_level_view',
  tags_context_ring: (p: { pct: number }) => `contexto ${p.pct}`,
  ctx_uso_contexto: () => 'uso de contexto',
}));

import { useUnistyles } from 'react-native-unistyles';
import { SessionRow } from './SessionRow';
import { HANGAR_MARK_ARCS } from '../../ui/HangarMark';

// O tema de teste não traz as pílulas de estado, que pintam a marca da linha.
useUnistyles().theme.tokens.pill = {
  working: { bg: '#eef', fg: '#00f' },
  idle: { bg: '#efe', fg: '#0a0' },
  input: { bg: '#fec', fg: '#f90' },
  dead: { bg: '#fee', fg: '#f00' },
};

const base = { serverId: 's1', serverLabel: 'casa', serverColor: '#8b5cf6', tracked: true, state: 'idle' as const, last_activity: 1_700_000_000 };

async function render(session: AggSession, extra: Record<string, unknown> = {}) {
  const container = document.createElement('div');
  const root = createRoot(container);
  const noop = () => {};
  await act(async () =>
    root.render(createElement(SessionRow, { session, mostrarServidor: false, onPress: noop, onGit: noop, onExcluir: noop, onRenomear: noop, onResume: noop, onWorktree: noop, ...extra })),
  );
  return { container, root };
}

describe('SessionRow', () => {
  afterEach(() => { Platform.OS = 'android'; });

  it.each([false, true])('ícone de computador antecede o servidor sem mudar sua cor ou anúncio (compact=%s)', async (compact) => {
    const session: AggSession = { ...base, name: 'api', worktree: true, worktree_path: '/repo/api', branch: 'fix/api' };
    for (const os of ['ios', 'android'] as const) {
      Platform.OS = os;
      const { container, root } = await render(session, { compact, mostrarServidor: true });
      try {
        const server = Array.from(container.querySelectorAll('span')).find(el => el.textContent === base.serverLabel)!;
        const mark = server.previousElementSibling!;
        expect(mark).not.toBeNull();
        expect(mark.querySelector(os === 'ios' ? '[data-symbol="desktopcomputer"]' : 'svg')).not.toBeNull();
        expect(mark.getAttribute('aria-hidden')).toBe('true');
        expect(mark.getAttribute('data-important-for-accessibility')).toBe('no');
        const probe = document.createElement('span');
        probe.style.color = base.serverColor;
        expect(server.style.color).toBe(probe.style.color);
        expect(container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label')?.split(base.serverLabel)).toHaveLength(2);
        if (os === 'android') {
          const monitor = mark.querySelector('svg')!;
          const branch = Array.from(server.parentElement!.querySelectorAll('svg')).at(-1)!;
          expect(monitor.getAttribute('width')).toBe('12');
          expect(monitor.getAttribute('stroke')).toBe(useUnistyles().theme.tokens.text.muted);
          expect(monitor.getAttribute('width')).toBe(branch.getAttribute('width'));
          expect(monitor.getAttribute('stroke')).toBe(branch.getAttribute('stroke'));
        }
      } finally { act(() => root.unmount()); }
      const hidden = await render(session, { compact });
      try {
        expect(hidden.container.textContent).not.toContain(base.serverLabel);
        expect(hidden.container.querySelector('[data-symbol="desktopcomputer"]')).toBeNull();
      } finally { act(() => hidden.root.unmount()); }
    }
  });

  it.each([false, true])('sinal de sessão sem terminal acompanha o nome e o rótulo acessível (compact=%s)', async (compact) => {
    for (const headless of [true, false, undefined]) {
      const { container, root } = await render({ ...base, name: 'api', headless }, { compact });
      try {
        const name = Array.from(container.querySelectorAll('span')).find(el => el.textContent === 'api');
        const signal = container.querySelector('[role="image"][aria-label="Sem terminal"]');
        const label = container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label');
        if (headless) {
          expect(signal).not.toBeNull();
          expect(name?.nextElementSibling).toBe(signal);
          expect(label).toContain('Sem terminal');
        } else {
          expect(signal).toBeNull();
          expect(label).not.toContain('Sem terminal');
        }
      } finally { act(() => root.unmount()); }
    }
  });

  it.each([false, true])('última resposta aparece abaixo do nome em uma linha discreta (compact=%s)', async (compact) => {
    const reply = 'Voz do Hangar: como funciona hoje e o que dá para fazer';
    const { container, root } = await render(
      { ...base, name: 'voz-pesquisa', last_reply: reply, cwd: '/hangar', branch: 'main', conta: 'claude:/home/user/.claude' },
      { compact },
    );
    try {
      const name = Array.from(container.querySelectorAll('span')).find(el => el.textContent === 'voz-pesquisa');
      const subtitle = name?.parentElement?.nextElementSibling as HTMLElement | null;
      expect(subtitle?.textContent).toBe(`◆ ${reply}`);
      expect(subtitle?.getAttribute('data-number-of-lines')).toBe('1');
      expect(subtitle?.style.color).toBe(useUnistyles().theme.tokens.text.muted);
      expect(subtitle?.style.fontStyle).not.toBe('italic');
      expect(container.querySelector('button[aria-label^="voz-pesquisa,"]')?.getAttribute('aria-label')).toContain(reply);
      expect(container.querySelector('[data-testid="session-avatar"]')).not.toBeNull();
      expect(container.textContent).toContain('hangar · main');
      expect(container.querySelector('[aria-label="conta conta_padrao"]')).not.toBeNull();
    } finally { act(() => root.unmount()); }
  });

  it.each([
    { state: 'working', question: undefined, label: '2 agentes · local_bash: conferindo o servidor', expected: '2 agentes · local_bash: conferindo o servidor', italic: true },
    { state: 'idle', question: 'Qual conta usar?', label: undefined, expected: 'Qual conta usar?', italic: false },
    { state: 'awaiting_input', question: 'Pode continuar?', label: undefined, expected: 'Pode continuar?', italic: false },
  ] as const)('subtítulo preserva pergunta e atividade antes da resposta ($state, $expected)', async ({ state, question, label, expected, italic }) => {
    const { container, root } = await render({ ...base, name: 'api', state, question, label, last_reply: 'Resposta anterior' });
    try {
      const subtitle = Array.from(container.querySelectorAll('span')).find(el => el.textContent === expected);
      expect(subtitle).toBeDefined();
      expect(subtitle?.getAttribute('data-number-of-lines')).toBe('1');
      expect(subtitle?.style.color).toBe(question ? useUnistyles().theme.tokens.pill.input.fg : useUnistyles().theme.tokens.text.secondary);
      expect(subtitle?.style.fontStyle === 'italic').toBe(italic);
      expect(container.textContent).not.toContain('Resposta anterior');
      expect(container.textContent).not.toContain('◆');
    } finally { act(() => root.unmount()); }
  });

  it.each(['working', 'awaiting_input', 'dead'] as const)('não mostra resposta antiga no estado %s sem atividade ou pergunta', async (state) => {
    const { container, root } = await render({ ...base, name: 'api', state, last_reply: 'Resposta anterior' });
    try {
      expect(container.textContent).not.toContain('Resposta anterior');
      expect(container.textContent).not.toContain('◆');
    } finally { act(() => root.unmount()); }
  });

  it.each([undefined, null, ''])('sessão parada sem última resposta (%s) não ganha subtítulo vazio', async (last_reply) => {
    const { container, root } = await render({ ...base, name: 'api', last_reply });
    try {
      const name = Array.from(container.querySelectorAll('span')).find(el => el.textContent === 'api');
      expect(name?.parentElement?.nextElementSibling).toBeNull();
      expect(container.textContent).not.toContain('◆');
    } finally { act(() => root.unmount()); }
  });

  it.each([
    { provider: 'claude', label: 'Claude', path: 'm4.7144' },
    { provider: 'codex', label: 'Codex', path: 'M22.2819' },
    { provider: 'kimi', label: 'Kimi', path: 'M21.765' },
    { provider: 'pi', label: 'Pi', letter: 'π' },
    { provider: 'omp', label: 'OMP', letter: 'Ω' },
    { provider: undefined, label: 'Claude', path: 'M 4.00' },
  ] as const)('marca de $label só no avatar, inclusive na linha compacta', async ({ provider, label, ...mark }) => {
    for (const compact of [false, true]) {
      const { container, root } = await render({ ...base, name: 'api', provider, cwd: '/repo', branch: 'main' }, { compact });
      try {
        const name = Array.from(container.querySelectorAll('span')).find(el => el.textContent === 'api');
        const glyph = container.querySelector('[data-testid="session-avatar"]');
        expect(glyph).not.toBeNull();
        expect(name?.previousElementSibling).toBeNull();
        if ('path' in mark) expect(glyph?.querySelector('path')?.getAttribute('d')).toMatch(new RegExp(`^${mark.path}`));
        else expect(glyph?.textContent).toBe(mark.letter);
        expect(container.textContent).not.toContain(label);
        expect(container.textContent).toContain('repo · main');
        expect(container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label')).toContain(label);
      } finally { act(() => root.unmount()); }
    }
  });

  it.each(['gpt', 'local-gpt', 'gpt-6-1-sol', 'GPT-6', 'openai', 'codex'])('motor %s usa OpenAI na cor do texto no avatar e preserva a engrenagem', async (engine) => {
    Platform.OS = 'ios';
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', engine });
    try {
      const name = Array.from(container.querySelectorAll('span')).find(el => el.textContent === 'api');
      expect(name?.previousElementSibling).toBeNull();
      const logo = container.querySelector('[data-testid="session-avatar"] svg');
      expect(logo?.querySelector('path')?.getAttribute('d')).toMatch(/^M22.2819/);
      expect(logo?.getAttribute('fill')).toBe('#000');
      expect(container.querySelector('[data-symbol="gearshape"]')?.parentElement?.textContent).toBe(engine);
      const label = container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label');
      expect(label).toContain('Claude');
      expect(label).toContain(engine);
    } finally { act(() => root.unmount()); }
  });

  it.each(['unknown', 'orq'])('provider %s sem marca usa os arcos do Hangar no avatar', async (provider) => {
    Platform.OS = 'ios';
    const { container, root } = await render({ ...base, name: 'api', provider: provider as AggSession['provider'] });
    try {
      const name = Array.from(container.querySelectorAll('span')).find(el => el.textContent === 'api');
      expect(name?.previousElementSibling).toBeNull();
      expect(container.querySelector('[data-testid="session-avatar"] path')?.getAttribute('d')).toBe(HANGAR_MARK_ARCS[0].d);
      expect(container.textContent).not.toContain('unknown');
      expect(container.textContent).not.toContain('?');
    } finally { act(() => root.unmount()); }
  });

  it('motor Claude mantém a engrenagem nos metadados e o provedor na acessibilidade', async () => {
    Platform.OS = 'ios';
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', engine: 'gpt-6-1-sol', cwd: '/repo', branch: 'main' });
    try {
      expect(container.textContent).not.toContain('Claude');
      const row = container.querySelector('button[aria-label^="api,"]');
      expect(row?.getAttribute('aria-label')).toContain('gpt-6-1-sol');
      expect(row?.getAttribute('aria-label')).toContain('Claude');
      const mark = container.querySelector('[data-symbol="gearshape"]');
      expect(mark).not.toBeNull();
      const engine = Array.from(mark!.parentElement!.querySelectorAll('span')).find(el => el.textContent === 'gpt-6-1-sol');
      expect(engine?.style.color).toBe('#00f');
      expect(engine?.style.fontWeight).toBe('600');
      expect(container.textContent).toContain('repo · main');
    } finally {
      act(() => root.unmount());
    }
  });

  it.each([false, true])('motor não encolhe e metadados longos cedem largura (compact=%s)', async (compact) => {
    Platform.OS = 'ios';
    const { container, root } = await render(
      { ...base, name: 'api', provider: 'claude', engine: 'gpt-6-1-sol',
        cwd: '/repos/projeto-com-pasta-muito-longa', branch: 'fix/branch-com-nome-muito-longo' },
      { compact },
    );
    try {
      const mark = container.querySelector('[data-symbol="gearshape"]');
      const engine = mark?.parentElement;
      const text = Array.from(engine?.querySelectorAll('span') ?? []).find(el => el.textContent === 'gpt-6-1-sol');
      expect(engine?.style.flexShrink).toBe('0');
      expect(text?.style.flexShrink).toBe('0');
      const metadata = engine?.nextElementSibling as HTMLElement | null;
      expect(metadata?.textContent).toBe(' · projeto-com-pasta-muito-longa · fix/branch-com-nome-muito-longo');
      expect(metadata?.style.flexShrink).toBe('1');
      expect(metadata?.getAttribute('data-number-of-lines')).toBe('1');
    } finally {
      act(() => root.unmount());
    }
  });

  it.each([
    { engine: undefined, label: 'Claude' },
    { engine: 'gpt-6-1-sol', label: 'gpt-6-1-sol' },
  ])('servidor vai na placa que fecha a linha e encolhe sem separador (provedor=$label)', async ({ engine, label }) => {
    Platform.OS = 'ios';
    const serverLabel = 'servidor-com-nome-longo-para-disputar-a-largura';
    const { container, root } = await render(
      { ...base, name: 'api', provider: 'claude', engine, serverLabel, cwd: '/repo' },
      { mostrarServidor: true },
    );
    try {
      const server = Array.from(container.querySelectorAll('span')).find(el => el.textContent?.startsWith(serverLabel));
      expect(server?.textContent).toBe(serverLabel);
      expect(server?.getAttribute('data-number-of-lines')).toBe('1');
      expect(server?.style.flexShrink).toBe('1');
      const plate = server?.parentElement?.parentElement as HTMLElement | null;
      const row = plate?.parentElement as HTMLElement | null;
      expect(row?.lastElementChild).toBe(plate);
      expect([...(row?.querySelectorAll('span') ?? [])].some(el => el.textContent === ' · ')).toBe(false);
      expect(row?.textContent?.slice(0, row.textContent.indexOf(serverLabel))).toContain(engine || 'repo');
      if (engine) {
        const mark = row?.querySelector('[data-symbol="gearshape"]');
        expect(mark).not.toBeNull();
        expect(mark?.parentElement?.textContent).toBe('gpt-6-1-sol');
      }
      expect(container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label')).toContain(label);
    } finally {
      act(() => root.unmount());
    }
  });

  it.each([
    { provider: 'claude', engine: undefined, label: 'Claude' },
    { provider: 'codex', engine: 'gpt-6-1-sol', label: 'Codex' },
    { provider: 'orq', engine: undefined, label: null },
  ] as const)('marca de motor preserva provedor $provider sem motor Claude', async ({ provider, engine, label }) => {
    Platform.OS = 'ios';
    const { container, root } = await render({ ...base, name: 'api', provider, engine });
    try {
      expect(container.querySelector('[data-symbol="gearshape"]')).toBeNull();
      expect(container.textContent).not.toContain('gpt-6-1-sol');
      const accessible = container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label');
      if (label) {
        expect(container.textContent).not.toContain(label);
        expect(accessible).toContain(label);
      } else {
        expect(accessible).not.toContain('Orq');
      }
    } finally {
      act(() => root.unmount());
    }
  });

  it.each([
    { conta: 'claude:/home/user/.claude-claude-200-3', label: '200-3', name: 'claude-200-3', color: '#97c69d' },
    { conta: 'claude:C:\\Users\\user\\.claude-claude-200-3\\', label: '200-3', name: 'claude-200-3', color: '#97c69d' },
    { conta: 'claude:/home/user/.claude/', label: 'conta_padrao', name: 'conta_padrao', color: '#9ba9d9' },
    { conta: 'codex:C:\\Users\\user\\.codex\\', label: 'conta_padrao', name: 'conta_padrao', color: '#d3a781' },
    { conta: 'codex:C:\\Users\\user\\.codex-claude-x\\', label: 'claude-x', name: 'claude-x', color: '#ccbf87' },
    { conta: 'codex:/home/user/.codex-codex-x', label: 'x', name: 'codex-x', color: '#80bbc3' },
  ])('conta $conta fecha a linha de metadados com nome curto e ponto da cor PWA', async ({ conta, label, name, color }) => {
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', conta, cwd: '/repo', branch: 'main' });
    try {
      const chip = container.querySelector<HTMLElement>(`[aria-label="conta ${name}"]`);
      expect(chip).not.toBeNull();
      expect(chip!.textContent).toBe(label);
      expect(chip!.querySelector<HTMLElement>('span')?.style.color).toBe('#666');
      expect(chip!.querySelector<HTMLElement>('div')?.style.backgroundColor).toBe(color);
      expect(chip!.style.maxWidth).toBe('96px');
      expect(chip!.style.flexShrink, 'os metadados cedem antes da conta').toBe('0');
      expect(chip!.nextElementSibling, 'a conta fecha a linha').toBeNull();
      // A conta mora na placa, último pedaço da linha de metadados.
      const line = chip!.parentElement!.parentElement!;
      expect(line.lastElementChild).toBe(chip!.parentElement);
      expect(line.textContent).toContain('repo · main');
      expect(line.textContent).not.toContain('api');
      expect(container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label')).toContain(`conta ${name}`);
    } finally {
      act(() => root.unmount());
    }
  });

  it('etiqueta da conta e da máquina vindas do servidor vencem o cálculo local', async () => {
    Platform.OS = 'android';
    const { container, root } = await render({
      ...base, name: 'api', provider: 'claude', conta: 'claude:/home/user/.claude-claude-200-3', cwd: '/repo', branch: 'main',
      accountTag: { name: 'ana', icon: 'key-round', color: 'blue' },
      machineTag: { name: 'Casa', icon: 'house', color: null },
    }, { mostrarServidor: true });
    try {
      const chip = container.querySelector<HTMLElement>('[aria-label="conta ana"]');
      expect(chip!.textContent).toBe('ana');
      expect(chip!.querySelector('svg')?.getAttribute('stroke')).toBe('#3b82f6');
      const plate = chip!.parentElement!;
      expect(plate.textContent).toBe(`${base.serverLabel}ana`);
      expect(container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label')).toContain('conta ana');
    } finally {
      act(() => root.unmount());
    }
  });

  it('anel de contexto entra no rótulo da linha só com leitura', async () => {
    const lida = await render({ ...base, name: 'api', provider: 'claude', context: { used: 150_000, window: 200_000 } });
    const sem = await render({ ...base, name: 'web', provider: 'claude' });
    try {
      expect(lida.container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label')).toContain('contexto 75');
      expect(sem.container.querySelector('button[aria-label^="web,"]')?.getAttribute('aria-label')).not.toContain('contexto');
    } finally {
      act(() => { lida.root.unmount(); sem.root.unmount(); });
    }
  });

  it.each([undefined, null, 'chave:gpt-6-1-sol', 'kimi:api'])('chip de conta ignora identidade %s', async (conta) => {
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', engine: 'gpt-6-1-sol', conta });
    try {
      expect(container.querySelector('[aria-label^="conta "]')).toBeNull();
      expect(container.textContent).not.toContain('conta_padrao');
    } finally {
      act(() => root.unmount());
    }
  });

  it('selo do grupo entra no rótulo da linha: escondido do leitor, ele só existia na tela', async () => {
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', pair_peers: ['web'], pair_task: 'migracao-login' });
    try {
      expect(container.querySelector('button')?.getAttribute('aria-label')).toContain('migracao-login');
    } finally {
      act(() => root.unmount());
    }
  });

  it('orquestrador sem pasta: selo próprio e sem menu de toque longo', async () => {
    const { container, root } = await render({ ...base, name: 'g1-orq', provider: 'orq', pair_gid: 'g1', orq_arbiter: 'arb' });
    expect(container.textContent).toContain('orq_row_badge');
    expect(container.querySelector('[data-menu]')).toBeNull();
    act(() => root.unmount());
  });

  it('orquestrador com cwd: menu só com Git', async () => {
    const { container, root } = await render({ ...base, name: 'g1-orq', provider: 'orq', pair_gid: 'g1', orq_arbiter: 'arb', cwd: '/repo' });
    expect(container.querySelector('[data-menu]')?.getAttribute('data-menu')).toBe('git');
    act(() => root.unmount());
  });

  it('sessão comum: renomear e excluir no toque longo, sem botões de arrasto', async () => {
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude' });
    expect(container.textContent).not.toContain('orq_row_badge');
    expect(container.querySelector('[data-menu]')?.getAttribute('data-menu')).toBe('rename delete');
    expect(container.querySelector('[aria-label="sessao_excluir_curto"]')).toBeNull();
    act(() => root.unmount());
  });

  it.each([
    ['codex', false],
    ['kimi', false],
    ['pi', true],
    ['claude', true],
  ])('sem vínculo: %s abre a conversa? bloqueada=%s', async (provider, bloqueada) => {
    const { container, root } = await render({ ...base, name: 'nova', provider: provider as AggSession['provider'], tracked: false });
    const linha = container.querySelector<HTMLButtonElement>('button[aria-label^="nova,"]');
    expect(linha).not.toBeNull();
    expect(linha!.disabled).toBe(bloqueada);
    act(() => root.unmount());
  });

  it('worktree vira ação do menu, e a branch ganha o ícone de ramo', async () => {
    Platform.OS = 'ios';
    const onWorktree = vi.fn();
    const { container, root } = await render(
      { ...base, name: 'api', provider: 'claude', worktree: true, worktree_path: '/r/hangar-x', cwd: '/r/hangar', branch: 'feat/x' }, { onWorktree });
    try {
      expect(container.textContent).not.toContain('hangar-x');
      const icon = container.querySelector('[data-symbol="arrow.triangle.branch"]');
      expect(icon?.nextElementSibling?.textContent).toBe('feat/x');
    } finally { act(() => root.unmount()); }
    Platform.OS = 'android';
    const again = await render(
      { ...base, name: 'api', provider: 'claude', worktree: true, worktree_path: '/r/hangar-x', cwd: '/r/hangar', branch: 'feat/x' }, { onWorktree });
    expect(again.container.querySelector('[data-menu]')?.getAttribute('data-menu')).toBe('rename git worktree delete');
    await act(async () => { again.container.querySelector<HTMLButtonElement>('[data-action="worktree"]')!.click(); });
    expect(onWorktree).toHaveBeenCalledWith(expect.objectContaining({ name: 'api' }), '/r/hangar-x');
    act(() => again.root.unmount());
  });

  it('worktree sem branch (HEAD destacado) mostra o nome da worktree no lugar da branch', async () => {
    Platform.OS = 'ios';
    const { container, root } = await render(
      { ...base, name: 'api', provider: 'claude', worktree: true, worktree_path: '/r/hangar-x', cwd: '/r/hangar', branch: null }, { onWorktree: undefined });
    try {
      const icon = container.querySelector('[data-symbol="arrow.triangle.branch"]');
      expect(icon?.nextElementSibling?.textContent).toBe('hangar-x');
    } finally { act(() => root.unmount()); }
  });

  it('git como no PWA: commits a enviar e a trazer e linhas alteradas na linha de metadados', async () => {
    Platform.OS = 'ios';
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', cwd: '/repo', branch: 'main',
      git_ahead: 2, git_behind: 1, git_added: 10, git_removed: 3 });
    try {
      const after = (symbol: string) => {
        const icon = container.querySelector(`[data-symbol="${symbol}"]`);
        return icon?.nextElementSibling?.textContent;
      };
      expect(after('arrow.up'), 'a enviar').toBe('2');
      expect(after('arrow.down'), 'a trazer').toBe('1');
      expect(container.textContent).toContain('+10');
      expect(container.textContent).toContain('−3');
      expect(container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label')).toContain('sync 2/1');
    } finally { act(() => root.unmount()); }
  });

  it('git em dia ou sem upstream não desenha nada', async () => {
    Platform.OS = 'ios';
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', cwd: '/repo', branch: 'main',
      git_ahead: 0, git_behind: null, git_added: 0, git_removed: null });
    try {
      expect(container.querySelector('[data-symbol="arrow.up"]')).toBeNull();
      expect(container.querySelector('[data-symbol="arrow.down"]')).toBeNull();
      expect(container.textContent).not.toMatch(/[+−]\d/);
      expect(container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label')).not.toContain('sync');
    } finally { act(() => root.unmount()); }
  });

  it('servidor de convite: sem ação de worktree', async () => {
    const { container, root } = await render(
      { ...base, name: 'api', provider: 'claude', worktree: true, worktree_path: '/r/hangar-x', cwd: '/r/hangar', serverInvite: true }, { onWorktree: undefined });
    expect(container.querySelector('[data-action="worktree"]')).toBeNull();
    act(() => root.unmount());
  });

  it.each([
    { state: 'working', limited: false, ring: '#00f', badge: null, dot: true },
    { state: 'awaiting_input', limited: false, ring: '#f90', badge: null, dot: false },
    { state: 'idle', limited: false, ring: '#ccc', badge: 'checkmark', dot: false },
    { state: 'idle', limited: true, ring: '#f90', badge: 'hourglass', dot: false },
    { state: 'working', limited: true, ring: '#00f', badge: null, dot: true },
    { state: 'awaiting_input', limited: true, ring: '#f90', badge: null, dot: false },
    { state: 'dead', limited: true, ring: '#ccc', badge: null, dot: false },
  ] as const)('avatar para $state (limite=$limited) conserva estado e limite no VoiceOver', async ({ state, limited, ring, badge, dot }) => {
    Platform.OS = 'ios';
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', state, limited, limit_reset: limited ? '14:00' : undefined });
    try {
      const avatar = container.querySelector<HTMLElement>('[data-testid="session-avatar"]');
      expect(avatar).not.toBeNull();
      expect(avatar?.querySelector<HTMLElement>('[data-testid="session-avatar-ring"]')?.style.borderColor).toBe(ring);
      expect(!!avatar?.querySelector('[data-testid="session-avatar-dot"]')).toBe(dot);
      const seal = avatar?.querySelector<HTMLElement>('[data-testid="session-avatar-badge"]');
      if (badge) {
        expect(seal?.querySelector(`[data-symbol="${badge}"]`)).not.toBeNull();
        expect(seal?.style.backgroundColor).toBe(limited ? '#f90' : '#0a0');
      } else expect(seal).toBeNull();
      expect(avatar?.getAttribute('aria-hidden')).toBe('true');
      const lines = [...container.querySelectorAll('span')].map((el) => el.textContent);
      expect(lines).not.toContain(rotuloEstado(state));
      const label = container.querySelector('button[aria-label^="api,"]')?.getAttribute('aria-label');
      expect(label).toContain(rotuloEstado(state));
      if (limited) expect(label).toContain('sessao_limite_volta 14:00');
    } finally { act(() => root.unmount()); }
  });

  it('separa servidor e branch de worktree mesmo quando a pasta repete o nome', async () => {
    Platform.OS = 'ios';
    const { container, root } = await render({ ...base, name: 'hangar-x', provider: 'codex', serverLabel: 'notebook-jefferson',
      cwd: '/r/hangar-x', worktree: true, worktree_path: '/r/hangar-x', branch: 'feat/mobile-avatar-estado' }, { mostrarServidor: true });
    try {
      const server = [...container.querySelectorAll('span')].find(el => el.textContent === 'notebook-jefferson');
      const plate = server?.parentElement?.parentElement as HTMLElement | null;
      const branch = plate?.parentElement?.querySelector('[data-symbol="arrow.triangle.branch"]');
      expect(branch).not.toBeNull();
      expect(plate?.contains(branch ?? null)).toBe(false);
    } finally { act(() => root.unmount()); }
  });

  it('convite oferece parar de acompanhar, nunca excluir a sessão', async () => {
    const onExcluir = vi.fn();
    const session = { ...base, name: 'convidada', provider: 'claude' as const, serverInvite: true };
    const { container, root } = await render(session, { onExcluir });
    expect(container.querySelector('[data-action="delete"]')).toBeNull();
    const stop = container.querySelector<HTMLButtonElement>('[data-action="stop-following"]');
    expect(stop?.textContent).toBe('convite_parar');
    await act(async () => stop!.click());
    expect(onExcluir).toHaveBeenCalledExactlyOnceWith(session);
    act(() => root.unmount());
  });

  it('sessão de fora: marca do dono e só "Sair do pareamento" no menu', async () => {
    const onPair = vi.fn();
    const { container, root } = await render({ ...base, name: 'web', provider: 'claude', cwd: '/r/web', guest_kind: 'pair',
      outside: { alias: 'casa', owner: 'Ana', localServerId: 's1', localSession: 'api' } }, { onPair });
    expect(container.textContent).toContain('de Ana');
    expect(container.querySelector('[data-menu]')?.getAttribute('data-menu')).toBe('leave');
    act(() => root.unmount());
  });

  it('sessão própria com par: convidar, aceitar e desfazer um por dono', async () => {
    const pair = (owner: string, session: string) => ({ alias: owner.toLowerCase(), owner, session, address: 'https://x.ts.net:8443', level: 'view' as const, pending: false });
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude',
      pair_externals: [pair('Ana', 'web'), pair('Bia', 'docs')] }, { onPair: vi.fn() });
    expect(container.querySelector('[data-menu]')?.getAttribute('data-menu')).toBe('rename pair-invite pair-accept undo:0 undo:1 delete');
    act(() => root.unmount());
  });

  it('acesso completo pendente mostra o selo e o botão Liberar', async () => {
    const onPair = vi.fn();
    const pair = { alias: 'casa', owner: 'Ana', session: 'web', address: 'https://x.ts.net:8443', level: 'full' as const, pending: true };
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', pair_externals: [pair] }, { onPair });
    expect(container.textContent).toContain('pair_full_pending');
    await act(async () => { (container.querySelector('[aria-label="liberar Ana"]') as HTMLElement).click(); });
    expect(onPair).toHaveBeenCalledWith(expect.objectContaining({ name: 'api' }), { kind: 'release', pair });
    act(() => root.unmount());
  });

  it('worktree apagada aparece como texto', async () => {
    const { container, root } = await render({ ...base, name: 'api', provider: 'claude', worktree_gone: true, worktree_path: '/r/hangar-x' });
    expect(container.textContent).toContain('worktree_apagada');
    expect(container.querySelector('[aria-label^="sessao_worktree"]')).toBeNull();
    act(() => root.unmount());
  });
});
