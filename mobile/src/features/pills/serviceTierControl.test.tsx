// @vitest-environment happy-dom
import { act, createElement, forwardRef, useImperativeHandle, useState, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CodexModelsResponse, ModelOptionsResponse, Server, SessionInfo, StateEvent } from '@hangar/core';
import { SessionSettingsButton, SessionSettingsControl } from '../../chat/SessionSettings';
import { chatStore, _resetChatsForTests } from '../../stores/chat';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const routing = vi.hoisted(() => ({ push: vi.fn() }));
const nativeSheet = vi.hoisted(() => ({ finishDismiss: null as (() => void) | null }));
vi.mock('expo-router', () => ({ useRouter: () => routing }));
const api = vi.hoisted(() => ({ credentials: vi.fn(), codex: vi.fn(), models: vi.fn(), tier: vi.fn(), permissions: vi.fn(), modelWrite: vi.fn(), modeWrite: vi.fn(),
  engines: vi.fn(), providerModels: vi.fn(), providerWrite: vi.fn() }));
const state = vi.hoisted(() => ({
  servers: [] as Server[], activeId: 'b',
  rows: [] as (SessionInfo & { serverId: string })[],
  stateEvent: null as StateEvent | null, statusLine: null as string | null,
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  getCodexModels: api.codex, getModelOptions: api.models, setServiceTier: api.tier, getPermissionModes: api.permissions,
  getCredentialsForServer: api.credentials,
  getEnginesForServer: api.engines, claudeProviderModels: api.providerModels, switchClaudeProvider: api.providerWrite,
  setModelEffort: api.modelWrite, setEngineModel: api.modelWrite, setCodexModel: api.modelWrite,
  setPermissionMode: api.modeWrite, setCodexMode: api.modeWrite,
}));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {} }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('@lodev09/react-native-true-sheet', () => ({ TrueSheet: forwardRef(function NativeSheet({ children, onDidDismiss }: { children: ReactNode; onDidDismiss?: () => void }, ref) {
  const [visible, setVisible] = useState(false);
  useImperativeHandle(ref, () => ({ present: async () => { setVisible(true); }, dismiss: async () => { setVisible(false); } }));
  nativeSheet.finishDismiss = () => { setVisible(false); onDidDismiss?.(); };
  return visible ? createElement('div', null, children) : null;
}) }));
vi.mock('expo-blur', async () => ({ BlurView: (await import('react-native')).View }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('sonner-native', () => ({ toast: Object.assign(() => {}, { success: () => {}, error: () => {}, dismiss: () => {} }), Toaster: () => null }));
vi.mock('../../ui/AnchoredPanel', () => ({
  AnchoredPanel: ({ open, children, onClose }: { open: boolean; children: ReactNode; onClose: () => void }) => open
    ? createElement('div', null, children, createElement('button', { onClick: onClose, 'aria-label': 'dismiss-settings' })) : null,
}));
vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(),
  Pressable: ({ accessibilityValue, ...props }: Record<string, any>) => createElement('button', {
    'aria-label': props.accessibilityLabel, 'aria-valuetext': accessibilityValue?.text,
    role: props.accessibilityRole, disabled: props.disabled, onClick: props.onPress,
    'aria-selected': props.accessibilityRole === 'tab' ? props.accessibilityState?.selected : undefined,
    'aria-checked': props.accessibilityRole === 'radio' ? props.accessibilityState?.checked : undefined,
    style: Object.assign({}, ...[typeof props.style === 'function' ? props.style({ pressed: false }) : props.style].flat(Infinity).filter(Boolean)),
  }, props.children),
  Switch: ({ value, onValueChange, disabled, accessibilityLabel, accessibilityState }: {
    value: boolean; onValueChange: (value: boolean) => void; disabled?: boolean;
    accessibilityLabel?: string; accessibilityState?: { busy?: boolean };
  }) => createElement('button', { role: 'switch', 'aria-checked': value, disabled, 'aria-label': accessibilityLabel,
    'aria-busy': accessibilityState?.busy, onClick: () => onValueChange(!value) }),
}));
vi.mock('../../paraglide/messages', async original => ({
  ...await original<typeof import('../../paraglide/messages')>(),
  ...Object.fromEntries((
    'reasoning_level_low reasoning_level_medium reasoning_level_high reasoning_level_xhigh reasoning_level_max reasoning_level_ultra '
    + 'composer_session_settings composer_session_settings_hint composer_modelo native_close native_ctl_model_title native_ctl_search '
    + 'native_ctl_no_results comum_nenhum_modelo native_new_chat_reasoning modelo_sem_niveis native_ctl_mode_title '
    + 'native_ctl_loading native_ctl_retry native_ctl_probe native_mode_probe_hint permissao_sem_ciclo '
    + 'chat_mode_plan chat_mode_normal native_codex_permission_probe_hint composer_codex_approval '
    + 'native_mode_plan native_mode_auto native_mode_manual native_mode_acceptEdits native_mode_bypassPermissions native_mode_dontAsk '
    + 'native_ctl_fast native_ctl_fast_on native_ctl_fast_off native_ctl_fast_hint native_ctl_fast_unavailable '
    + 'native_ctl_speed native_ctl_speed_normal native_ctl_speed_ultrafast native_ctl_speed_normal_hint native_ctl_speed_fast_hint '
    + 'native_ctl_speed_ultrafast_hint native_ctl_speed_ultrafast_unavailable '
    + 'native_invalid_response servidor_nao_existe chat_servidor_removido cota_conta_ativa accounts_switch_title conta_padrao '
    + 'native_ctl_all_models native_ctl_featured_models comum_voltar codex_ui_concluir native_usage_card_session live_voice_action_usage_label codex_limites_titulo '
    + 'native_claude_provider_picker native_claude_provider_back native_claude_provider_apply native_claude_provider_confirm comum_cancelar'
  ).split(' ').map((key) => [key, () => key])),
  native_ctl_failed: ({ reason }: { reason: string }) => `native_ctl_failed ${reason}`,
}));

const roots: Root[] = [];
function codex(tiers: CodexModelsResponse['models'][number]['serviceTiers'] = [{ id: 'priority' }], tier: string | null = 'default'): CodexModelsResponse {
  return { current: { model: 'gpt-current', effort: 'high', service_tier: tier }, models: [
    { model: 'gpt-current', displayName: 'GPT', description: null, defaultEffort: 'high', efforts: [], serviceTiers: tiers },
  ] };
}
function engine(): ModelOptionsResponse {
  return { kind: 'engine', engine: 'local-gpt', supports_fast: true,
    current: { model: 'gpt-current[1m]', service_tier: 'default' },
    models: [{ id: 'gpt-current', supports_fast: true }] };
}
function unsupportedModel(provider: string): CodexModelsResponse | ModelOptionsResponse {
  if (provider === 'codex') {
    const next = codex([]);
    next.current.model = next.models[0].model = 'gpt-next';
    return next;
  }
  const next = engine();
  next.current!.model = 'gpt-next';
  next.models = [{ id: 'gpt-next', supports_fast: false }];
  return next;
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
async function mount(provider = 'codex', serverId = 'a', openRequest?: { which: 'model' | 'effort' | 'mode' | 'account' | 'usage'; n: number }) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  const element = () => createElement(SessionSettingsButton, { serverId, name: 'sess', provider, headless: false, openRequest });
  const open = async () => {
    await act(async () => { syncStores(serverId); container.querySelector<HTMLButtonElement>('[aria-label="composer_session_settings"]')!.click(); });
    await act(async () => container.querySelector<HTMLButtonElement>('[role="tab"][aria-label="native_usage_card_session"]')!.click());
  };
  const close = async () => { await act(async () => { container.querySelector<HTMLButtonElement>('[aria-label="native_close"]')!.click(); nativeSheet.finishDismiss?.(); }); };
  await act(async () => { syncStores(serverId); root.render(element()); });
  if (!openRequest) {
    expect(fastButton(container)).toBeNull();
    await open();
  }
  return { container, open, close, refresh: async () => { await act(async () => { syncStores(serverId); root.render(element()); }); } };
}
function syncStores(serverId: string) {
  useServers.setState({ servers: state.servers, activeId: state.activeId, ready: true, vaultReadable: true, aviso: null });
  useSessions.setState({ rows: state.rows.map(row => ({ serverLabel: row.serverId, serverColor: '#7c87e8', ...row })), byServerRecord: {} });
  chatStore(serverId, 'sess').use.setState({ statusLine: state.statusLine, stateEvent: state.stateEvent, stats: null });
}
const speedButton = (container: HTMLElement, label: string) => container.querySelector<HTMLButtonElement>(`[role="radio"][aria-label="${label}"]`)!;
const fastButton = (container: HTMLElement) => speedButton(container, 'native_ctl_fast');
const ultraButton = (container: HTMLElement) => speedButton(container, 'native_ctl_speed_ultrafast');
const checked = (container: HTMLElement) => fastButton(container).getAttribute('aria-checked');
// Ligar/desligar Fast pela escolha de velocidade: Fast marcado volta a Normal, senão marca Fast.
const toggle = (container: HTMLElement) => fastButton(container) && checked(container) === 'true'
  ? speedButton(container, 'native_ctl_speed_normal') : fastButton(container);

it('layout A separa as abas e mostra três modelos incluindo o atual', async () => {
  const catalog = codex();
  catalog.models = ['first', 'second', 'third', 'gpt-current', 'last'].map(model => ({
    ...catalog.models[0], model, displayName: model,
  }));
  api.codex.mockResolvedValue(catalog);
  const { container } = await mount('codex', 'a', { which: 'model', n: 1 });
  const tabs = container.querySelectorAll('[role="tab"]');
  expect(tabs).toHaveLength(3);
  const models = [...container.querySelectorAll('[role="radio"]')].map(row => row.textContent);
  expect(models).toEqual(['first', 'second', 'gpt-current']);
  expect(fastButton(container)).toBeNull();
});

it('layout A abre todos os modelos e o raciocínio e volta ao resumo após aplicar', async () => {
  const catalog = codex();
  catalog.models[0].efforts = [{ value: 'low', description: null }, { value: 'high', description: null }];
  catalog.models.push(...['other-a', 'other-b', 'other-c'].map(model => ({ ...catalog.models[0], model, displayName: model })));
  api.codex.mockResolvedValue(catalog);
  api.modelWrite.mockResolvedValue({ ok: true });
  const { container } = await mount('codex', 'a', { which: 'model', n: 1 });
  const action = (label: string) => container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!;
  expect(action('native_ctl_all_models')).not.toBeNull();
  await act(async () => action('native_ctl_all_models').click());
  expect(container.querySelectorAll('[role="radio"]')).toHaveLength(4);
  expect(container.querySelector('[aria-label="native_ctl_search"]')).not.toBeNull();
  await act(async () => action('comum_voltar').click());
  expect(container.querySelectorAll('[role="radio"]')).toHaveLength(3);
  await act(async () => action('native_new_chat_reasoning').click());
  expect(container.querySelectorAll('[role="radio"]')).toHaveLength(2);
  await act(async () => action('reasoning_level_low').click());
  expect(api.modelWrite).toHaveBeenCalledExactlyOnceWith('sess', 'gpt-current', 'low', state.servers[0]);
  expect(container.querySelectorAll('[role="radio"]')).toHaveLength(3);
});

it('escolhe o modelo da conta e confirma sem perder o resultado da lista ao vivo', async () => {
  const source = { lifecycle_id: 'k:claude', jsonl: '/example/conversation.jsonl', headless: false, engine: null, engine_account: null, engine_binding: null };
  state.rows[0] = { ...state.rows[0], provider: 'claude', ...source };
  api.engines.mockResolvedValue({ claude_provider_switch: true, motores: {
    pro: { label: 'Pro', cliproxy_binding: 'bound-pro', cliproxy_bound_account: { account: 'pro-account', label: 'ChatGPT Pro' } },
    plus: { label: 'Plus', cliproxy_binding: 'bound-plus', cliproxy_bound_account: { account: 'plus-account', label: 'ChatGPT Plus' } },
  } });
  const first = deferred<{ id: string }[]>();
  api.providerModels.mockImplementation((_server, target) => target.engine === 'pro' ? first.promise : Promise.resolve([{ id: 'gpt-plus' }]));
  const written = deferred<{ ok: boolean; engine: string; engine_account: string; engine_binding: string; model: string; service_tier: string }>();
  api.providerWrite.mockReturnValue(written.promise);
  const { container, refresh } = await mount('claude', 'a', { which: 'model', n: 1 });
  const action = (label: string) => container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
  const click = async (label: string) => { await act(async () => action(label)!.click()); };
  await click('native_claude_provider_picker');
  await click('Pro');
  await click('Plus');
  await act(async () => first.resolve([{ id: 'gpt-pro' }]));
  expect(action('gpt-pro')).toBeNull();
  expect(action('native_claude_provider_apply')?.disabled).toBe(true);
  await click('gpt-plus');
  await click('native_claude_provider_apply');
  await click('native_claude_provider_back');
  expect(action('gpt-plus')?.getAttribute('aria-checked')).toBe('true');
  await click('native_claude_provider_apply');
  await click('comum_cancelar');
  expect(api.providerWrite).not.toHaveBeenCalled();
  await click('native_claude_provider_picker');
  await click('Plus');
  await click('gpt-plus');
  await click('native_claude_provider_apply');
  await click('native_claude_provider_confirm');
  const destination = { engine: 'plus', engine_account: 'plus-account', engine_binding: 'bound-plus', model: 'gpt-plus' };
  expect(api.providerWrite).toHaveBeenCalledExactlyOnceWith(state.servers[0], 'sess', { ...destination, source });
  state.rows[0] = { ...state.rows[0], ...destination };
  await refresh();
  await act(async () => written.resolve({ ok: true, ...destination, service_tier: 'default' }));
  expect(action('native_claude_provider_confirm')).toBeNull();
});

it('layout A mantém Codex aberto e relê os níveis depois de trocar modelo', async () => {
  const catalog = codex();
  catalog.models[0].efforts = [{ value: 'low', description: null }, { value: 'high', description: null }];
  catalog.models.push({ ...catalog.models[0], model: 'gpt-next', displayName: 'Next', defaultEffort: 'medium', efforts: [{ value: 'medium', description: null }, { value: 'max', description: null }] });
  api.codex.mockImplementation(async () => catalog);
  api.modelWrite.mockImplementation(async (_name: string, model: string, effort: string) => {
    catalog.current = { ...catalog.current, model, effort };
    return { ok: true };
  });
  const { container } = await mount('codex', 'a', { which: 'model', n: 1 });
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Next"]')!.click());
  expect(container.querySelectorAll('[role="tab"]')).toHaveLength(3);
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="native_new_chat_reasoning"]')!.click());
  expect([...container.querySelectorAll('[role="radio"]')].map(row => row.textContent)).toEqual(['reasoning_level_medium', 'reasoning_level_max']);
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="reasoning_level_max"]')!.click());
  expect(api.modelWrite.mock.calls).toEqual([
    ['sess', 'gpt-next', 'medium', state.servers[0]],
    ['sess', 'gpt-next', 'max', state.servers[0]],
  ]);
});

it('layout A agrupa modo Fast e aprovação na aba Sessão', async () => {
  const catalog = codex();
  catalog.current.mode = 'default';
  api.codex.mockResolvedValue(catalog);
  api.modeWrite.mockResolvedValue({ mode: 'plan' });
  const { container } = await mount('codex', 'a', { which: 'account', n: 1 });
  expect(fastButton(container)).not.toBeNull();
  const mode =container.querySelector<HTMLButtonElement>('[aria-label="native_ctl_mode_title"]');
  expect(mode).not.toBeNull();
  expect(container.querySelector('[aria-label="composer_codex_approval"]')).not.toBeNull();
  await act(async () => mode!.click());
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="chat_mode_plan"]')!.click());
  expect(api.modeWrite).toHaveBeenCalledExactlyOnceWith('sess', 'plan', state.servers[0]);
  expect(container.querySelectorAll('[role="tab"]')).toHaveLength(3);
  expect(fastButton(container)).not.toBeNull();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="composer_codex_approval"]')!.click());
  expect(container.textContent).toContain('native_codex_permission_probe_hint');
});

it('layout A fecha para a confirmação Claude sem pintar o modelo como aplicado', async () => {
  state.rows[0] = { ...state.rows[0], provider: 'claude', model: 'Opus 5.5' };
  api.models.mockResolvedValue({ kind: 'claude', engine: null, models: [{ id: 'opus', name: 'Opus 5.5' }, { id: 'fable', name: 'Fable 5.1' }] });
  api.modelWrite.mockResolvedValue({ ok: true, pending_confirm: 'high' });
  const { container } = await mount('claude', 'a', { which: 'model', n: 1 });
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Fable 5.1"]')!.click());
  expect(container.querySelector('[role="tab"]')).toBeNull();
  expect(container.querySelector('[aria-label="composer_session_settings"]')?.textContent).toContain('Opus 5.5');
});

it('layout A mantém Claude aberto quando a troca é confirmada sem pergunta', async () => {
  state.rows[0] = { ...state.rows[0], provider: 'claude', model: 'Opus 5.5' };
  api.models.mockResolvedValue({ kind: 'claude', engine: null, models: [{ id: 'opus', name: 'Opus 5.5' }, { id: 'fable', name: 'Fable 5.1' }] });
  api.modelWrite.mockResolvedValue({ ok: true, scope: 'session' });
  const { container } = await mount('claude', 'a', { which: 'model', n: 1 });
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Fable 5.1"]')!.click());
  expect(container.querySelectorAll('[role="tab"]')).toHaveLength(3);
  expect(container.querySelector('[aria-label="composer_session_settings"]')?.textContent).toContain('Fable 5.1');
});

it('layout A volta ao modelo inicial usando o padrão dele em vez do esforço antigo', async () => {
  const catalog = codex();
  catalog.models[0].defaultEffort = 'low';
  catalog.models[0].efforts = [{ value: 'low', description: null }, { value: 'high', description: null }];
  catalog.models.push({ ...catalog.models[0], model: 'gpt-next', displayName: 'Next', defaultEffort: 'medium', efforts: [{ value: 'medium', description: null }] });
  let current = { ...catalog.current };
  api.codex.mockImplementation(async () => ({ ...catalog, current: { ...current } }));
  api.modelWrite.mockImplementation(async (_name: string, model: string, effort: string) => {
    current = { ...current, model, effort };
    return { ok: true };
  });
  const { container } = await mount('codex', 'a', { which: 'model', n: 1 });
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Next"]')!.click());
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="GPT"]')!.click());
  expect(api.modelWrite).toHaveBeenLastCalledWith('sess', 'gpt-current', 'low', state.servers[0]);
});

it('layout A acompanha o modelo informado pelo SSE enquanto a folha está aberta', async () => {
  const catalog = codex();
  catalog.models.push({ ...catalog.models[0], model: 'gpt-next', displayName: 'Next' });
  api.codex.mockResolvedValue(catalog);
  state.statusLine = '🤖 gpt-current │ 💬 1k/1k 4k/200k';
  const { container } = await mount('codex', 'a', { which: 'model', n: 1 });
  expect(container.querySelector('[aria-label="GPT"]')?.getAttribute('aria-checked')).toBe('true');
  state.statusLine = '🤖 gpt-next │ 💬 1k/1k 4k/200k';
  api.codex.mockResolvedValue({ ...catalog, current: { ...catalog.current, model: 'gpt-next' } });
  await act(async () => chatStore('a', 'sess').use.setState({ statusLine: state.statusLine }));
  expect(container.querySelector('[aria-label="Next"]')?.getAttribute('aria-checked')).toBe('true');
});

it('layout A explica quando o modelo não oferece níveis de raciocínio', async () => {
  const { container } = await mount('codex', 'a', { which: 'effort', n: 1 });
  expect(container.textContent).toContain('modelo_sem_niveis');
  expect(container.querySelector('[role="radio"]')).toBeNull();
});

it('layout A sai do detalhe quando o modelo deixa de oferecer raciocínio', async () => {
  state.statusLine = '🤖 Opus5.5 (high)';
  api.models.mockResolvedValue({ kind: 'claude', engine: null, models: [{ id: 'opus', name: 'Opus 5.5' }, { id: 'haiku', name: 'Haiku 4.5' }] });
  const { container, refresh } = await mount('claude', 'a', { which: 'effort', n: 1 });
  expect(container.querySelector('[role="radio"]')).not.toBeNull();
  state.statusLine = '🤖 Haiku4.5';
  await refresh();
  expect(container.querySelectorAll('[role="tab"]')).toHaveLength(3);
  expect(container.querySelector('button[aria-label="native_new_chat_reasoning"]')).toBeNull();
});

it('layout A esconde raciocínio do Haiku informado pela lista antes da statusline', async () => {
  state.rows[0] = { ...state.rows[0], provider: 'claude', model: 'Haiku4.5' };
  api.models.mockResolvedValue({ kind: 'claude', engine: null, models: [{ id: 'haiku', name: 'Haiku 4.5' }] });
  const { container } = await mount('claude', 'a', { which: 'model', n: 1 });
  expect(container.querySelector('[aria-label="Haiku 4.5"]')).not.toBeNull();
  expect(container.querySelector('button[aria-label="native_new_chat_reasoning"]')).toBeNull();
});

it('layout A relê Fast após trocar modelo mesmo antes de chegar SSE', async () => {
  const catalog = codex();
  catalog.models.push({ ...catalog.models[0], model: 'gpt-next', displayName: 'Next', serviceTiers: [] });
  let current = { ...catalog.current };
  api.codex.mockImplementation(async () => ({ ...catalog, current: { ...current } }));
  api.modelWrite.mockImplementation(async (_name: string, model: string, effort: string) => {
    current = { ...current, model, effort };
    return { ok: true };
  });
  const { container } = await mount('codex', 'a', { which: 'model', n: 1 });
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Next"]')!.click());
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="native_usage_card_session"]')!.click());
  expect(toggle(container).disabled).toBe(true);
  await act(async () => toggle(container).click());
  expect(api.tier).not.toHaveBeenCalled();
});

it('layout A conserva a identidade da variante 1M e permite voltar ao modelo básico', async () => {
  state.rows[0] = { ...state.rows[0], provider: 'claude', model: 'Opus' };
  api.models.mockResolvedValue({ kind: 'claude', engine: null, models: [{ id: 'opus', name: 'Opus' }, { id: 'opus[1m]', name: 'Opus (1M context)' }] });
  api.modelWrite.mockResolvedValue({ ok: true, scope: 'session' });
  const { container } = await mount('claude', 'a', { which: 'model', n: 1 });
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Opus (1M context)"]')!.click());
  expect(container.querySelector('[aria-label="Opus (1M context)"]')?.getAttribute('aria-checked')).toBe('true');
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="Opus"]')!.click());
  expect(api.modelWrite.mock.calls).toEqual([
    ['sess', { model: 'opus[1m]', scope: 'session' }, state.servers[0]],
    ['sess', { model: 'opus', scope: 'session' }, state.servers[0]],
  ]);
});

it('layout A abre limites do Codex apenas depois do fechamento nativo', async () => {
  state.rows[0].provider = 'codex';
  const { container } = await mount('codex', 'a', { which: 'usage', n: 1 });
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="codex_limites_titulo"]')!.click());
  expect(routing.push).not.toHaveBeenCalled();
  expect(container.querySelector('[role="tab"]')).toBeNull();
  await act(async () => nativeSheet.finishDismiss!());
  expect(routing.push).toHaveBeenCalledExactlyOnceWith('/s/a/sess/codex-limits');
  await act(async () => nativeSheet.finishDismiss!());
  expect(routing.push).toHaveBeenCalledExactlyOnceWith('/s/a/sess/codex-limits');
});

beforeEach(() => {
  vi.resetAllMocks();
  nativeSheet.finishDismiss = null;
  api.credentials.mockResolvedValue([]);
  api.engines.mockResolvedValue({ motores: {} });
  state.servers = [
    { id: 'a', label: 'A', baseUrl: 'https://a.test', token: 'a' },
    { id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'b' },
  ];
  state.activeId = 'b';
  state.rows = [{ name: 'sess', serverId: 'a', state: 'idle', engine: null }];
  state.stateEvent = null;
  state.statusLine = null;
  api.codex.mockResolvedValue(codex());
  api.models.mockResolvedValue(engine());
  api.permissions.mockResolvedValue({ current: 'bypassPermissions', modes: [], sondavel: false });
  api.tier.mockResolvedValue({ ok: true, service_tier: 'priority' });
  vi.stubGlobal('fetch', vi.fn(() => { throw new Error('unexpected-network'); }));
});
afterEach(() => {
  act(() => { for (const root of roots.splice(0)) root.unmount(); });
  _resetChatsForTests();
  vi.unstubAllGlobals();
});

describe('Conta atual na folha de ajustes real', () => {
  it('linha no topo mostra nome amigável e as duas cotas do servidor da conversa', async () => {
    state.rows[0] = { ...state.rows[0], provider: 'claude', conta: 'claude:/home/test/.claude-team' };
    api.credentials.mockResolvedValue([{ id: 'claude:/home/test/.claude-team', nome: 'Equipe', tipo: 'claude',
      login: { estado: 'ok', loggedIn: true }, cota: { estado: 'lida', janelas: [{ rotulo: '5h', pct: 36 }, { rotulo: '7d', pct: 67 }] } }]);
    const { container } = await mount('claude');
    const account = container.querySelector('[aria-label="accounts_switch_title"]');
    expect(account?.textContent).toContain('Equipe');
    expect(account?.textContent).toContain('5h 36%');
    expect(account?.textContent).toContain('7d 67%');
    expect(account?.getAttribute('aria-valuetext')).toContain('5h 36%');
    expect(account?.getAttribute('aria-valuetext')).toContain('7d 67%');
    expect(api.credentials).toHaveBeenCalledWith(state.servers[0]);
  });
  it.each([
    { provider: 'claude', account: { conta: 'claude:/home/test/.claude-route-account' }, label: 'route-account' },
    { provider: 'claude', account: { engine: 'local-gpt', conta: 'chave:local-gpt' }, label: 'local-gpt' },
    { provider: 'codex', account: { codex_account: 'codex-route-account' }, label: 'route-account' },
    { provider: 'codex', account: { codex_home: '/home/test/.codex-route-home' }, label: 'route-home' },
  ])('$provider mostra $label da sessão da rota com outro servidor ativo', async ({ provider, account, label }) => {
    state.rows = [
      { name: 'sess', serverId: 'b', state: 'idle', provider: 'claude', conta: 'claude:/home/test/.claude-active-account' },
      { name: 'another', serverId: 'a', state: 'idle', provider: 'claude', conta: 'claude:/home/test/.claude-other-session' },
      { name: 'sess', serverId: 'a', state: 'idle', provider: provider as SessionInfo['provider'], ...account },
    ];
    const { container } = await mount(provider);
    expect(container.textContent).toContain('cota_conta_ativa');
    expect(container.textContent).toContain(label);
    expect(container.textContent).not.toContain('active-account');
    expect(container.textContent).not.toContain('other-session');
  });

  it('sem conta reconhecida não inventa conta do servidor ativo', async () => {
    state.rows = [{ name: 'sess', serverId: 'b', state: 'idle', conta: 'claude:/home/test/.claude-active-account' }];
    const { container } = await mount('claude');
    expect(container.textContent).not.toContain('active-account');
  });
});

describe('Troca de conta na folha de ajustes real', () => {
  it.each(['mode', 'account'] as const)('%s abre a mesma folha sem selecionar e troca fecha antes de navegar para a rota', async (which) => {
    state.rows[0].provider = 'claude';
    state.rows[0].conta = 'claude:/home/test/.claude-route-account';
    const { container, refresh } = await mount('claude', 'a', { which, n: 1 });
    expect(container.querySelector('[aria-label="native_close"]')).not.toBeNull();
    expect(container.querySelector('[role="tab"][aria-label="native_usage_card_session"]')?.getAttribute('aria-selected')).toBe('true');
    const action = container.querySelector<HTMLButtonElement>('[aria-label="accounts_switch_title"]');
    expect(action).not.toBeNull();
    expect(parseFloat(action!.style.minHeight)).toBeGreaterThanOrEqual(44);
    expect(api.modelWrite).not.toHaveBeenCalled();
    expect(api.modeWrite).not.toHaveBeenCalled();
    expect(api.tier).not.toHaveBeenCalled();
    await act(async () => action!.click());
    expect(container.querySelector('[aria-label="native_close"]')).toBeNull();
    expect(routing.push).not.toHaveBeenCalled();
    await act(async () => nativeSheet.finishDismiss!());
    expect(routing.push).toHaveBeenCalledExactlyOnceWith('/s/a/sess/account');
    await refresh();
    expect(container.querySelector('[aria-label="native_close"]')).toBeNull();
  });

  it.each(['invite', 'removed', 'inviteEnded', 'missing'] as const)('não oferece troca em servidor %s, mesmo com outro próprio ativo', async (kind) => {
    if (kind === 'missing') state.servers = state.servers.slice(1);
    else state.servers[0] = { ...state.servers[0], [kind]: true };
    const { container } = await mount('claude', 'a', { which: 'account', n: 1 });
    expect(container.querySelector('[aria-label="native_close"]')).not.toBeNull();
    expect(container.querySelector('[aria-label="accounts_switch_title"]')).toBeNull();
    expect(routing.push).not.toHaveBeenCalled();
  });

  it('Codex mostra conta atual sem oferecer troca', async () => {
    state.rows[0].provider = 'codex';
    state.rows[0].codex_account = 'route-codex-account';
    const { container } = await mount('codex', 'a', { which: 'account', n: 1 });
    expect(container.textContent).toContain('route-codex-account');
    expect(container.querySelector('[aria-label="accounts_switch_title"]')).toBeNull();
    expect(routing.push).not.toHaveBeenCalled();
  });
});

describe('Modelos na folha de ajustes real', () => {
  it('oferece o Fable 5.2 pelo identificador completo do catálogo e conserva a escolha', async () => {
    state.rows[0] = { ...state.rows[0], provider: 'claude', model: 'claude-opus-5-5' };
    api.models.mockResolvedValue({ kind: 'claude', engine: null, models: [
      { id: 'opus', name: 'Opus 5.5' },
      { id: 'claude-fable-5-2', name: 'Fable 5.2' },
      { id: 'fable', name: 'Fable 5.1' },
    ] });
    api.modelWrite.mockResolvedValue({ ok: true, scope: 'session', result: 'Set model to Fable 5.2' });
    const { container } = await mount('claude', 'a', { which: 'model', n: 1 });
    const fable = [...container.querySelectorAll<HTMLButtonElement>('[role="radio"]')]
      .find((button) => button.textContent?.includes('Fable 5.2'));
    expect(fable).toBeDefined();
    await act(async () => fable!.click());
    expect(api.modelWrite).toHaveBeenCalledWith('sess', { model: 'claude-fable-5-2', scope: 'session' }, state.servers[0]);
    expect(container.querySelector('[aria-label="composer_session_settings"]')?.textContent).toContain('Fable 5.2');
  });

  it.each([
    { key: 'id', items: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }] },
    { key: 'hint', items: [{ label: 'Modelo', hint: 'pi/a' }, { label: 'Modelo', hint: 'omp/b' }] },
    { key: 'label', items: [{ label: 'A' }, { label: 'B' }] },
    { key: 'empty-id', items: [{ id: '', label: 'criar_padrao' }, { id: 'b', label: 'B' }] },
  ])('clique na mesma linha após reordenar seleciona o modelo original por $key', async ({ items }) => {
    const container = document.createElement('div');
    const root = createRoot(container);
    roots.push(root);
    const select = vi.fn();
    const render = (catalog: typeof items) => createElement(SessionSettingsControl, {
      open: true, onOpen: () => {}, onClose: () => {},
      model: { display: 'modelo', items: catalog, select },
      effort: { hidden: true, items: [], select: async () => {} },
      fast: { hidden: true, tier: 'default', available: false, choose: async () => {} },
    });
    await act(async () => root.render(render(items)));
    const firstModel = container.querySelector<HTMLButtonElement>('[role="radio"]')!;
    await act(async () => root.render(render([items[1], items[0]])));
    await act(async () => firstModel.click());
    expect(select).toHaveBeenCalledExactlyOnceWith(items[0]);
  });
});

describe('Fast na folha de ajustes real', () => {
  it('chip do chat conserva modelo, contexto e esforço e mostra Fast só quando ligado', async () => {
    state.statusLine = '🤖 GPT·1M (high) │ 💬 1k/1k 4k/1M';
    const { container, close } = await mount();
    const chip = () => container.querySelector<HTMLButtonElement>('[aria-label="composer_session_settings"]')!;
    expect(chip().textContent).toBe('GPT · 1M high');
    await act(async () => toggle(container).click());
    await close();
    expect(chip().textContent).toBe('GPT · 1M high · native_ctl_fast');
    expect(api.tier).toHaveBeenCalledExactlyOnceWith('sess', 'priority', state.servers[0]);
  });

  it.each([
    { tiers: [{ id: 'priority', hidden: false }], available: true },
    { tiers: [{ id: 'priority' }], available: true },
    { tiers: [{ id: 'priority', hidden: true }], available: false },
    { tiers: [], available: false },
    { tiers: [{ id: 'default' }], available: false },
  ])('Codex só liga com priority visível: $tiers', async ({ tiers, available }) => {
    api.codex.mockResolvedValue(codex(tiers));
    const { container } = await mount();
    expect(checked(container)).toBe('false');
    expect(toggle(container).disabled).toBe(!available);
    expect(container.textContent).toContain(available ? 'native_ctl_speed_normal_hint' : 'native_ctl_fast_unavailable');
    await act(async () => toggle(container).click());
    expect(api.tier).toHaveBeenCalledTimes(available ? 1 : 0);
  });

  it('não usa o suporte de outro modelo nem um catálogo sem serviceTiers', async () => {
    const catalog = codex();
    delete catalog.models[0].serviceTiers;
    catalog.models.push({ ...codex().models[0], model: 'gpt-other' });
    api.codex.mockResolvedValue(catalog);
    const { container } = await mount();
    expect(toggle(container).disabled).toBe(true);
  });

  it.each([null, 'future'])('tier desconhecido %s não habilita Fast no Codex', async (tier) => {
    api.codex.mockResolvedValue(codex([{ id: 'priority' }], tier));
    const { container } = await mount();
    expect(toggle(container).disabled).toBe(true);
    expect(checked(container)).toBe('false');
  });

  it('priority já ligado permite desligar mesmo sem suporte no catálogo', async () => {
    api.codex.mockResolvedValue(codex([], 'priority'));
    api.tier.mockResolvedValue({ ok: true, service_tier: 'default' });
    const { container } = await mount();
    expect(checked(container)).toBe('true');
    expect(toggle(container).disabled).toBe(false);
    await act(async () => toggle(container).click());
    expect(api.tier).toHaveBeenCalledExactlyOnceWith('sess', 'default', state.servers[0]);
    expect(checked(container)).toBe('false');
    expect(toggle(container).disabled).toBe(true);
  });

  it.each([
    { list: 'priority', live: null, on: true, available: true },
    { list: 'priority', live: 'default', on: false, available: false },
    { list: 'default', live: 'priority', on: true, available: true },
    { list: 'priority', live: 'future', on: false, available: false },
  ])('SSE vence lista, e lista vence catálogo: $list / $live', async ({ list, live, on, available }) => {
    api.codex.mockResolvedValue(codex([], 'default'));
    state.rows[0].codex_service_tier = list;
    state.stateEvent = { session: 'sess', state: 'idle', codex_service_tier: live };
    const { container } = await mount();
    expect(checked(container)).toBe(String(on));
    expect(toggle(container).disabled).toBe(!available);
  });

  it('Claude aceita só o motor correspondente, com suporte global e no modelo sem [1m]', async () => {
    state.rows[0].engine = 'local-gpt';
    const { container } = await mount('claude');
    expect(toggle(container).disabled).toBe(false);
    await act(async () => toggle(container).click());
    expect(api.tier).toHaveBeenCalledExactlyOnceWith('sess', 'priority', state.servers[0]);
    expect(checked(container)).toBe('true');
  });

  it.each(['anthropic', 'wrong-engine', 'global-disabled', 'claude-catalog'])('Claude não oferece Fast: %s', async (kind) => {
    state.rows[0].engine = kind === 'anthropic' ? null : 'local-gpt';
    const catalog = engine();
    if (kind === 'wrong-engine') catalog.engine = 'other';
    if (kind === 'global-disabled') catalog.supports_fast = false;
    if (kind === 'claude-catalog') catalog.kind = 'claude';
    api.models.mockResolvedValue(catalog);
    const { container } = await mount('claude');
    expect(toggle(container)).toBeNull();
    expect(api.tier).not.toHaveBeenCalled();
  });

  it('motor que deixou de oferecer Fast com a sessão nele: só o Normal, e ele volta', async () => {
    state.rows[0].engine = 'local-gpt';
    const catalog = { ...engine(), supports_fast: false, current: { model: 'gpt-current[1m]', service_tier: 'priority' } };
    catalog.models = [{ id: 'gpt-current', supports_fast: false }];
    api.models.mockResolvedValue(catalog);
    const { container } = await mount('claude');
    expect(checked(container)).toBe('true');
    expect(fastButton(container).disabled).toBe(true);
    const normal = speedButton(container, 'native_ctl_speed_normal');
    expect(normal.disabled).toBe(false);
    api.tier.mockResolvedValue({ ok: true, service_tier: 'default' });
    await act(async () => normal.click());
    expect(api.tier).toHaveBeenCalledExactlyOnceWith('sess', 'default', state.servers[0]);
  });

  it('modelo atual sem Fast num motor com Fast e a sessão em Fast: só o Normal, e ele volta', async () => {
    state.rows[0].engine = 'local-gpt';
    const catalog = { ...engine(), current: { model: 'gpt-current[1m]', service_tier: 'priority' } };
    catalog.models = [{ id: 'gpt-current', supports_fast: false }];
    api.models.mockResolvedValue(catalog);
    const { container } = await mount('claude');
    expect(fastButton(container).disabled).toBe(true);
    const normal = speedButton(container, 'native_ctl_speed_normal');
    expect(normal.disabled).toBe(false);
    api.tier.mockResolvedValue({ ok: true, service_tier: 'default' });
    await act(async () => normal.click());
    expect(api.tier).toHaveBeenCalledExactlyOnceWith('sess', 'default', state.servers[0]);
  });

  it.each(['model-disabled', 'unknown-tier'])('Claude mantém o interruptor indisponível: %s', async (kind) => {
    state.rows[0].engine = 'local-gpt';
    const catalog = engine();
    if (kind === 'model-disabled') catalog.models[0].supports_fast = false;
    else catalog.current!.service_tier = 'future';
    api.models.mockResolvedValue(catalog);
    const { container } = await mount('claude');
    expect(toggle(container).disabled).toBe(true);
    expect(container.textContent).toContain('native_ctl_fast_unavailable');
  });

  it('reabrir Claude relê o tier externo sem usar o catálogo cacheado dos outros controles', async () => {
    state.rows[0].engine = 'local-gpt';
    const cached = engine();
    let live = engine();
    api.models.mockImplementation((_name: string, _server: Server, options?: { fresh?: boolean }) =>
      Promise.resolve(options?.fresh ? live : cached));
    const { container, open, close } = await mount('claude');
    expect(checked(container)).toBe('false');
    await close();
    live = { ...engine(), current: { model: 'gpt-current[1m]', service_tier: 'priority' } };
    await open();
    expect(checked(container)).toBe('true');
    expect(api.models.mock.calls.filter((call) => call[2]?.fresh)).toHaveLength(2);
    expect(api.models.mock.calls.filter((call) => call[2]?.fresh).every((call) => call[1] === state.servers[0])).toBe(true);
    api.tier.mockResolvedValue({ ok: true, service_tier: 'default' });
    await act(async () => toggle(container).click());
    expect(api.tier).toHaveBeenCalledExactlyOnceWith('sess', 'default', state.servers[0]);
  });

  it('reabrir substitui uma leitura Claude ainda em voo e ignora sua resposta antiga', async () => {
    state.rows[0].engine = 'local-gpt';
    const old = deferred<ModelOptionsResponse>();
    const current = { ...engine(), current: { model: 'gpt-current[1m]', service_tier: 'priority' } };
    let freshReads = 0;
    api.models.mockImplementation((_name: string, _server: Server, options?: { fresh?: boolean }) =>
      options?.fresh ? (++freshReads === 1 ? old.promise : Promise.resolve(current)) : Promise.resolve(engine()));
    const { container, open, close } = await mount('claude');
    await close();
    await open();
    expect(freshReads).toBe(2);
    expect(checked(container)).toBe('true');
    await act(async () => old.resolve(engine()));
    expect(checked(container)).toBe('true');
  });

  it.each([
    ['codex', 'status'], ['codex', 'list'], ['claude', 'status'], ['claude', 'list'],
  ])('%s atualiza o suporte ao mudar modelo pela %s, sem reler por tokens', async (provider, source) => {
    state.rows[0].engine = provider === 'claude' ? 'local-gpt' : null;
    state.rows[0].model = 'gpt-current';
    const { container, refresh } = await mount(provider);
    expect(toggle(container).disabled).toBe(false);
    const reads = provider === 'codex' ? api.codex : api.models;
    reads.mockResolvedValue(unsupportedModel(provider));
    const before = reads.mock.calls.length;
    if (source === 'list') state.rows[0].model = 'gpt-next';
    else state.statusLine = '🤖 gpt-next │ 💬 1k/1k 4k/200k';
    await refresh();
    expect(reads.mock.calls.length).toBeGreaterThan(before);
    expect(toggle(container).disabled).toBe(true);
    expect(container.textContent).toContain('native_ctl_fast_unavailable');
    const afterModelChange = reads.mock.calls.length;
    state.statusLine = '🤖 gpt-next │ 💬 1k/1k 5k/200k';
    await refresh();
    expect(reads).toHaveBeenCalledTimes(afterModelChange);
    await act(async () => toggle(container).click());
    expect(api.tier).not.toHaveBeenCalled();
  });

  it.each(['codex', 'claude'])('%s ignora catálogo anterior se o modelo muda durante a leitura', async (provider) => {
    state.rows[0].engine = provider === 'claude' ? 'local-gpt' : null;
    state.rows[0].model = 'gpt-current';
    const old = deferred<CodexModelsResponse | ModelOptionsResponse>();
    const reads = provider === 'codex' ? api.codex : api.models;
    reads.mockReturnValue(old.promise);
    const { container, refresh } = await mount(provider);
    expect(container.querySelector('[role="progressbar"]')).not.toBeNull();
    reads.mockResolvedValue(unsupportedModel(provider));
    state.rows[0].model = 'gpt-next';
    await refresh();
    expect(toggle(container).disabled).toBe(true);
    await act(async () => old.resolve(provider === 'codex' ? codex() : engine()));
    expect(toggle(container).disabled).toBe(true);
    expect(container.textContent).toContain('native_ctl_fast_unavailable');
  });

  it('carregamento e falha da leitura ficam visíveis, com nova tentativa', async () => {
    const read = deferred<CodexModelsResponse>();
    api.codex.mockReturnValue(read.promise);
    const { container } = await mount();
    expect(container.querySelector('[role="progressbar"]')).not.toBeNull();
    expect(toggle(container)).toBeNull();
    await act(async () => read.reject(new Error('catalog-offline')));
    expect(container.textContent).toContain('catalog-offline');
    api.codex.mockResolvedValue(codex());
    const retries = [...container.querySelectorAll('button')].filter((b) => b.textContent === 'native_ctl_retry');
    await act(async () => retries.at(-1)!.click());
    expect(toggle(container).disabled).toBe(false);
  });

  it('usa o servidor A da rota com B ativo, bloqueia reentrada e espera o ACK', async () => {
    const request = deferred<{ ok: boolean; service_tier: string }>();
    api.tier.mockReturnValue(request.promise);
    state.stateEvent = { session: 'sess', state: 'idle', codex_service_tier: 'default' };
    const { container, refresh } = await mount();
    expect(api.codex.mock.calls.every((call) => call[0] === 'sess' && call[1] === state.servers[0])).toBe(true);
    act(() => { toggle(container).click(); toggle(container).click(); });
    expect(api.tier).toHaveBeenCalledExactlyOnceWith('sess', 'priority', state.servers[0]);
    expect(checked(container)).toBe('false');
    expect(toggle(container).disabled).toBe(true);
    expect(container.querySelector('[role="progressbar"]')).not.toBeNull();
    await act(async () => request.resolve({ ok: true, service_tier: 'priority' }));
    expect(checked(container)).toBe('true');
    expect(toggle(container).disabled).toBe(false);
    state.stateEvent = { session: 'sess', state: 'idle', codex_service_tier: 'priority' };
    await refresh();
    state.stateEvent = { session: 'sess', state: 'idle', codex_service_tier: 'default' };
    await refresh();
    expect(checked(container)).toBe('false');
  });

  it.each([false, true])('ACK atrasado não vence SSE que já avançou (volta antes: %s)', async (returnsBeforeAck) => {
    const request = deferred<{ ok: boolean; service_tier: string }>();
    api.tier.mockReturnValue(request.promise);
    state.stateEvent = { session: 'sess', state: 'idle', codex_service_tier: 'default' };
    const { container, refresh } = await mount();
    act(() => toggle(container).click());
    state.stateEvent = { session: 'sess', state: 'idle', codex_service_tier: 'priority' };
    await refresh();
    if (returnsBeforeAck) {
      state.stateEvent = { session: 'sess', state: 'idle', codex_service_tier: 'default' };
      await refresh();
    }
    await act(async () => request.resolve({ ok: true, service_tier: 'priority' }));
    if (!returnsBeforeAck) {
      state.stateEvent = { session: 'sess', state: 'idle', codex_service_tier: 'default' };
      await refresh();
    }
    expect(checked(container)).toBe('false');
  });

  it('falha da gravação mantém o switch e mostra a causa', async () => {
    api.tier.mockRejectedValue(new Error('tier-offline'));
    const { container } = await mount();
    await act(async () => toggle(container).click());
    expect(checked(container)).toBe('false');
    expect(toggle(container).disabled).toBe(false);
    expect(container.querySelector('[role="alert"]')?.textContent).toBe('tier-offline');
  });

  it.each([
    { ok: false, service_tier: 'priority' }, { ok: true, service_tier: 'future' }, { ok: true }, null,
  ])('não pinta sucesso com ACK inválido: %j', async (response) => {
    api.tier.mockResolvedValue(response);
    const { container } = await mount();
    await act(async () => toggle(container).click());
    expect(checked(container)).toBe('false');
    expect(container.querySelector('[role="alert"]')?.textContent).toBe('native_invalid_response');
  });

  it('Codex Pro oferece Ultra Fast, grava ultrafast e mostra no chip', async () => {
    api.codex.mockResolvedValue(codex([{ id: 'priority' }, { id: 'ultrafast' }]));
    api.tier.mockResolvedValue({ ok: true, service_tier: 'ultrafast' });
    const { container, close } = await mount();
    expect(ultraButton(container).disabled).toBe(false);
    expect(container.textContent).not.toContain('native_ctl_speed_ultrafast_unavailable');
    await act(async () => ultraButton(container).click());
    expect(api.tier).toHaveBeenCalledExactlyOnceWith('sess', 'ultrafast', state.servers[0]);
    expect(ultraButton(container).getAttribute('aria-checked')).toBe('true');
    expect(checked(container)).toBe('false');
    expect(container.textContent).toContain('native_ctl_speed_ultrafast_hint');
    await close();
    expect(container.querySelector('[aria-label="composer_session_settings"]')?.textContent).toContain('native_ctl_speed_ultrafast');
  });

  it('sem ultrafast no catálogo o Ultra Fast fica indisponível e explica', async () => {
    const { container } = await mount();
    expect(ultraButton(container).disabled).toBe(true);
    expect(container.textContent).toContain('native_ctl_speed_ultrafast_unavailable');
    await act(async () => ultraButton(container).click());
    expect(api.tier).not.toHaveBeenCalled();
  });

  it('ACK de outro tier mostra o que o servidor confirmou, não o pedido', async () => {
    api.codex.mockResolvedValue(codex([{ id: 'priority' }, { id: 'ultrafast' }]));
    const { container } = await mount();
    await act(async () => ultraButton(container).click());
    expect(ultraButton(container).getAttribute('aria-checked')).toBe('false');
    expect(fastButton(container).getAttribute('aria-checked')).toBe('true');
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it.each([true, false])('Claude com motor segue supports_ultrafast do modelo (%s)', async (supports) => {
    state.rows[0].engine = 'local-gpt';
    const catalog = engine();
    catalog.supports_ultrafast = supports;
    catalog.models[0].supports_ultrafast = supports;
    api.models.mockResolvedValue(catalog);
    api.tier.mockResolvedValue({ ok: true, service_tier: 'ultrafast' });
    const { container } = await mount('claude');
    expect(ultraButton(container).disabled).toBe(!supports);
    await act(async () => ultraButton(container).click());
    expect(api.tier).toHaveBeenCalledTimes(supports ? 1 : 0);
    if (supports) expect(api.tier).toHaveBeenCalledWith('sess', 'ultrafast', state.servers[0]);
  });

  it('sem A na lista não consulta nem grava no B ativo', async () => {
    state.servers = state.servers.filter((s) => s.id === 'b');
    const { container } = await mount();
    expect(container.textContent).toContain('servidor_nao_existe');
    expect(api.codex).not.toHaveBeenCalled();
    expect(api.tier).not.toHaveBeenCalled();
  });
});
