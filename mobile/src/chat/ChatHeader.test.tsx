// @vitest-environment happy-dom
import { act, createElement, type ComponentProps, type ReactNode } from 'react';
import { rotuloEstado, type AggSession } from '@hangar/core';
import { Platform } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { WorkingMarkPaused } from '../ui/WorkingMark';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import * as m from '../paraglide/messages';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
vi.mock('expo-router', () => ({ useLocalSearchParams: () => ({ server: 'casa' }) }));
vi.mock('../features/voice/LiveVoiceChatButton', () => ({ LiveVoiceChatButton: () => null }));
const store = vi.hoisted(() => ({ rows: [] as AggSession[] }));
vi.mock('../stores/sessions', () => ({ useSessions: (pick: (s: typeof store) => unknown) => pick(store) }));
vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return { ...native, View: ({ testID, accessibilityElementsHidden, importantForAccessibility, ...props }: import('react-native').ViewProps) =>
    createElement(native.View, { ...props, ...{ 'data-testid': testID, 'aria-hidden': accessibilityElementsHidden, 'data-important-for-accessibility': importantForAccessibility } }) };
});
vi.mock('react-native-svg', async (original) => ({
  ...await original<typeof import('react-native-svg')>(),
  default: ({ children, ...props }: { children?: ReactNode }) => createElement('svg', props, children),
  Path: (props: { d: string }) => createElement('path', props),
}));
const animation = vi.hoisted(() => ({ reduced: true, repeat: vi.fn(), cancel: vi.fn() }));
vi.mock('react-native-reanimated', async (original) => {
  const native = await original<typeof import('react-native-reanimated')>();
  const { useState } = await import('react');
  return { ...native, useReducedMotion: () => animation.reduced,
    useSharedValue: (value: number) => useState(() => ({ value }))[0],
    withRepeat: (...args: unknown[]) => { animation.repeat(...args); return 0; },
    cancelAnimation: (value: unknown) => animation.cancel(value) };
});
vi.mock('../stores/servers', () => ({ useServers: (pick: (s: { servers: never[] }) => unknown) => pick({ servers: [] }) }));

const { ChatHeader } = await import('./ChatHeader');

let host: HTMLDivElement;
let root: Root;
beforeEach(() => {
  Platform.OS = 'ios';
  store.rows = [];
  animation.reduced = true;
  vi.clearAllMocks();
  host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
});
afterEach(() => {
  act(() => root.unmount());
  host.remove();
  Platform.OS = 'android';
});

const render = (onHome?: () => void, props: Partial<ComponentProps<typeof ChatHeader>> = {}, paused = false) =>
  act(() => root.render(createElement(WorkingMarkPaused.Provider, { value: paused }, createElement(ChatHeader, {
    name: 'mobile-menu', state: 'idle', onBack: vi.fn(), onMore: vi.fn(), onTitlePress: vi.fn(), onHome, ...props,
  }))));
const session: AggSession = { name: 'mobile-menu', serverId: 'casa', serverLabel: 'casa', serverColor: '#00f', state: 'idle', last_activity: 0 };
const avatar = () => host.querySelector<HTMLElement>('[data-testid="session-avatar"]');
const title = () => host.querySelector<HTMLButtonElement>('button[aria-label^="mobile-menu,"]');
const homeButton = () => host.querySelector(`[aria-label="${m.chat_ir_inicio()}"]`) as HTMLElement | null;

it.each([
  ['phone', false, 90],
  ['phone', true, 130],
  ['tablet', false, 46],
  ['tablet', true, 46],
] as const)('alinha o loop no cabeçalho %s com Início %s sem alterar o recuo do celular', (header, home, padding) => {
  render(home ? vi.fn() : undefined, { header, chipLoop: createElement('i', { 'data-loop': true }) });
  expect(host.querySelector<HTMLElement>('[data-loop]')!.parentElement!.style.paddingLeft).toBe(`${padding}px`);
});

it.each(['/home/u/hangar-voz', undefined])('ícone decorativo identifica o servidor no subtítulo com pasta %s', (cwd) => {
  store.rows = [{ ...session, cwd }];
  render();
  const icon = host.querySelector('[data-symbol="desktopcomputer"]');
  expect(icon).not.toBeNull();
  const mark = icon!.parentElement!;
  expect(mark.getAttribute('aria-hidden')).toBe('true');
  expect(mark.getAttribute('data-important-for-accessibility')).toBe('no');
  expect(mark.nextElementSibling?.textContent).toBe('casa');
  if (cwd) expect(mark.previousElementSibling?.textContent).toBe('hangar-voz @ ');
  expect(title()?.getAttribute('aria-label')?.split('casa')).toHaveLength(2);
});

it('sem servidor conhecido não desenha o computador', () => {
  render();
  expect(host.querySelector('[data-symbol="desktopcomputer"]')).toBeNull();
});

it('mostra a casa ao lado do voltar e leva ao Início', () => {
  const onHome = vi.fn();
  render(onHome);
  act(() => homeButton()!.click());
  expect(onHome).toHaveBeenCalledTimes(1);
});

it('sem destino de Início, não ocupa espaço no cabeçalho', () => {
  render();
  expect(homeButton()).toBeNull();
});

it.each([
  { provider: 'claude', path: 'm4.7144', color: '#d97757' },
  { provider: 'codex', path: 'M22.2819', color: '#000' },
  { provider: 'kimi', path: 'M21.765' },
  { provider: 'pi', path: 'M1.167' },
  { provider: 'omp', path: 'M1.286' },
  { provider: undefined, path: 'M 4.00' },
  { provider: 'orq', path: 'M 4.00' },
] as const)('avatar do provider $provider pertence à sessão da rota', ({ provider, ...mark }) => {
  store.rows = [{ ...session, provider: 'kimi', serverId: 'outro' }, { ...session, provider }];
  render();
  expect(avatar()).not.toBeNull();
  if ('path' in mark) expect(avatar()?.querySelector('path')?.getAttribute('d')).toMatch(new RegExp(`^${mark.path}`));
  else expect(avatar()?.textContent).toBe(mark.letter);
  if ('color' in mark) expect(avatar()?.querySelector('svg')?.getAttribute('fill')).toBe(mark.color);
  expect(avatar()?.style.width).toBe('30px');
  expect(title()?.style.minHeight).toBe('44px');
});

it.each(['gpt-6-1-sol', 'GPT-6', 'openai', 'codex'])('motor Claude %s mostra OpenAI', (engine) => {
  store.rows = [{ ...session, provider: 'claude', engine }];
  render();
  expect(avatar()?.querySelector('path')?.getAttribute('d')).toMatch(/^M22.2819/);
  expect(avatar()?.querySelector('svg')?.getAttribute('fill')).toBe('#000');
});

it.each([
  { state: 'awaiting_input', limited: false, dot: false, badge: false, warning: true },
  { state: 'idle', limited: false, dot: false, badge: true, icon: 'checkmark' },
  { state: 'idle', limited: true, dot: false, badge: true, warning: true, icon: 'hourglass' },
  { state: 'dead', limited: false, dot: false, badge: false },
] as const)('estado ao vivo $state e limite $limited chegam ao avatar e VoiceOver', ({ state, limited, dot, badge, ...expected }) => {
  // A lista atrasada não pode substituir o estado que o chat acabou de receber.
  store.rows = [{ ...session, provider: 'claude', state: 'dead', limited }];
  render(undefined, { state });
  expect(avatar()).not.toBeNull();
  expect(!!avatar()?.querySelector('[data-testid="session-avatar-dot"]')).toBe(dot);
  expect(!!avatar()?.querySelector('[data-testid="session-avatar-badge"]')).toBe(badge);
  const { theme } = useUnistyles();
  const ring = avatar()?.querySelector<HTMLElement>('[data-testid="session-avatar-ring"]');
  const probe = document.createElement('div');
  probe.style.borderColor = 'warning' in expected ? theme.tokens.status.warning : theme.tokens.border.default;
  expect(ring?.style.borderColor).toBe(probe.style.borderColor);
  if ('icon' in expected) expect(avatar()?.querySelector(`[data-symbol="${expected.icon}"]`)).not.toBeNull();
  expect(title()?.getAttribute('aria-label')).toContain(rotuloEstado(state));
  if (limited) expect(title()?.getAttribute('aria-label')).toContain(m.sessao_limite());
});

it('sem estado ao vivo não anuncia pronta nem usa o estado da lista', () => {
  store.rows = [{ ...session, provider: 'claude', state: 'working' }];
  render(undefined, { state: null });
  expect(avatar()).not.toBeNull();
  expect(avatar()?.querySelector('[data-testid="session-avatar-dot"]')).toBeNull();
  expect(avatar()?.querySelector('[data-testid="session-avatar-badge"]')).toBeNull();
  expect(title()?.getAttribute('aria-label')).not.toContain(rotuloEstado('idle'));
});

it('trabalhando mostra a marca do Hangar girando no lugar do avatar; parada volta o agente', () => {
  store.rows = [{ ...session, provider: 'claude' }];
  render(undefined, { state: 'working' });
  expect(host.querySelector('[data-testid="chat-working-mark"]')).not.toBeNull();
  expect(avatar()).toBeNull();
  expect(title()?.getAttribute('aria-label')).toContain(rotuloEstado('working'));
  render(undefined, { state: 'idle' });
  expect(host.querySelector('[data-testid="chat-working-mark"]')).toBeNull();
  expect(avatar()).not.toBeNull();
});

it.each([
  { state: 'working', word: false },
  { state: 'idle', word: false },
  { state: 'awaiting_input', word: true, warning: true },
  { state: 'dead', word: true },
] as const)('estado $state: sem pílula; só aguardando e encerrada viram palavra antes do destino', ({ state, word, ...expected }) => {
  store.rows = [{ ...session, provider: 'claude', cwd: '/home/u/hangar-voz' }];
  render(undefined, { state });
  const marker = host.querySelector<HTMLElement>('[testid="chat-state-word"]');
  expect(!!marker).toBe(word);
  // O texto visível do cabeçalho é nome + linha de baixo; nenhum rótulo solto ao lado.
  const visible = host.textContent ?? '';
  if (word) expect(visible).toContain(`${rotuloEstado(state)} · hangar-voz @ casa`);
  else expect(visible).not.toContain(rotuloEstado(state));
  if ('warning' in expected) {
    const { theme } = useUnistyles();
    const probe = document.createElement('div');
    probe.style.color = theme.tokens.status.warning;
    expect(marker?.style.color).toBe(probe.style.color);
  }
  expect(title()?.getAttribute('aria-label')).toContain(rotuloEstado(state));
});

it('tocar no avatar mantém a ação do título para trocar de sessão', () => {
  const onTitlePress = vi.fn();
  render(undefined, { onTitlePress });
  expect(avatar()).not.toBeNull();
  act(() => avatar()!.click());
  expect(onTitlePress).toHaveBeenCalledTimes(1);
});
