// @vitest-environment happy-dom
import { act, createElement, forwardRef, useImperativeHandle, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { parseLiveVoiceMessage, type Credencial, type LiveVoiceState, type LiveVoiceSettingsReply } from '@hangar/core';
import { LiveVoiceSheet, type LiveVoiceSheetProps, type LiveVoiceView } from './LiveVoiceSheet';
import { LiveVoiceButton } from './LiveVoiceButton';
import * as m from '../../paraglide/messages';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const native = vi.hoisted(() => ({ markdown: vi.fn(), scroll: vi.fn(), announce: vi.fn(), os: 'android' }));
vi.mock('react-native', async original => ({
  ...await original<typeof import('react-native')>(),
  Platform: { get OS() { return native.os; }, select: (x: Record<string, unknown>) => x[native.os] ?? x.default },
  AccessibilityInfo: { announceForAccessibility: native.announce, isReduceTransparencyEnabled: () => Promise.resolve(false),
    addEventListener: () => ({ remove() {} }) },
  ScrollView: forwardRef(function VoiceScroll({ children }: { children?: ReactNode }, ref) {
    useImperativeHandle(ref, () => ({ scrollTo: native.scroll }));
    return createElement('div', { 'data-scroll-content': true }, children);
  }),
  Pressable: ({ children, onPress, accessibilityRole, accessibilityLabel, accessibilityState, disabled }: {
    children?: ReactNode; onPress?: () => void; accessibilityRole?: string; accessibilityLabel?: string; disabled?: boolean;
    accessibilityState?: { selected?: boolean; expanded?: boolean; busy?: boolean; disabled?: boolean };
  }) => createElement('button', { onClick: onPress, role: accessibilityRole, 'aria-label': accessibilityLabel,
    'aria-selected': accessibilityState?.selected, 'aria-expanded': accessibilityState?.expanded,
    'aria-busy': accessibilityState?.busy, disabled }, children),
}));
vi.mock('../../ui/Sheet', () => ({
  Sheet: ({ open, children, onDismiss }: { open?: boolean; children: ReactNode; onDismiss: () => void }) => open
    ? createElement('section', null, children, createElement('button', { onClick: onDismiss, 'aria-label': 'native-dismiss' }, 'native-dismiss')) : null,
}));
vi.mock('../../ui/AnchoredPanel', () => ({
  AnchoredPanel: ({ open, children }: { open: boolean; children: ReactNode }) => open
    ? createElement('div', { 'data-popup-layer': true }, children) : null,
}));
// Como o real: o `useUnistyles` devolve um Proxy novo do tema a cada render.
vi.mock('react-native-unistyles', async original => {
  const actual = await original<typeof import('react-native-unistyles')>();
  return { ...actual, useUnistyles: () => { const ui = actual.useUnistyles(); return { ...ui, theme: new Proxy(ui.theme, {}) }; } };
});
// Fronteira do renderizador nativo; não monta a árvore de bolhas do chat.
vi.mock('../../chat/AssistantBubble', () => ({ mkMarkdownStyle: () => ({ paragraph: {} }) }));
vi.mock('react-native-enriched-markdown', () => ({
  EnrichedMarkdownText: (props: { markdown: string; flavor: string }) => {
    native.markdown(props);
    return createElement('article', { 'data-renderer': 'native-markdown' });
  },
}));

const settingsReply = (): LiveVoiceSettingsReply => ({
  enabled: true, codex: true, jev: true, voices: ['voice-a', 'voice-b'], call: { active: false, client: null },
  settings: { codex_account: 'account-a', voice: 'voice-a', style: 'conversational', organizer: {
    direct: { model: 'model-a', effort: 'medium', tier: null },
    plan: { model: 'model-b', effort: 'high', tier: 'priority' },
  } },
});
const account = (id: string): Credencial => ({ id, tipo: 'codex', codex_account: id, nome: id, nome_natural: id, ativa: true, usos: [] });
const liveState = (): LiveVoiceState => ({
  phase: 'live', mode: 'direct', activity: 'idle', draft: null, error: null, plan: null,
  effective: { model: 'effective-model', effort: 'effective-effort', tier: 'default' }, context: null,
  limits: { five_hour: { used_percent: 25, resets_at: null }, seven_day: null }, backstage: [], thought: '',
  action: null, followed: [], server: 'voice-server', client: 'mobile',
});
const initialView = (): LiveVoiceView => ({
  phase: 'idle', serverLabel: 'server-fixture', state: null, error: null, muted: false, levels: { input: 0, output: 0 }, liveSince: null,
  settings: { loading: false, reply: settingsReply(), error: null, saving: false, saveError: null,
    accounts: [account('account-a'), account('account-b')],
    models: [{ id: 'model-a', efforts: ['low', 'medium'], default_effort: 'medium' }, { id: 'model-b', efforts: ['medium', 'high'] }], modelsError: null },
  memory: { loading: false, entries: [], error: null, forgetError: null },
});

const staticLevels = { get: () => view.levels, subscribe: () => () => {} };
let container: HTMLDivElement;
let root: Root;
let view: LiveVoiceView;
let callbacks: Omit<LiveVoiceSheetProps, 'open' | 'view' | 'levels' | 'servers' | 'serverId'>;
const servers = [{ id: 'server-a', label: 'server-fixture' }, { id: 'server-b', label: 'other-server' }];
let serverId: string;
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-10-15T12:00:00Z'));
  native.markdown.mockClear();
  native.scroll.mockClear();
  native.announce.mockClear();
  native.os = 'android';
  container = document.createElement('div');
  root = createRoot(container);
  view = initialView();
  serverId = 'server-a';
  callbacks = { onSelectServer: vi.fn(), onDismiss: vi.fn(), onStart: vi.fn(), onStop: vi.fn(), onToggleMute: vi.fn(), onMode: vi.fn(), onRetry: vi.fn(),
    onSaveSettings: vi.fn(), onSettingsOpen: vi.fn(), onForgetMemory: vi.fn() };
});
afterEach(async () => { await act(async () => root.unmount()); vi.useRealTimers(); });
const render = (open = true) => act(async () => root.render(createElement(LiveVoiceSheet, { levels: staticLevels, open, view, servers, serverId, ...callbacks })));
const button = (label: string) => [...container.querySelectorAll('button')].find(el => el.getAttribute('aria-label') === label)!;
const click = (label: string) => act(async () => button(label).click());
const choose = async (title: string, option: string) => {
  const triggers = [...container.querySelectorAll<HTMLButtonElement>('button[aria-label]')]
    .filter(el => el.getAttribute('aria-label')!.startsWith(`${title}: `));
  expect(triggers).toHaveLength(1);
  await act(async () => triggers[0].click());
  await act(async () => [...container.querySelectorAll<HTMLButtonElement>('button[role="radio"]')]
    .find(el => el.getAttribute('aria-label') === option)!.click());
};

describe('LiveVoiceSheet', () => {
  it('oferece cinco destinos no fluxo da folha sem popup e fecha ao escolher primeiro ou último', async () => {
    const destinations = [
      { id: 'server-a', label: 'server-fixture' },
      { id: 'server-b', label: 'orion-fixture' },
      { id: 'server-c', label: 'lyra-fixture' },
      { id: 'server-d', label: 'vega-fixture' },
      { id: 'server-e', label: 'draco-fixture' },
    ];
    await act(async () => root.render(createElement(LiveVoiceSheet, { levels: staticLevels, open: true, view, servers: destinations, serverId, ...callbacks })));
    expect(container.querySelector('[role="radiogroup"]')).toBeNull();
    for (const [label, id] of [['server-fixture', 'server-a'], ['draco-fixture', 'server-e']]) {
      await click(`${m.live_voice_server()}: server-fixture`);
      const options = [...container.querySelectorAll<HTMLButtonElement>('button[role="radio"]')];
      expect(options.map(option => option.getAttribute('aria-label'))).toEqual([
        'server-fixture', 'orion-fixture', 'lyra-fixture', 'vega-fixture', 'draco-fixture',
      ]);
      for (const option of options) expect(option.closest('[data-popup-layer]')).toBeNull();
      const group = container.querySelector('[role="radiogroup"]')!;
      expect(group.getAttribute('aria-label')).toBe(m.live_voice_server());
      expect(container.querySelector('section [data-scroll-content]')?.firstElementChild).toBe(group);
      expect(container.querySelectorAll('[data-scroll-content]')).toHaveLength(1);
      expect(options.map(option => option.getAttribute('aria-selected'))).toEqual(['true', 'false', 'false', 'false', 'false']);
      await click(label);
      expect(callbacks.onSelectServer).toHaveBeenLastCalledWith(id);
      expect(container.querySelector('[role="radiogroup"]')).toBeNull();
    }
    expect(callbacks.onSelectServer).toHaveBeenCalledTimes(2);
    expect(callbacks.onStart).not.toHaveBeenCalled();
  });

  it.each(['idle', 'closed'] as const)('seletor de servidor permite escolher próxima chamada em %s mesmo após erro', async phase => {
    view.phase = phase;
    if (phase === 'closed') view.error = { code: 'connection_lost' };
    await render();
    expect(button(`${m.live_voice_server()}: server-fixture`)).toBeDefined();
    await choose(m.live_voice_server(), 'other-server');
    expect(callbacks.onSelectServer).toHaveBeenCalledExactlyOnceWith('server-b');
    expect(callbacks.onStart).not.toHaveBeenCalled();
  });

  it('servidor removido após erro permite selecionar B sem oferecer A', async () => {
    view.phase = 'closed';
    view.error = { code: 'server_removed', detail: m.chat_servidor_removido() };
    await act(async () => root.render(createElement(LiveVoiceSheet, { levels: staticLevels, open: true, view, servers: [servers[1]], serverId, ...callbacks })));
    const trigger = button(`${m.live_voice_server()}: server-fixture`);
    expect(trigger).toBeDefined();
    expect(container.textContent).toContain(m.chat_servidor_removido());
    expect(callbacks.onSelectServer).not.toHaveBeenCalled();
    await act(async () => trigger.click());
    const options = [...container.querySelectorAll<HTMLButtonElement>('button[role="radio"]')];
    expect(options.map(option => option.getAttribute('aria-label'))).toEqual(['other-server']);
    expect(container.textContent).toContain(m.chat_servidor_removido());
    expect(callbacks.onSelectServer).not.toHaveBeenCalled();
    await act(async () => options[0].click());
    expect(callbacks.onSelectServer).toHaveBeenCalledExactlyOnceWith('server-b');
    expect(callbacks.onStart).not.toHaveBeenCalled();
  });

  it.each(['connecting', 'live'] as const)('fecha a lista já aberta ao entrar em %s e impede seleção', async phase => {
    await render();
    await click(`${m.live_voice_server()}: server-fixture`);
    expect(button('other-server')).toBeDefined();
    view.phase = phase;
    await render();
    expect(container.querySelector('[role="radiogroup"]')).toBeNull();
    expect(button(`${m.live_voice_server()}: server-fixture`).disabled).toBe(true);
    await click(`${m.live_voice_server()}: server-fixture`);
    expect(container.querySelectorAll('button[role="radio"]')).toHaveLength(0);
    expect(callbacks.onSelectServer).not.toHaveBeenCalled();
    view.phase = 'closed';
    await render();
    expect(container.querySelector('[role="radiogroup"]')).toBeNull();
  });

  it.each(['connecting', 'live'] as const)('seletor de servidor fica desabilitado em %s', async phase => {
    view.phase = phase;
    await render();
    const trigger = button(`${m.live_voice_server()}: server-fixture`);
    expect(trigger).toBeDefined();
    expect(trigger.disabled).toBe(true);
    await act(async () => trigger.click());
    expect(container.querySelectorAll('button[role="radio"]')).toHaveLength(0);
    expect(callbacks.onSelectServer).not.toHaveBeenCalled();
  });

  it('seletor de servidor pede rolagem ao topo somente quando abre a lista', async () => {
    await render();
    expect(native.scroll).not.toHaveBeenCalled();
    await click(`${m.live_voice_server()}: server-fixture`);
    expect(native.scroll).toHaveBeenCalledExactlyOnceWith({ y: 0, animated: false });
    await click(`${m.live_voice_server()}: server-fixture`);
    expect(native.scroll).toHaveBeenCalledTimes(1);
    await click(`${m.live_voice_server()}: server-fixture`);
    expect(native.scroll).toHaveBeenCalledTimes(2);
    expect(native.scroll).toHaveBeenLastCalledWith({ y: 0, animated: false });
  });

  it('seletor de servidor alterna a lista e a fecha ao reabrir a folha ou mudar de servidor', async () => {
    await render();
    await click(`${m.live_voice_server()}: server-fixture`);
    expect(button('other-server')).toBeDefined();
    await click(`${m.live_voice_server()}: server-fixture`);
    expect(container.querySelector('[role="radiogroup"]')).toBeNull();
    await click(`${m.live_voice_server()}: server-fixture`);
    await render(false);
    await render();
    expect(container.querySelector('[role="radiogroup"]')).toBeNull();
    await click(`${m.live_voice_server()}: server-fixture`);
    expect(button('other-server')).toBeDefined();
    serverId = 'server-b';
    view = { ...initialView(), serverLabel: 'other-server' };
    await render();
    expect(container.querySelector('[role="radiogroup"]')).toBeNull();
    expect(callbacks.onSelectServer).not.toHaveBeenCalled();
    await click(`${m.live_voice_server()}: other-server`);
    expect(button('other-server').getAttribute('aria-selected')).toBe('true');
    expect(button('server-fixture').getAttribute('aria-selected')).toBe('false');
  });

  it('seletor de servidor fecha ajustes da máquina anterior quando a escolha muda', async () => {
    await render();
    await click(m.native_live_voice_settings_memory());
    expect(button(m.native_live_voice_settings_memory()).getAttribute('aria-expanded')).toBe('true');
    serverId = 'server-b';
    view = { ...initialView(), serverLabel: 'other-server' };
    await render();
    expect(button(m.native_live_voice_settings_memory()).getAttribute('aria-expanded')).toBe('false');
    expect(button(`${m.live_voice_server()}: other-server`)).toBeDefined();
  });

  it('conserva o rótulo salvo no app antes e durante a chamada mesmo com outro nome no estado do servidor', async () => {
    view.serverLabel = 'app-server-label';
    await render();
    expect(button(`${m.live_voice_server()}: app-server-label`)).toBeDefined();
    view.state = { ...liveState(), server: 'runtime-hostname' };
    for (const phase of ['connecting', 'live'] as const) {
      view.phase = phase;
      await render();
      expect(button(`${m.live_voice_server()}: app-server-label`)).toBeDefined();
      expect(container.textContent).not.toContain('runtime-hostname');
    }
  });

  it('mostra carregamento, erro selecionável e nova tentativa antes de permitir iniciar', async () => {
    view.settings = { ...view.settings, reply: null, loading: true };
    await render();
    expect(container.textContent).toContain(m.comum_carregando());
    expect(button(m.live_voice_start())).toBeUndefined();
    view.settings = { ...view.settings, loading: false, error: 'settings-error' };
    await render();
    expect(container.textContent).toContain('settings-error');
    await click(m.sync_retry());
    expect(callbacks.onRetry).toHaveBeenCalledOnce();
    view.settings = { ...view.settings, reply: settingsReply(), error: null };
    await render();
    await click(m.live_voice_start());
    expect(callbacks.onStart).toHaveBeenCalledOnce();
  });

  it('mostra falha e nova tentativa com ajustes fechados mesmo conservando resposta anterior', async () => {
    view.settings.error = 'cached-settings-error';
    await render();
    expect(callbacks.onSettingsOpen).not.toHaveBeenCalled();
    expect(container.textContent).toContain('cached-settings-error');
    expect(button(m.live_voice_start())).toBeDefined();
    await click(m.sync_retry());
    expect(callbacks.onRetry).toHaveBeenCalledOnce();
    await click(m.native_live_voice_settings_memory());
    expect(button(`${m.live_voice_account()}: account-a`)).toBeDefined();
    expect(button(`${m.live_voice_mode_direct()}: ${m.live_voice_model()}: model-a`)).toBeDefined();
    expect(callbacks.onSaveSettings).not.toHaveBeenCalled();
  });

  it('explica Codex ausente e conta ausente sem oferecer iniciar', async () => {
    view.settings.reply = { ...settingsReply(), codex: false };
    await render();
    expect(container.textContent).toContain(m.live_voice_error_codex_missing());
    expect(button(m.live_voice_start())).toBeUndefined();
    view.settings.reply = { ...settingsReply(), enabled: false };
    await render();
    expect(container.textContent).toContain(m.live_voice_error_no_account());
    expect(button(m.live_voice_start())).toBeUndefined();
  });

  it('distingue permissões nativas de erro do navegador e conserva o detalhe recebido', async () => {
    view.phase = 'closed';
    view.error = { code: 'microphone', detail: 'device-permission-detail' };
    await render();
    expect(container.textContent).toContain(m.native_live_voice_error_microphone());
    expect(container.textContent).not.toContain(m.live_voice_error_microphone());
    expect(container.textContent).toContain('device-permission-detail');
    await click(m.sync_retry());
    expect(callbacks.onRetry).toHaveBeenCalledOnce();
  });

  it.each([
    { code: 'audio_restore', message: () => m.native_live_voice_error_audio_restore() },
    { code: 'opening_timeout', message: () => m.native_live_voice_error_opening_timeout() },
  ])('apresenta a mensagem traduzida de $code sem mostrar o código cru', async ({ code, message }) => {
    view.phase = 'closed';
    view.error = { code };
    await render();
    expect(container.textContent).toContain(message());
    expect(container.textContent).not.toContain(code);
    await click(m.sync_retry());
    expect(callbacks.onRetry).toHaveBeenCalledOnce();
  });

  it('não exibe token recebido como código de erro desconhecido', async () => {
    view.phase = 'closed';
    view.error = { code: 'token-fixture' };
    await render();
    expect(container.textContent).toContain(m.live_voice_error_failed());
    expect(container.textContent).not.toContain('token-fixture');
  });

  it('mantém mudo e encerrar acessíveis enquanto conecta e fecha a folha sem parar a chamada', async () => {
    view.phase = 'connecting';
    await render();
    expect(container.textContent).toContain(m.live_voice_connecting());
    await click(m.live_voice_mute());
    expect(callbacks.onToggleMute).toHaveBeenCalledOnce();
    view.phase = 'live';
    view.muted = true;
    await render();
    expect(button(m.live_voice_unmute()).getAttribute('aria-selected')).toBe('true');
    await click(m.native_live_voice_close());
    await click('native-dismiss');
    expect(callbacks.onDismiss).toHaveBeenCalledTimes(2);
    expect(callbacks.onStop).not.toHaveBeenCalled();
    await click(m.live_voice_stop());
    expect(callbacks.onStop).toHaveBeenCalledOnce();
  });

  it('segura o mudo durante ditado sem oferecer ativação que não teria efeito e mantém Encerrar', async () => {
    view.phase = 'live';
    view.dictating = true;
    view.state = liveState();
    await render();
    expect(button(m.native_live_voice_dictation_hold_short()).disabled).toBe(true);
    expect(button(m.live_voice_unmute())).toBeUndefined();
    expect(container.textContent).toContain(m.native_live_voice_dictation_hold());
    expect(container.textContent).toContain(m.codex_voice_muted());
    await click(m.native_live_voice_dictation_hold_short());
    expect(callbacks.onToggleMute).not.toHaveBeenCalled();
    await click(m.live_voice_stop());
    expect(callbacks.onStop).toHaveBeenCalledOnce();
  });

  it('troca somente o modo pelo callback e segue a seleção recebida do servidor', async () => {
    view.phase = 'live';
    view.state = liveState();
    await render();
    expect(button(m.live_voice_mode_direct()).getAttribute('aria-selected')).toBe('true');
    await click(m.live_voice_mode_plan());
    expect(callbacks.onMode).toHaveBeenCalledWith('planejar');
    expect(callbacks.onStart).not.toHaveBeenCalled();
    expect(callbacks.onSaveSettings).not.toHaveBeenCalled();
    expect(button(m.live_voice_mode_direct()).getAttribute('aria-selected')).toBe('true');
    view.state = { ...view.state, mode: 'plan' };
    await render();
    expect(button(m.live_voice_mode_plan()).getAttribute('aria-selected')).toBe('true');
  });

  it('não inventa modo nem atividade em enums futuros e conserva a chamada e seus controles', async () => {
    const message = parseLiveVoiceMessage(JSON.stringify({ type: 'state', state: { ...liveState(),
      mode: 'future-mode', activity: 'future-activity', draft: 'draft-fixture',
      backstage: [{ kind: 'future-backstage', text: 'backstage-fixture' }],
      action: { kind: 'future-action', text: 'action-fixture' }, progress: 'progress-fixture', thought: 'thought-fixture' } }));
    if (message?.type !== 'state') throw new Error('missing-state');
    view.phase = 'live';
    view.state = message.state;
    await render();
    expect(button(m.live_voice_mode_direct())).toBeUndefined();
    expect(button(m.live_voice_mode_plan())).toBeUndefined();
    expect(container.textContent).toContain(m.codex_voice_listening());
    expect(container.textContent).toContain('draft-fixture');
    for (const text of ['action-fixture', 'progress-fixture', 'thought-fixture', 'future-mode', 'future-activity'])
      expect(container.textContent).not.toContain(text);
    await click(m.native_live_voice_details());
    for (const text of ['backstage-fixture', 'effective-model', '25%']) expect(container.textContent).toContain(text);
    await click(m.native_live_voice_settings_memory());
    expect(button(m.live_voice_mute()).disabled).toBe(false);
    expect(button(m.live_voice_stop()).disabled).toBe(false);
    await click(m.live_voice_mute());
    expect(callbacks.onToggleMute).toHaveBeenCalledOnce();
    await click(m.live_voice_stop());
    expect(callbacks.onStop).toHaveBeenCalledOnce();
    expect(callbacks.onMode).not.toHaveBeenCalled();
    view.state = { ...message.state, mode: 'plan' };
    await render();
    expect(button(m.live_voice_mode_plan()).getAttribute('aria-selected')).toBe('true');
  });

  it('conserva os textos de ação e bastidores sem exigir categoria reconhecida', async () => {
    const message = parseLiveVoiceMessage(JSON.stringify({ type: 'state', state: { ...liveState(), activity: 'working',
      backstage: [{ kind: 'future-backstage', text: 'backstage-fixture' }], action: { kind: 'future-action', text: 'action-fixture' } } }));
    if (message?.type !== 'state') throw new Error('missing-state');
    view.phase = 'live';
    view.state = message.state;
    await render();
    expect(container.textContent).toContain(m.live_voice_status_working());
    expect(container.textContent).toContain('action-fixture');
    expect(container.textContent).not.toContain('future-action');
    await click(m.native_live_voice_details());
    expect(container.textContent).toContain('backstage-fixture');
    expect(container.textContent).not.toContain('future-backstage');
  });

  it('nível de quem fala chega pela assinatura e troca o estado sem nova vista', async () => {
    view.phase = 'live';
    view.state = { ...liveState(), plan: { path: '/plan-fixture.md', markdown: '# plan-fixture' } };
    let current = { input: 0, output: 0 };
    const listeners = new Set<() => void>();
    const levels = { get: () => current, subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; } };
    await act(async () => root.render(createElement(LiveVoiceSheet, { levels, open: true, view, servers, serverId, ...callbacks })));
    expect(container.textContent).toContain(m.codex_voice_listening());
    const markdownRenders = native.markdown.mock.calls.length;
    await act(async () => { current = { input: 0.5, output: 0 }; listeners.forEach(listener => listener()); });
    expect(container.textContent).toContain(m.live_voice_status_you());
    // Quem fala troca só o orbe: o plano não é redesenhado.
    expect(native.markdown.mock.calls.length).toBe(markdownRenders);
  });

  it('estado da chamada é região viva com o progresso, sem o cronômetro nem quem está falando', async () => {
    view.phase = 'live';
    view.liveSince = Date.now();
    view.state = { ...liveState(), activity: 'working', action: { kind: 'tool', text: 'tool-action' }, progress: 'step-progress' };
    view.levels = { input: 0.5, output: 0 };
    await render();
    expect(container.textContent).toContain(m.live_voice_status_you());
    const live = () => container.querySelector('[accessibilityliveregion="polite"]');
    const label = live()?.getAttribute('aria-label');
    expect(label).toContain(m.live_voice_status_working());
    expect(label).not.toContain(m.live_voice_status_you());
    expect(label).toContain('tool-action');
    expect(label).toContain('step-progress');
    expect(label).not.toContain('00:00');
    await act(async () => { vi.advanceTimersByTime(3000); });
    expect(container.textContent).toContain('00:03');
    expect(live()?.getAttribute('aria-label')).toBe(label);
  });

  it('no iOS a mudança de atividade é anunciada ao VoiceOver; o cronômetro não', async () => {
    native.os = 'ios';
    view.phase = 'live';
    view.liveSince = Date.now();
    view.state = { ...liveState(), activity: 'idle' };
    await render();
    await act(async () => { vi.advanceTimersByTime(2000); });
    expect(native.announce).not.toHaveBeenCalled();
    view = { ...view, state: { ...liveState(), activity: 'working', action: { kind: 'tool', text: 'tool-action' } } };
    await render();
    expect(native.announce).toHaveBeenCalledExactlyOnceWith(`${m.live_voice_status_working()}, tool-action`);
  });

  it('estilo do plano é o mesmo objeto entre renders com o tema inalterado', async () => {
    view.phase = 'live';
    view.state = { ...liveState(), plan: { path: '/plan-fixture.md', markdown: '# plan-fixture' } };
    await render();
    view = { ...view, liveSince: 1 };
    await render();
    const styles = native.markdown.mock.calls.map(([props]) => (props as { markdownStyle: unknown }).markdownStyle);
    expect(styles.length).toBeGreaterThanOrEqual(2);
    expect(new Set(styles).size).toBe(1);
  });

  it('entrega plano ao renderizador Markdown e mostra pedido, atividade, seguidas e escolhas efetivas', async () => {
    const markdown = '# plan-fixture\n\n- **step-fixture**';
    view.phase = 'live';
    view.state = { ...liveState(), plan: { path: '/plan-fixture.md', markdown }, draft: 'held-request',
      activity: 'working', action: { kind: 'tool', text: 'tool-action' }, progress: 'step-progress', thought: '**thought-tail**',
      error: { code: 'state-error', text: 'state-error-detail' },
      backstage: [{ kind: 'heard', text: 'heard-fixture' }], followed: [{ server: 'peer-fixture', name: 'session-fixture' }] };
    await render();
    expect(native.markdown).toHaveBeenCalledWith(expect.objectContaining({ markdown, flavor: 'github' }));
    expect(container.querySelector('[data-renderer="native-markdown"]')).not.toBeNull();
    expect(container.textContent).not.toContain(markdown);
    for (const data of ['held-request', 'tool-action', 'step-progress', 'thought-tail', 'state-error-detail']) {
      expect(container.textContent).toContain(data);
    }
    // Modelo, cota e bastidores ficam em Detalhes; a tela da chamada só mostra o estado.
    for (const data of ['heard-fixture', 'peer-fixture::session-fixture', 'effective-model', '25%']) expect(container.textContent).not.toContain(data);
    await click(m.native_live_voice_details());
    for (const data of ['heard-fixture', 'peer-fixture::session-fixture', 'effective-model', 'effective-effort', '25%']) {
      expect(container.textContent).toContain(data);
    }
  });

  it('tela da chamada mostra estado e controles; Detalhes mostra só o que a conta informou', async () => {
    view.phase = 'live';
    view.state = { ...liveState(), effective: { model: 'model-only', effort: null, tier: null },
      limits: { five_hour: null, seven_day: { used_percent: 19, resets_at: null } } };
    await render();
    // Nome acessível = texto visível (WCAG 2.5.3): quem fala "Detalhes" aciona o botão.
    for (const label of [m.live_voice_mute(), m.native_live_voice_details(), m.live_voice_stop()]) expect(button(label)?.textContent).toBe(label);
    for (const text of ['model-only', m.ctx_limite_7d(), m.native_live_voice_effective()]) expect(container.textContent).not.toContain(text);
    await click(m.native_live_voice_details());
    expect(button(m.live_voice_stop())).toBeUndefined();
    const section = (title: string) => [...container.querySelectorAll('[role="header"]')]
      .find(el => el.textContent === title)?.parentElement?.textContent ?? '';
    const inUse = section(m.native_live_voice_effective());
    expect(inUse).toContain('model-only');
    expect(inUse).not.toContain(m.live_voice_effort());
    expect(inUse).not.toContain(m.live_voice_tier());
    expect(section(m.native_live_voice_limits())).toContain(`${m.ctx_limite_7d()}19%`);
    expect(container.textContent).not.toContain(m.ctx_limite_5h());
    await click(m.native_live_voice_settings_memory());
    expect(button(m.live_voice_stop())).toBeDefined();
  });

  it('queda da chamada aparece também em Detalhes, com nova tentativa', async () => {
    view.phase = 'live';
    view.state = liveState();
    await render();
    await click(m.native_live_voice_details());
    view.phase = 'closed';
    view.error = { code: 'connection_lost' };
    await render();
    expect(container.textContent).toContain(m.live_voice_error_connection_lost());
    await click(m.sync_retry());
    expect(callbacks.onRetry).toHaveBeenCalledOnce();
  });

  it('nova tentativa fica fora do grupo lido de uma vez pelo VoiceOver', async () => {
    view.settings = { ...view.settings, reply: null, error: 'settings-error' };
    await render();
    const retry = button(m.sync_retry());
    expect(retry).toBeDefined();
    const group = [...container.querySelectorAll('div[aria-label]')].find(el => el.getAttribute('aria-label') === m.live_voice_error_failed());
    expect(group).toBeDefined();
    expect(group!.contains(retry)).toBe(false);
  });

  it('antes de ligar mostra que está pronta, o botão de iniciar e o caminho para os ajustes', async () => {
    await render();
    expect(container.textContent).toContain(m.native_live_voice_ready());
    expect(button(m.live_voice_start())).toBeDefined();
    expect(button(m.live_voice_mute())).toBeUndefined();
    expect(button(m.native_live_voice_settings_memory()).getAttribute('aria-expanded')).toBe('false');
  });

  it('mostra tempo somente ao vivo e retoma o início original ao reabrir a folha', async () => {
    view.phase = 'live';
    view.liveSince = Date.now();
    await render();
    expect(container.textContent).toContain('00:00');
    await act(async () => vi.advanceTimersByTime(2000));
    expect(container.textContent).toContain('00:02');
    await render(false);
    await act(async () => vi.advanceTimersByTime(3000));
    expect(container.textContent).toBe('');
    await render();
    expect(container.textContent).toContain('00:05');
    view.phase = 'closed';
    await render();
    expect(container.textContent).not.toContain('00:05');
  });

  it('abre catálogo e memória só por ação explícita e salva o objeto inteiro sem alterar os campos antes da resposta', async () => {
    await render();
    expect(callbacks.onSettingsOpen).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain(m.live_voice_memory_title());
    await click(m.native_live_voice_settings_memory());
    expect(callbacks.onSettingsOpen).toHaveBeenCalledOnce();
    expect(container.textContent).toContain(m.native_live_voice_next_call());
    const saved = settingsReply().settings;
    await choose(m.live_voice_account(), 'account-b');
    expect(callbacks.onSaveSettings).toHaveBeenLastCalledWith({ ...saved, codex_account: 'account-b' });
    expect(button(`${m.live_voice_account()}: account-a`)).toBeDefined();
    await choose(`${m.live_voice_mode_plan()}: ${m.live_voice_model()}`, 'model-a');
    expect(callbacks.onSaveSettings).toHaveBeenLastCalledWith({ ...saved, organizer: { ...saved.organizer,
      plan: { ...saved.organizer.plan, model: 'model-a', effort: 'medium' } } });
    expect(view.settings.reply?.settings).toEqual(saved);
    view.settings.saving = true;
    await render();
    expect(button(`${m.live_voice_account()}: account-a`).disabled).toBe(true);
    await click(`${m.live_voice_account()}: account-a`);
    expect(container.querySelectorAll('button[role="radio"]')).toHaveLength(0);
    expect(callbacks.onSaveSettings).toHaveBeenCalledTimes(2);
    view.settings.saving = false;
    view.settings.saveError = 'save-failed';
    await render();
    expect(container.textContent).toContain('save-failed');
    expect(button(`${m.live_voice_account()}: account-a`).disabled).toBe(false);
    await click(m.native_live_voice_reload_settings_memory());
    expect(callbacks.onSettingsOpen).toHaveBeenCalledTimes(2);
  });

  it.each([
    { current: 'medium', efforts: ['low', 'medium'], defaultEffort: 'low', expected: 'medium' },
    { current: 'high', efforts: ['low', 'medium'], defaultEffort: 'unknown-effort', expected: 'low' },
    { current: 'high', efforts: undefined, defaultEffort: null, expected: 'high' },
  ])('troca de modelo conserva esforço válido ou desconhecido e só usa opção suportada ($expected)', async ({ current, efforts, defaultEffort, expected }) => {
    view.settings.reply!.settings.organizer.plan.effort = current;
    view.settings.models = [{ id: 'model-new', efforts, default_effort: defaultEffort }];
    await render();
    await click(m.native_live_voice_settings_memory());
    await choose(`${m.live_voice_mode_plan()}: ${m.live_voice_model()}`, 'model-new');
    const saved = view.settings.reply!.settings;
    expect(callbacks.onSaveSettings).toHaveBeenCalledWith({ ...saved, organizer: { ...saved.organizer,
      plan: { ...saved.organizer.plan, model: 'model-new', effort: expected } } });
    expect(button(`${m.live_voice_mode_plan()}: ${m.live_voice_model()}: model-b`)).toBeDefined();
  });

  it('catálogo vazio ou com erro conserva conta, modelo, esforço, velocidade e voz atuais', async () => {
    view.settings.accounts = [];
    view.settings.models = [];
    view.settings.reply!.voices = [];
    await render();
    await click(m.native_live_voice_settings_memory());
    expect(container.textContent).toContain(m.native_live_voice_accounts_empty());
    expect(container.textContent).toContain(m.native_live_voice_models_empty());
    expect(button(`${m.live_voice_account()}: account-a`)).toBeDefined();
    expect(button(`${m.live_voice_voice()}: voice-a`)).toBeDefined();
    expect(button(`${m.live_voice_mode_plan()}: ${m.live_voice_model()}: model-b`)).toBeDefined();
    expect(button(`${m.live_voice_mode_plan()}: ${m.live_voice_effort()}: high`)).toBeDefined();
    view.settings.modelsError = 'catalog-error';
    view.settings.error = 'accounts-error';
    await render();
    expect(container.textContent).toContain(m.comum_falha_carregar_modelos());
    expect(container.textContent).toContain('catalog-error');
    expect(container.textContent).toContain('accounts-error');
    expect(callbacks.onSaveSettings).not.toHaveBeenCalled();
  });

  it('memória trata carregamento, vazio, falha e esquecimento pelo id sem remover a entrada antes da resposta', async () => {
    view.memory.loading = true;
    await render();
    await click(m.native_live_voice_settings_memory());
    expect(container.textContent).toContain(m.comum_carregando());
    expect(container.textContent).not.toContain(m.live_voice_memory_empty());
    view.memory.loading = false;
    await render();
    expect(container.textContent).toContain(m.live_voice_memory_empty());
    view.memory.entries = [{ id: 'memory-id', kind: 'preferencia', text: 'memory-fixture', learned: 'learned-fixture', confirmed: 'confirmed-fixture' }];
    view.memory.error = 'memory-read-error';
    view.memory.forgetError = 'memory-forget-error';
    await render();
    expect(container.textContent).toContain(m.live_voice_memory_error());
    expect(container.textContent).toContain(m.live_voice_memory_forget_failed());
    expect(container.textContent).toContain(m.live_voice_memory_kind_preferencia());
    expect(container.textContent).toContain('memory-read-error');
    expect(container.textContent).toContain('memory-forget-error');
    await click(m.live_voice_memory_forget({ text: 'memory-fixture' }));
    expect(callbacks.onForgetMemory).toHaveBeenCalledWith('memory-id');
    expect(container.textContent).toContain('memory-fixture');
  });
});

describe('LiveVoiceButton', () => {
  it('respeita visibilidade e abre pelo callback, sem iniciar ou encerrar chamadas por conta própria', async () => {
    const props = { phase: 'idle' as const, muted: false, levels: { input: 0, output: 0 }, error: false, visible: false, onPress: vi.fn() };
    await act(async () => root.render(createElement(LiveVoiceButton, props)));
    expect(container.textContent).toBe('');
    await act(async () => root.render(createElement(LiveVoiceButton, { ...props, visible: true })));
    await act(async () => container.querySelector('button')!.click());
    expect(props.onPress).toHaveBeenCalledOnce();
  });

  it('expõe conectando, ao vivo, mudo e erro sem anunciar valores dos níveis', async () => {
    const props = { phase: 'connecting' as const, muted: false, levels: { input: 0.6, output: 0.4 }, error: false, visible: true, onPress: vi.fn() };
    await act(async () => root.render(createElement(LiveVoiceButton, props)));
    expect(container.querySelector('button')!.getAttribute('aria-busy')).toBe('true');
    expect(container.querySelector('button')!.getAttribute('aria-label')).toContain(m.live_voice_connecting());
    await act(async () => root.render(createElement(LiveVoiceButton, { ...props, phase: 'live', muted: true })));
    expect(container.querySelector('button')!.getAttribute('aria-label')).toContain(m.codex_voice_muted());
    expect(container.textContent).not.toContain('0.6');
    expect(container.querySelector('[testid="voice-bars"]')).toBeNull();
    await act(async () => root.render(createElement(LiveVoiceButton, { ...props, phase: 'live', muted: false })));
    expect(container.querySelector('[testid="voice-bars"]')).not.toBeNull();
    await act(async () => root.render(createElement(LiveVoiceButton, { ...props, phase: 'closed', error: true })));
    expect(container.querySelector('[testid="voice-bars"]')).toBeNull();
    expect(container.textContent).toContain(m.native_live_voice_error_short());
    expect(container.querySelector('button')!.getAttribute('aria-label')).toContain(m.live_voice_error_failed());
  });
});
