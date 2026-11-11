// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CodexModelsResponse, ModelOptionsResponse, Server, SessionInfo, StateEvent } from '@hangar/core';
import { SessionSettingsButton, SessionSettingsControl } from '../../chat/SessionSettings';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const routing = vi.hoisted(() => ({ push: vi.fn() }));
vi.mock('expo-router', () => ({ useRouter: () => routing }));
const api = vi.hoisted(() => ({ credentials: vi.fn(), codex: vi.fn(), models: vi.fn(), tier: vi.fn(), permissions: vi.fn(), modelWrite: vi.fn(), modeWrite: vi.fn() }));
const state = vi.hoisted(() => ({
  servers: [] as Server[], activeId: 'b',
  rows: [] as (SessionInfo & { serverId: string })[],
  stateEvent: null as StateEvent | null, statusLine: null as string | null,
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  getCodexModels: api.codex, getModelOptions: api.models, setServiceTier: api.tier, getPermissionModes: api.permissions,
  getCredentialsForServer: api.credentials,
  setModelEffort: api.modelWrite, setEngineModel: api.modelWrite, setCodexModel: api.modelWrite,
  setPermissionMode: api.modeWrite, setCodexMode: api.modeWrite,
}));
vi.mock('../../stores/servers', () => ({ useServers: (select: (s: typeof state) => unknown) => select(state) }));
vi.mock('../../stores/sessions', () => ({ useSessions: (select: (s: typeof state) => unknown) => select(state) }));
vi.mock('../../stores/chat', () => ({ chatStore: () => ({ use: (select: (s: typeof state) => unknown) => select(state) }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/AnchoredPanel', () => ({
  AnchoredPanel: ({ open, children, onClose }: { open: boolean; children: ReactNode; onClose: () => void }) => open
    ? createElement('div', null, children, createElement('button', { onClick: onClose, 'aria-label': 'dismiss-settings' })) : null,
}));
vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(),
  Pressable: ({ accessibilityValue, ...props }: Record<string, any>) => createElement('button', {
    'aria-label': props.accessibilityLabel, 'aria-valuetext': accessibilityValue?.text,
    role: props.accessibilityRole, disabled: props.disabled, onClick: props.onPress,
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
    + 'native_invalid_response servidor_nao_existe chat_servidor_removido cota_conta_ativa accounts_switch_title conta_padrao'
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
async function mount(provider = 'codex', serverId = 'a', openRequest?: { which: 'model' | 'effort' | 'mode' | 'account'; n: number }) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  const element = () => createElement(SessionSettingsButton, { serverId, name: 'sess', provider, headless: false, openRequest });
  const open = async () => { await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="composer_session_settings"]')!.click()); };
  const close = async () => { await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="dismiss-settings"]')!.click()); };
  await act(async () => root.render(element()));
  if (!openRequest) {
    expect(container.querySelector('[role="switch"]')).toBeNull();
    await open();
  }
  return { container, open, close, refresh: async () => { await act(async () => root.render(element())); } };
}
const toggle = (container: HTMLElement) => container.querySelector<HTMLButtonElement>('[role="switch"]')!;
const checked = (container: HTMLElement) => toggle(container).getAttribute('aria-checked');

beforeEach(() => {
  vi.resetAllMocks();
  api.credentials.mockResolvedValue([]);
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
});
afterEach(() => { act(() => { for (const root of roots.splice(0)) root.unmount(); }); });

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
    expect(container.querySelector('[role="heading"], [role="header"]')?.textContent).toBe('cota_conta_ativa');
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
    expect(container.querySelector('[aria-label="dismiss-settings"]')).not.toBeNull();
    expect(container.textContent).toContain('native_ctl_model_title');
    const action = container.querySelector<HTMLButtonElement>('[aria-label="accounts_switch_title"]');
    expect(action).not.toBeNull();
    expect(parseFloat(action!.style.minHeight)).toBeGreaterThanOrEqual(44);
    expect(api.modelWrite).not.toHaveBeenCalled();
    expect(api.modeWrite).not.toHaveBeenCalled();
    expect(api.tier).not.toHaveBeenCalled();
    await act(async () => action!.click());
    expect(container.querySelector('[aria-label="dismiss-settings"]')).toBeNull();
    expect(routing.push).toHaveBeenCalledExactlyOnceWith('/s/a/sess/account');
    await refresh();
    expect(container.querySelector('[aria-label="dismiss-settings"]')).toBeNull();
  });

  it.each(['invite', 'removed', 'inviteEnded', 'missing'] as const)('não oferece troca em servidor %s, mesmo com outro próprio ativo', async (kind) => {
    if (kind === 'missing') state.servers = state.servers.slice(1);
    else state.servers[0] = { ...state.servers[0], [kind]: true };
    const { container } = await mount('claude', 'a', { which: 'account', n: 1 });
    expect(container.querySelector('[aria-label="dismiss-settings"]')).not.toBeNull();
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
      fast: { hidden: true, on: false, available: false, toggle: async () => {} },
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
    expect(container.textContent).toContain(available ? 'native_ctl_fast_hint' : 'native_ctl_fast_unavailable');
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
    expect(reads).toHaveBeenCalledTimes(before + 1);
    expect(toggle(container).disabled).toBe(true);
    expect(container.textContent).toContain('native_ctl_fast_unavailable');
    state.statusLine = '🤖 gpt-next │ 💬 1k/1k 5k/200k';
    await refresh();
    expect(reads).toHaveBeenCalledTimes(before + 1);
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

  it('sem A na lista não consulta nem grava no B ativo', async () => {
    state.servers = state.servers.filter((s) => s.id === 'b');
    const { container } = await mount();
    expect(container.textContent).toContain('servidor_nao_existe');
    expect(api.codex).not.toHaveBeenCalled();
    expect(api.tier).not.toHaveBeenCalled();
  });
});
