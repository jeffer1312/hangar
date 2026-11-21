// @vitest-environment happy-dom
import { act, createElement, useLayoutEffect } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { _limparEsfriamentoParaTestes, _resetRotasForTests, configureApi, type AggSession, type SessionInfo, type Server, type StateEvent } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const boundary = vi.hoisted(() => ({ params: { server: 'own', name: 'same-conversation' }, alert: vi.fn(), replace: vi.fn(), release: vi.fn(), back: vi.fn(), canGoBack: false, options: vi.fn() }));
vi.mock('expo-router', () => ({
  useLocalSearchParams: () => ({ ...boundary.params }),
  useRouter: () => ({ replace: boundary.replace, back: boundary.back, canGoBack: () => boundary.canGoBack }),
  Stack: { Screen: ({ options }: { options: unknown }) => {
    useLayoutEffect(() => { boundary.options(options); }, [options]);
    return null;
  } },
}));
vi.mock('react-native-unistyles', async original => {
  const actual = await original<typeof import('react-native-unistyles')>();
  return { ...actual, useUnistyles: () => {
    const value = actual.useUnistyles();
    return { ...value, theme: new Proxy(value.theme, {}) };
  } };
});
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {} }));
vi.mock('../stores/sessions', async () => {
  const { create } = await import('zustand');
  return { useSessions: create(() => ({ rows: [], byServerRecord: {}, byServer: [], retain: () => boundary.release })) };
});
vi.mock('../stores/chat', async () => {
  const { create } = await import('zustand');
  const stores = new Map();
  return { chatStore: (server: string, name: string) => {
    const key = `${server}:${name}`;
    if (!stores.has(key)) stores.set(key, { use: create(() => ({ stateEvent: null })) });
    return stores.get(key);
  } };
});
vi.mock('react-native', async original => {
  const actual = await original<typeof import('react-native')>();
  return { ...actual, Platform: { ...actual.Platform, OS: 'ios' }, View: ({ accessibilityValue, collapsable, ...props }: Record<string, any>) => {
    const viewProps = { ...props, 'aria-valuenow': accessibilityValue?.now, 'data-collapsable': collapsable };
    return createElement(actual.View, viewProps);
  }, Alert: { alert: boundary.alert },
    Pressable: ({ accessibilityState, accessibilityValue, ...props }: Record<string, any>) => createElement(actual.Pressable, { ...props, 'aria-valuetext': accessibilityValue?.text }),
    ScrollView: ({ contentInsetAdjustmentBehavior, contentContainerStyle, ...props }: Record<string, any>) => {
      const viewProps = { ...props, style: props.style, 'data-inset-adjustment': contentInsetAdjustmentBehavior };
      return createElement(actual.View, viewProps);
    },
    Text: ({ selectable, ...props }: Record<string, unknown>) => createElement(actual.Text, props),
  };
});

import AccountRoute from '../../app/s/[server]/[name]/account';
import { useServers } from '../stores/servers';
import { useSessions } from '../stores/sessions';
import { chatStore } from '../stores/chat';
import * as m from '../paraglide/messages';

const own: Server = { id: 'own', label: 'owner-server', baseUrl: 'https://owner.test', token: 'owner-token' };
const other: Server = { id: 'other', label: 'active-server', baseUrl: 'https://active.test', token: 'active-token' };
type AccountSession = SessionInfo & { config_dir?: string; engine_account?: string; lifecycle_id?: string };
const source: AccountSession = { name: 'same-conversation', provider: 'claude', state: 'idle', conta: 'claude:/home/u/.claude-alpha', jsonl: '/history/life-a.jsonl' };
const targets = [
  { path: '/home/u/.claude-beta', label: 'beta', pct: 20, low: false, full: false },
  { path: '/home/u/.claude-full', label: 'full', pct: 100, low: true, full: true },
  { path: '/home/u/.claude-unknown', label: 'unknown', pct: null, low: false, full: false },
];
const roots: Root[] = [];
let network: ReturnType<typeof vi.fn>;
let listReply: () => Promise<Response>;
let sessionReply: () => Promise<Response>;
let postReply: () => Promise<Response>;
let engineReply: () => Promise<Response>;
let credentialsReply: () => Promise<Response>;
const engineAccounts = [
  { account: 'one', credential_id: 'codex:/home/u/.codex-one', email: 'one@test', label: 'ChatGPT one' },
  { account: 'two', credential_id: 'codex:/home/u/.codex-two', email: 'two@test', label: 'ChatGPT two' },
  { account: 'full', credential_id: 'codex:/home/u/.codex-full', email: 'full@test', label: 'ChatGPT full' },
  { account: 'low', credential_id: 'codex:/home/u/.codex-low', email: 'low@test', label: 'ChatGPT low' },
  { account: 'off', credential_id: 'codex:/home/u/.codex-off', email: 'off@test', label: 'ChatGPT off' },
  { account: 'unavailable', credential_id: 'codex:/home/u/.codex-unavailable', email: '', label: 'ChatGPT unavailable' },
  { account: '', credential_id: 'codex:/home/u/.codex-bad', email: '', label: 'invalid-account' },
  { account: 'bad', credential_id: 'claude:/home/u/.claude-bad', email: '', label: 'invalid-credential' },
  { account: 'orphan', credential_id: 'codex:/home/u/.codex-orphan', email: '', label: 'missing-credential' },
];
const engineSource: AccountSession = { ...source, engine: 'local-gpt', conta: 'codex:/home/u/.codex-one', engine_account: 'one' };
const credentials = engineAccounts.slice(0, 6).map((a, i) => ({
  id: a.credential_id, tipo: 'codex', nome: a.account, nome_natural: a.account, ativa: false, usos: ['codex'],
  login: { estado: i === 5 ? 'indisponivel' : 'ok', loggedIn: i === 4 ? false : true },
  cota: { estado: i === 1 ? 'indisponivel' : 'lida', janelas: [{ rotulo: '5h', pct: i === 2 ? 99 : i === 3 ? 95 : 17 }] },
}));
const claudeCredential = (path: string, name: string) => ({ id: `claude:${path}`, tipo: 'claude', nome: name, nome_natural: name,
  path, ativa: false, usos: ['claude_code'], login: { estado: 'ok', loggedIn: true } });
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
}
function setSession(session: AccountSession | null = source) {
  const rows = session ? [{ ...session, serverId: own.id, serverLabel: own.label } as AggSession] : [];
  useSessions.setState({ rows, byServerRecord: { own: rows }, byServer: [{ server: own, sessions: rows, loaded: true, error: null }] });
}
const posts = () => network.mock.calls.filter(([, init]) => init?.method === 'POST');
const button = (container: HTMLElement, label: string) => container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`);
async function mount() {
  const container = document.createElement('div');
  const root = createRoot(container); roots.push(root);
  await act(async () => root.render(createElement(AccountRoute)));
  return { container, root };
}
const rerender = (root: Root) => act(async () => root.render(createElement(AccountRoute)));
const click = (container: HTMLElement, label: string) => act(async () => { button(container, label)!.click(); });
const confirm = () => {
  expect(boundary.alert.mock.calls.length, 'confirmação nativa antes da escrita').toBeGreaterThan(0);
  const buttons = boundary.alert.mock.calls.at(-1)![2] as Array<{ onPress?: () => void }>;
  return buttons[1].onPress!;
};

beforeEach(() => {
  _limparEsfriamentoParaTestes(); _resetRotasForTests();
  boundary.params = { server: 'own', name: source.name };
  boundary.alert.mockReset(); boundary.replace.mockReset(); boundary.release.mockReset();
  boundary.back.mockReset(); boundary.options.mockReset(); boundary.canGoBack = false;
  useServers.setState({ servers: [own, other], activeId: other.id, ready: true });
  setSession();
  chatStore(own.id, source.name).use.setState({ stateEvent: null });
  listReply = async () => json(targets);
  sessionReply = async () => json([source]);
  postReply = async () => json({ ok: true, config_dir: targets[0].path });
  engineReply = async () => json({ motores: { 'local-gpt': { cliproxy_accounts: engineAccounts } }, arquivo_corrompido: false, arquivo_caminho: '/engines.json' });
  credentialsReply = async () => json([...credentials, ...targets.map(t => claudeCredential(t.path, t.label)),
    claudeCredential('/home/u/.claude-alpha', 'alpha'), claudeCredential('/home/u/.claude-low', 'low')]);
  network = vi.fn((url: string, init?: RequestInit) => {
    if (url === `https://replacement.test/api/sessions/${source.name}/conta`) return init?.method === 'POST' ? postReply() : listReply();
    if (url === `${own.baseUrl}/api/engines`) return engineReply();
    if (url === `${own.baseUrl}/api/credenciais`) return credentialsReply();
    if (url.startsWith(`${own.baseUrl}/api/sessions/`) && url.endsWith('/conta')) return init?.method === 'POST' ? postReply() : listReply();
    if (url === `${own.baseUrl}/api/sessions`) return sessionReply();
    throw new Error(`unexpected-network: ${url}`);
  });
  vi.stubGlobal('fetch', network);
  configureApi({ getBaseUrl: () => other.baseUrl, getToken: () => other.token, onUnauthorized: () => {}, origin: null,
    createEventSource: () => { throw new Error('unexpected SSE'); } });
});
afterEach(() => {
  act(() => { roots.splice(0).forEach(root => root.unmount()); });
  vi.unstubAllGlobals();
  _limparEsfriamentoParaTestes(); _resetRotasForTests();
});

describe('Folha da conta no iOS', () => {
  it('usa título e fechar locais, sem republicar cabeçalho nativo ao renderizar', async () => {
    const pending = deferred<Response>(); listReply = () => pending.promise;
    const { container, root } = await mount();
    expect(boundary.options.mock.calls.at(-1)?.[0]).toMatchObject({ headerShown: false, contentStyle: { backgroundColor: '#fff' } });
    expect((container.firstElementChild as HTMLElement).style.backgroundColor).toBe('');
    expect(container.querySelector('[role="header"]')?.textContent).toBe(m.accounts_switch_title());
    expect(container.querySelector('[role="header"]')?.parentElement?.dataset.collapsable).toBe('false');
    const scroll = container.querySelector<HTMLElement>('[data-inset-adjustment]')!;
    expect(scroll.dataset.insetAdjustment).toBe('never');
    expect(scroll.style.flex).toBe('1 1 0%');
    expect(container.textContent).toContain(m.comum_carregando());
    await rerender(root);
    expect(boundary.options).toHaveBeenCalledTimes(1);
    expect(button(container, m.native_close())).not.toBeNull();
    await click(container, m.native_close());
    expect(boundary.replace).toHaveBeenCalledWith('/s/own/same-conversation');
    expect(boundary.back).not.toHaveBeenCalled();
    await act(async () => pending.resolve(json(targets)));
    expect(button(container, 'beta')).not.toBeNull();
  });

  it('permite fechar pelo histórico mesmo com erro ao carregar as contas', async () => {
    boundary.canGoBack = true;
    listReply = async () => json({ detail: 'accounts-unavailable' }, 503);
    const { container } = await mount();
    expect(container.textContent).toContain('accounts-unavailable');
    expect(button(container, m.native_close())).not.toBeNull();
    await click(container, m.native_close());
    expect(boundary.back).toHaveBeenCalledOnce();
    expect(boundary.replace).not.toHaveBeenCalled();
  });
});

describe('Contas com identificação e cota completa', () => {
  it('destino sem login confirmado fica selecionável com aviso e no fim', async () => {
    credentialsReply = async () => json([claudeCredential('/home/u/.claude-full', 'full')]);
    const { container } = await mount();
    expect(button(container, 'beta')).not.toBeNull();
    expect(button(container, 'beta')!.disabled).toBe(false);
    expect(container.textContent!.indexOf('.claude-full')).toBeLessThan(container.textContent!.indexOf('.claude-beta'));
    await click(container, 'beta');
    expect(boundary.alert).toHaveBeenCalledTimes(1);
    expect(posts()).toHaveLength(0);
  });
  const credential = (folder: string, pct: number, extra = {}) => ({
    id: `claude:/home/u/${folder}`, tipo: 'claude', nome: 'Ricardo e Brendon', nome_natural: folder,
    path: `/home/u/${folder}`, ativa: false, usos: ['claude_code'],
    login: { estado: 'ok', loggedIn: true, email: 'same@test', plano: 'Max' },
    cota: { estado: 'lida', janelas: [{ rotulo: '5h', pct, reset_ts: 1800000000 }, { rotulo: '7d', pct: 12, reset_ts: 1800086400 }] },
    ...extra,
  });
  it('ordena pela pior janela e distingue homônimas pela pasta, sem descartar nenhuma', async () => {
    listReply = async () => json([
      { path: '/home/u/.claude-02-200', label: 'Ricardo e Brendon', pct: 80, full: false, low: false },
      { path: '/home/u/.claude-claude-200-2', label: 'Ricardo e Brendon', pct: 30, full: false, low: false },
    ]);
    credentialsReply = async () => json([credential('.claude-02-200', 80), credential('.claude-claude-200-2', 30)]);
    const { container } = await mount();
    const content = container.textContent!;
    expect(content).toContain('same@test');
    expect(content).toContain('Max');
    expect(content.indexOf('.claude-claude-200-2')).toBeLessThan(content.indexOf('.claude-02-200'));
    const spoken = container.querySelector('button[aria-valuetext]')?.getAttribute('aria-valuetext');
    expect(spoken).toContain('.claude-claude-200-2');
    expect(spoken).toContain('5h 30%');
    expect(spoken).toContain('7d 12%');
    expect([...container.querySelectorAll('[role="progressbar"]')].map(e => e.getAttribute('aria-valuenow'))).toEqual(['30', '12', '80', '12']);
  });
  it('esconde só login comprovadamente ausente e conserva login que não pôde conferir', async () => {
    listReply = async () => json(['backup', 'uncertain', 'valid'].map((id) => ({ path: `/home/u/.claude-${id}`, label: id, pct: null, full: false, low: false })));
    credentialsReply = async () => json([
      credential('.claude-backup', 0, { login: { estado: 'ok', loggedIn: false } }),
      credential('.claude-uncertain', 0, { login: { estado: 'indisponivel', loggedIn: null, motivo: 'prazo' }, cota: { estado: 'indisponivel', motivo: 'http-429', janelas: [] } }),
      credential('.claude-valid', 0),
    ]);
    const { container } = await mount();
    expect(button(container, 'backup')).toBeNull();
    expect(container.textContent).toContain('.claude-uncertain');
    expect(container.textContent).toContain(m.cota_causa_limite());
    expect(button(container, 'uncertain')!.disabled).toBe(false);
  });
  it('marca a atual no topo com nome amigável e suas duas janelas', async () => {
    credentialsReply = async () => json([credential('.claude-alpha', 90, { nome: 'Conta atual' })]);
    const { container } = await mount();
    expect(container.textContent).toContain('Conta atual');
    expect(container.textContent!.indexOf('Conta atual')).toBeLessThan(container.textContent!.indexOf('beta'));
    expect([...container.querySelectorAll('[role="progressbar"]')].map(e => e.getAttribute('aria-valuenow'))).toContain('90');
    expect(container.textContent).toContain(m.mobile_accounts_quota_reset({ when: 'whatever' }).split('whatever')[0]);
  });
});

describe('C3 vida da sessão', () => {
  it.each([false, true])('alerta antigo recusa nova vida com mesma conta e transcript; ausência intermediária=%s', async absent => {
    setSession({ ...source, lifecycle_id: 't:old-birth' });
    const { container } = await mount();
    await click(container, 'beta'); const accept = confirm();
    if (absent) await act(async () => setSession(null));
    await act(async () => {
      setSession({ ...source, lifecycle_id: 't:new-birth' });
      accept();
    });
    expect(posts()).toHaveLength(0);
  });

  it.each(['origin', 'destination'])('reabertura legítima na %s após dispatch preserva sucesso', async phase => {
    const initial = { ...source, jsonl: '/home/u/.claude-alpha/projects/project-a/life-a.jsonl', lifecycle_id: 't:old-birth' };
    setSession(initial);
    const pending = deferred<Response>(); postReply = () => pending.promise;
    const { container } = await mount();
    await click(container, 'beta'); await act(async () => confirm()());
    const moved = { ...initial, conta: 'claude:/home/u/.claude-beta', jsonl: '/home/u/.claude-beta/projects/project-a/life-a.jsonl', lifecycle_id: 'k:new-key' };
    await act(async () => setSession(phase === 'origin' ? { ...initial, lifecycle_id: 't:reopened-birth' } : moved));
    await act(async () => pending.resolve(json({ ok: true, config_dir: targets[0].path })));
    expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'beta' }));
    await act(async () => setSession(moved));
    expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'beta' }));
  });

  it('motor GPT pode reabrir com outra vida sem perder confirmação da mesma conversa', async () => {
    const initial = { ...engineSource, lifecycle_id: 'k:old-key' }; setSession(initial);
    const pending = deferred<Response>(); postReply = () => pending.promise;
    const { container } = await mount();
    await click(container, 'ChatGPT two'); await act(async () => confirm()());
    await act(async () => setSession({ ...initial, conta: 'codex:/home/u/.codex-two', engine_account: 'two', lifecycle_id: 't:new-birth' }));
    await act(async () => pending.resolve(json({ ok: true })));
    expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'local-gpt · two' }));
  });

  it('erro parcial continua visível depois da reabertura legítima', async () => {
    const initial = { ...source, jsonl: '/home/u/.claude-alpha/projects/project-a/life-a.jsonl', lifecycle_id: 't:old-birth' }; setSession(initial);
    const pending = deferred<Response>(); postReply = () => pending.promise;
    const { container } = await mount();
    await click(container, 'beta'); await act(async () => confirm()());
    await act(async () => pending.resolve(json({ detail: 'terminal-reopen-failed' }, 409)));
    await act(async () => setSession({ ...initial, conta: 'claude:/home/u/.claude-beta', jsonl: '/home/u/.claude-beta/projects/project-a/life-a.jsonl', lifecycle_id: 'k:new-key' }));
    expect(container.textContent).toContain('terminal-reopen-failed');
    expect(container.textContent).not.toContain(m.mobile_session_account_success({ conta: 'beta' }));
  });
});

describe('C2 estado real da lista', () => {
  it.each([
    { loaded: false, error: null },
    { loaded: false, error: 'offline' },
    { loaded: true, error: 'offline' },
  ])('bucket %j não comprova sessão encerrada e permite busca direta', async status => {
    useSessions.setState({ rows: [], byServerRecord: { own: [] }, byServer: [{ server: own, sessions: [], ...status }] });
    const pending = deferred<Response>(); sessionReply = () => pending.promise;
    const { container } = await mount();
    expect(network.mock.calls.some(([url]) => url === `${own.baseUrl}/api/sessions`)).toBe(true);
    expect(container.textContent).not.toContain(m.chat_sessao_encerrada());
    await act(async () => pending.resolve(json([source])));
    expect(container.textContent).toContain('alpha');
    expect(button(container, 'beta')).not.toBeNull();
  });

  it('falha na busca sem lista boa mostra erro e retry recupera a sessão', async () => {
    useSessions.setState({ rows: [], byServerRecord: { own: [] }, byServer: [{ server: own, sessions: [], loaded: false, error: 'offline' }] });
    sessionReply = async () => json({ detail: 'direct-session-offline' }, 503);
    const { container } = await mount();
    expect(container.textContent).toContain('503');
    expect(container.textContent).not.toContain(m.chat_sessao_encerrada());
    sessionReply = async () => json([source]);
    await click(container, m.native_retry());
    expect(container.textContent).toContain('alpha');
    expect(button(container, 'beta')).not.toBeNull();
    expect(container.textContent).not.toContain('503');
  });

  it('queda da lista após mostrar a sessão não vira encerramento', async () => {
    const { container } = await mount();
    sessionReply = async () => json({ detail: 'list-disconnected' }, 503);
    await act(async () => useSessions.setState({ rows: [], byServerRecord: { own: [] }, byServer: [{ server: own, sessions: [], loaded: true, error: 'offline' }] }));
    expect(container.textContent).toContain('503');
    expect(container.textContent).not.toContain(m.chat_sessao_encerrada());
  });

  it('somente lista boa carregada e vazia comprova encerramento sem busca extra', async () => {
    setSession(null);
    const { container } = await mount();
    expect(container.textContent).toContain(m.chat_sessao_encerrada());
    expect(network).not.toHaveBeenCalled();
  });
});

describe('C1 transição real do transcript', () => {
  const paths = [
    { os: 'Linux', from: '/home/u/.claude-alpha/projects/project-a/life-a.jsonl', dir: '/home/u/.claude-beta', to: '/home/u/.claude-beta/projects/project-a/life-a.jsonl' },
    { os: 'Windows', from: 'C:\\Users\\u\\.claude-alpha\\projects\\project-a\\life-a.jsonl', dir: 'C:\\Users\\u\\.claude-beta', to: 'C:\\Users\\u\\.claude-beta\\projects\\project-a\\life-a.jsonl' },
  ];
  for (const pathsCase of paths) {
    it.each(['before', 'after'])(`${pathsCase.os}: preserva sucesso com lista chegando %s do POST`, async when => {
      setSession({ ...source, jsonl: pathsCase.from });
      listReply = async () => json([{ ...targets[0], path: pathsCase.dir }]);
      credentialsReply = async () => json([claudeCredential(pathsCase.dir, 'beta')]);
      const pending = deferred<Response>(); postReply = () => pending.promise;
      const { container } = await mount();
      await click(container, 'beta'); await act(async () => confirm()());
      const moved = { ...source, conta: `claude:${pathsCase.dir}`, jsonl: pathsCase.to };
      if (when === 'before') await act(async () => setSession(moved));
      await act(async () => pending.resolve(json({ ok: true, config_dir: pathsCase.dir })));
      expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'beta' }));
      if (when === 'after') await act(async () => setSession(moved));
      expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'beta' }));
      expect(container.textContent).not.toContain('alpha');
    });
  }

  it.each(['before', 'after'])('409 parcial fica visível quando lista do destino chega %s do erro', async when => {
    const from = '/home/u/.claude-alpha/projects/project-a/life-a.jsonl';
    setSession({ ...source, jsonl: from });
    const pending = deferred<Response>(); postReply = () => pending.promise;
    const { container } = await mount();
    await click(container, 'beta'); await act(async () => confirm()());
    const moved = { ...source, conta: 'claude:/home/u/.claude-beta', jsonl: '/home/u/.claude-beta/projects/project-a/life-a.jsonl' };
    if (when === 'before') await act(async () => setSession(moved));
    await act(async () => pending.resolve(json({ detail: 'moved-but-terminal-failed' }, 409)));
    expect(container.textContent).toContain('moved-but-terminal-failed');
    if (when === 'after') await act(async () => setSession(moved));
    expect(container.textContent).toContain('moved-but-terminal-failed');
    expect(container.textContent).not.toContain(m.mobile_session_account_success({ conta: 'beta' }));
  });

  it.each(['project', 'account', 'transcript', 'engine', 'server'])('não atribui retorno à mudança não esperada: %s', async kind => {
    setSession({ ...source, jsonl: '/home/u/.claude-alpha/projects/project-a/life-a.jsonl' });
    const pending = deferred<Response>(); postReply = () => pending.promise;
    const { container } = await mount();
    await click(container, 'beta'); await act(async () => confirm()());
    const moved = { ...source, conta: 'claude:/home/u/.claude-beta', jsonl: '/home/u/.claude-beta/projects/project-a/life-a.jsonl' };
    if (kind === 'project') moved.jsonl = '/home/u/.claude-beta/projects/project-b/life-a.jsonl';
    if (kind === 'account') moved.conta = 'claude:/home/u/.claude-third';
    if (kind === 'transcript') moved.jsonl = '/home/u/.claude-beta/projects/project-a/life-b.jsonl';
    await act(async () => {
      setSession(kind === 'engine' ? { ...moved, engine: 'another-engine' } : moved);
      if (kind === 'server') useServers.setState({ servers: [{ ...own, token: 'new-token' }, other] });
    });
    await act(async () => pending.resolve(json({ ok: true, config_dir: targets[0].path })));
    expect(container.textContent).not.toContain(m.mobile_session_account_success({ conta: 'beta' }));
  });
});

describe('F4 identidade e proteção', () => {
  it.each(['removed', 'replaced', 'conversation', 'account', 'engine', 'pair', 'session-gone'])('confirmação aberta é invalidada por %s antes de POST', async kind => {
    const { container } = await mount();
    await click(container, 'beta');
    const accept = confirm();
    await act(async () => {
      if (kind === 'removed') useServers.setState({ servers: [other] });
      if (kind === 'replaced') useServers.setState({ servers: [{ ...own, baseUrl: 'https://replacement.test', token: 'replacement-token' }, other] });
      if (kind === 'conversation') setSession({ ...source, jsonl: '/history/life-b.jsonl' });
      if (kind === 'account') setSession({ ...source, conta: 'claude:/home/u/.claude-third' });
      if (kind === 'engine') setSession(engineSource);
      if (kind === 'pair') setSession({ ...source, guest_kind: 'pair' });
      if (kind === 'session-gone') setSession(null);
      accept();
    });
    expect(posts()).toHaveLength(0);
  });

  it('mudar a rota com alerta aberto não escreve na sessão antiga nem na nova', async () => {
    const { container, root } = await mount();
    await click(container, 'beta');
    const accept = confirm();
    boundary.params.name = 'another-conversation';
    await rerender(root);
    await act(async () => accept());
    expect(posts()).toHaveLength(0);
  });

  it('double tap e dupla confirmação fazem um alerta e um POST, sempre para o primeiro alvo', async () => {
    const pending = deferred<Response>(); postReply = () => pending.promise;
    const { container } = await mount();
    await act(async () => {
      button(container, 'beta')!.click();
      button(container, 'unknown')!.click();
    });
    expect(boundary.alert).toHaveBeenCalledTimes(1);
    const accept = confirm();
    await act(async () => { accept(); accept(); });
    expect(posts()).toHaveLength(1);
    expect(JSON.parse(posts()[0][1].body)).toEqual({ config_dir: '/home/u/.claude-beta' });
    expect(button(container, 'unknown')!.disabled).toBe(true);
    await act(async () => pending.resolve(json({ ok: true, config_dir: targets[0].path })));
    expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'beta' }));
  });

  it('cancelar libera outro alvo e callback do alerta cancelado não pode escrever', async () => {
    const { container } = await mount();
    await click(container, 'beta');
    const oldAccept = confirm();
    const cancel = boundary.alert.mock.calls[0][2][0].onPress;
    expect(cancel).toBeTypeOf('function');
    await act(async () => cancel());
    await click(container, 'unknown');
    await act(async () => oldAccept());
    expect(posts()).toHaveLength(0);
    postReply = async () => json({ ok: true, config_dir: '/home/u/.claude-unknown' });
    await act(async () => confirm()());
    expect(posts()).toHaveLength(1);
    expect(JSON.parse(posts()[0][1].body)).toEqual({ config_dir: '/home/u/.claude-unknown' });
  });

  it('busy surgindo após confirmação impede POST e mostra motivo', async () => {
    const { container } = await mount();
    await click(container, 'beta');
    const accept = confirm();
    await act(async () => {
      setSession({ ...source, state: 'working' });
      accept();
    });
    expect(posts()).toHaveLength(0);
    expect(container.textContent).toContain(m.native_usage_card_move_busy());
  });

  it.each([{ invite: true }, { inviteEnded: true }, { removed: true }])('link direto recusa servidor restrito %j sem ler contas', async flags => {
    useServers.setState({ servers: [{ ...own, ...flags }, other] });
    const { container } = await mount();
    expect(container.textContent).toContain(m.chat_servidor_removido());
    expect(network).not.toHaveBeenCalled();
  });

  it.each(['codex', 'pi', 'omp', 'kimi'] as const)('provider %s mostra só conta atual sem destinos nem POST', async provider => {
    setSession({ ...source, provider, conta: provider === 'codex' ? 'codex:/home/u/.codex-main' : null });
    const { container } = await mount();
    expect(container.textContent).toContain(m.mobile_account_current());
    expect(container.textContent).toContain(provider === 'codex' ? 'main' : m.native_invalid_response());
    expect(button(container, 'beta')).toBeNull();
    expect(network).not.toHaveBeenCalled();
  });

  it('retorno antigo da listagem não repovoa outra conversa', async () => {
    const pending = deferred<Response>(); listReply = () => pending.promise;
    const { container, root } = await mount();
    boundary.params.name = 'another-conversation';
    await rerender(root);
    await act(async () => pending.resolve(json(targets)));
    expect(container.textContent).not.toContain('beta');
    expect(posts()).toHaveLength(0);
  });

  it('resposta da sessão buscada por link direto é descartada ao remover servidor', async () => {
    useSessions.setState({ rows: [], byServerRecord: { own: [] }, byServer: [{ server: own, sessions: [], loaded: false, error: null }] });
    const pending = deferred<Response>(); sessionReply = () => pending.promise;
    const { container } = await mount();
    await act(async () => useServers.setState({ servers: [other] }));
    await act(async () => pending.resolve(json([source])));
    expect(container.textContent).toContain(m.chat_servidor_removido());
    expect(container.textContent).not.toContain('alpha');
    expect(network.mock.calls.filter(([url]) => url.endsWith('/conta'))).toHaveLength(0);
  });
});

describe('F5 retorno e acesso direto', () => {
  it('POST confirmado mostra sucesso mesmo quando a lista já publicou a conta de destino', async () => {
    const pending = deferred<Response>(); postReply = () => pending.promise;
    const { container } = await mount();
    await click(container, 'beta');
    await act(async () => confirm()());
    await act(async () => setSession({ ...source, conta: 'claude:/home/u/.claude-beta' }));
    await act(async () => pending.resolve(json({ ok: true, config_dir: targets[0].path })));
    expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'beta' }));
    expect(container.textContent).not.toContain('alpha');
  });

  it('erro de POST antigo não aparece na sessão recriada com mesmo nome', async () => {
    const pending = deferred<Response>(); postReply = () => pending.promise;
    const { container } = await mount();
    await click(container, 'beta');
    await act(async () => confirm()());
    await act(async () => setSession({ ...source, jsonl: '/history/new-life.jsonl' }));
    await act(async () => pending.resolve(json({ detail: 'old-life-error' }, 409)));
    expect(container.textContent).not.toContain('old-life-error');
    expect(container.textContent).not.toContain(m.mobile_session_account_success({ conta: 'beta' }));
  });

  it('link direto de par não lista destinos nem permite POST', async () => {
    setSession({ ...source, guest_kind: 'pair' });
    const { container } = await mount();
    expect(container.textContent).toContain('alpha');
    expect(container.textContent).toContain(m.conta_mesma_so_claude());
    expect(button(container, 'beta')).toBeNull();
    expect(network).not.toHaveBeenCalled();
  });

  it.each([{ invite: true }, { inviteEnded: true }, { removed: true }])('restrição de servidor %j invalida alerta aberto', async flags => {
    const { container } = await mount();
    await click(container, 'beta'); const accept = confirm();
    await act(async () => {
      useServers.setState({ servers: [{ ...own, ...flags }, other] });
      accept();
    });
    expect(posts()).toHaveLength(0);
  });

  it('estado do chat que muda para working antes do callback impede POST', async () => {
    const { container } = await mount();
    await click(container, 'beta'); const accept = confirm();
    await act(async () => {
      chatStore(own.id, source.name).use.setState({ stateEvent: { state: 'working' } as StateEvent });
      accept();
    });
    expect(posts()).toHaveLength(0);
    expect(container.textContent).toContain(m.native_usage_card_move_busy());
  });

  it('trocar token descarta retorno antigo de abertura por link direto', async () => {
    useSessions.setState({ rows: [], byServerRecord: { own: [] }, byServer: [{ server: own, sessions: [], loaded: false, error: null }] });
    const old = deferred<Response>(); const next = deferred<Response>();
    let requests = 0;
    sessionReply = () => requests++ === 0 ? old.promise : next.promise;
    const { container } = await mount();
    await act(async () => useServers.setState({ servers: [{ ...own, token: 'next-token' }, other] }));
    await act(async () => old.resolve(json([source])));
    expect(container.textContent).not.toContain('alpha');
    expect(network.mock.calls.filter(([url]) => url.endsWith('/conta'))).toHaveLength(0);
    await act(async () => next.resolve(json([{ ...source, conta: 'claude:/home/u/.claude-new' }])));
    expect(container.textContent).toContain('new');
    expect(container.textContent).not.toContain('alpha');
    const calls = network.mock.calls.filter(([url]) => url.endsWith('/conta'));
    expect(calls).toHaveLength(1);
    expect(new Headers(calls[0][1].headers).get('Authorization')).toBe('Bearer next-token');
  });
});

describe('F3 motor GPT', () => {
  beforeEach(() => { setSession(engineSource); });

  it('mostra conta resolvida e envia engine_account sem trocar harness nem conta Anthropic', async () => {
    postReply = async () => json({ ok: true });
    const { container } = await mount();
    expect(container.textContent).toContain('local-gpt · one');
    expect(button(container, 'ChatGPT two')).not.toBeNull();
    expect(button(container, 'ChatGPT one')).toBeNull();
    expect(button(container, 'beta')).toBeNull();
    expect(container.textContent).not.toContain('invalid-account');
    expect(container.textContent).not.toContain('invalid-credential');
    expect(container.textContent).not.toContain('missing-credential');
    const choice = button(container, 'ChatGPT two')!;
    expect(choice.textContent).toContain(m.conta_cota_sem_leitura());
    expect(choice.textContent).not.toContain('17%');
    await click(container, 'ChatGPT two');
    expect(posts()).toHaveLength(0);
    await act(async () => confirm()());
    expect(posts()).toHaveLength(1);
    expect(posts()[0][0]).toBe('https://owner.test/api/sessions/same-conversation/conta');
    expect(JSON.parse(posts()[0][1].body)).toEqual({ engine_account: 'two' });
    expect(container.textContent).toContain('local-gpt · two');
    expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'local-gpt · two' }));
  });

  it('bloqueia 99%, oculta login desconectado e permite login incerto; avisa em 95%', async () => {
    const { container } = await mount();
    expect(button(container, 'ChatGPT full')).not.toBeNull();
    expect(button(container, 'ChatGPT full')!.disabled).toBe(true);
    expect(button(container, 'ChatGPT off')).toBeNull();
    expect(button(container, 'ChatGPT unavailable')!.disabled).toBe(false);
    await click(container, 'ChatGPT low');
    expect(boundary.alert).toHaveBeenCalledTimes(1);
    expect(boundary.alert.mock.calls[0][1]).toContain(m.conta_confirmar_acabando({ conta: 'ChatGPT low', pct: 95 }));
    expect(posts()).toHaveLength(0);
  });

  it('conta conectada só no proxy é destino sem cota e sem aviso de login', async () => {
    engineReply = async () => json({ motores: { 'local-gpt': { cliproxy_accounts: [...engineAccounts,
      { account: 'cpa-0123456789ab', credential_id: 'cliproxy:cpa-0123456789ab', email: 'solo@test', label: 'ChatGPT solo' }] } }, arquivo_corrompido: false });
    postReply = async () => json({ ok: true });
    const { container } = await mount();
    const choice = button(container, 'ChatGPT solo')!;
    expect(choice.disabled).toBe(false);
    await click(container, 'ChatGPT solo');
    await act(async () => confirm()());
    expect(JSON.parse(posts()[0][1].body)).toEqual({ engine_account: 'cpa-0123456789ab' });
  });

  it('exclui a conta por engine_account mesmo antes de conta resolvida chegar', async () => {
    setSession({ ...engineSource, conta: 'chave:local-gpt', engine_account: 'two' });
    const { container } = await mount();
    expect(button(container, 'ChatGPT one')).not.toBeNull();
    expect(button(container, 'ChatGPT two')).toBeNull();
    expect(container.textContent).toContain('local-gpt · two');
  });

  it('catálogo sem contas é restrição informativa, nunca oferece conta Anthropic', async () => {
    engineReply = async () => json({ motores: { 'local-gpt': {} }, arquivo_corrompido: false });
    const { container } = await mount();
    expect(container.textContent).toContain(m.mobile_session_engine_account_unavailable());
    expect(button(container, 'beta')).toBeNull();
    expect(posts()).toHaveLength(0);
    expect(network.mock.calls.some(([url]) => url.endsWith('/conta'))).toBe(false);
  });

  it.each(['http', 'cliproxy', 'credentials', 'corrupt'])('erro %s não vira vazio e retry recupera o catálogo', async kind => {
    if (kind === 'http') engineReply = async () => json({ detail: 'engine-failed' }, 503);
    if (kind === 'cliproxy') engineReply = async () => json({ motores: { 'local-gpt': { cliproxy_error: 'engine-failed' } }, arquivo_corrompido: false });
    if (kind === 'credentials') credentialsReply = async () => json({ detail: 'engine-failed' }, 503);
    if (kind === 'corrupt') engineReply = async () => json({ motores: {}, arquivo_corrompido: true, arquivo_caminho: '/engines.json' });
    const { container } = await mount();
    expect(button(container, m.native_retry())).not.toBeNull();
    expect(container.textContent).not.toContain(m.accounts_none());
    expect(container.textContent).not.toContain(m.mobile_session_engine_account_unavailable());
    if (kind !== 'corrupt') expect(container.textContent).toContain('engine-failed');
    engineReply = async () => json({ motores: { 'local-gpt': { cliproxy_accounts: engineAccounts } }, arquivo_corrompido: false });
    credentialsReply = async () => json(credentials);
    await click(container, m.native_retry());
    expect(button(container, 'ChatGPT two')).not.toBeNull();
  });
});

describe('F2 troca Anthropic', () => {
  it('pede confirmação antes de POST e continua a mesma conversa no alvo capturado', async () => {
    const { container } = await mount();
    await click(container, 'beta');
    expect(boundary.alert).toHaveBeenCalledTimes(1);
    expect(posts()).toHaveLength(0);
    expect(boundary.alert.mock.calls[0][1]).toContain(m.conta_confirmar_msg({ conta: 'beta' }));
    await act(async () => confirm()());
    expect(posts()).toHaveLength(1);
    const [url, init] = posts()[0];
    expect(url).toBe('https://owner.test/api/sessions/same-conversation/conta');
    expect(JSON.parse(init.body)).toEqual({ config_dir: '/home/u/.claude-beta' });
    expect(new Headers(init.headers).get('Authorization')).toBe('Bearer owner-token');
    expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'beta' }));
    expect(container.textContent).not.toContain('alpha');
    expect(boundary.replace).not.toHaveBeenCalled();
    expect(useServers.getState().activeId).toBe('other');
  });

  it('conta esgotada é bloqueada e conta quase cheia avisa na confirmação', async () => {
    listReply = async () => json([...targets, { path: '/home/u/.claude-low', label: 'low', pct: 96, low: true, full: false }]);
    const { container } = await mount();
    expect(button(container, 'full')!.disabled).toBe(true);
    await click(container, 'full');
    expect(boundary.alert).not.toHaveBeenCalled();
    await click(container, 'low');
    expect(boundary.alert).toHaveBeenCalledTimes(1);
    expect(boundary.alert.mock.calls[0][1]).toContain(m.conta_confirmar_acabando({ conta: 'low', pct: 96 }));
    expect(posts()).toHaveLength(0);
  });

  it.each(['working', 'awaiting_input', 'dead', null] as const)('não escreve sem idle confirmado: %s', async state => {
    setSession({ ...source, state: state as SessionInfo['state'] });
    const { container } = await mount();
    expect(container.textContent).toContain(m.native_usage_card_move_busy());
    expect(button(container, 'beta')!.disabled).toBe(true);
    await click(container, 'beta');
    expect(boundary.alert).not.toHaveBeenCalled();
    expect(posts()).toHaveLength(0);
  });

  it('estado do chat tem prioridade sobre idle da lista', async () => {
    chatStore(own.id, source.name).use.setState({ stateEvent: { state: 'working' } as StateEvent });
    const { container } = await mount();
    expect(button(container, 'beta')!.disabled).toBe(true);
    expect(container.textContent).toContain(m.native_usage_card_move_busy());
  });

  it('falha de POST fica visível sem sucesso e permite tentar de novo', async () => {
    postReply = async () => json({ detail: 'transfer-failed' }, 409);
    const { container } = await mount();
    await click(container, 'beta');
    await act(async () => confirm()());
    expect(container.textContent).toContain('transfer-failed');
    expect(container.textContent).not.toContain(m.mobile_session_account_success({ conta: 'beta' }));
    expect(container.textContent).toContain('alpha');
    postReply = async () => json({ ok: true, config_dir: targets[0].path });
    await click(container, 'beta');
    await act(async () => confirm()());
    expect(posts()).toHaveLength(2);
    expect(container.textContent).not.toContain('transfer-failed');
    expect(container.textContent).toContain(m.mobile_session_account_success({ conta: 'beta' }));
  });
});

describe('F1 leitura', () => {
  it('mostra conta da conversa e lê destinos no servidor da rota, sem mudar o ativo', async () => {
    const { container } = await mount();
    expect(container.textContent).toContain(m.mobile_account_current());
    expect(container.textContent).toContain('alpha');
    expect(button(container, 'beta')).not.toBeNull();
    expect(container.textContent).toContain(m.conta_cota_sem_leitura());
    expect(network.mock.calls[0][0]).toBe('https://owner.test/api/sessions/same-conversation/conta');
    expect(new Headers(network.mock.calls[0][1]?.headers).get('Authorization')).toBe('Bearer owner-token');
    expect(useServers.getState().activeId).toBe('other');
  });

  it('mostra carregamento, depois vazio, sem inventar destino', async () => {
    const pending = deferred<Response>(); listReply = () => pending.promise;
    const { container } = await mount();
    expect(container.textContent).toContain(m.comum_carregando());
    await act(async () => pending.resolve(json([])));
    expect(container.textContent).toContain(m.conta_nenhuma_outra());
    expect(container.textContent).not.toContain(m.comum_carregando());
    expect(posts()).toHaveLength(0);
  });

  it('expõe erro da listagem e retry relê os destinos reais', async () => {
    listReply = async () => json({ detail: 'catalog-offline' }, 503);
    const { container } = await mount();
    expect(container.textContent).toContain('catalog-offline');
    listReply = async () => json(targets);
    await click(container, m.native_retry());
    expect(button(container, 'beta')).not.toBeNull();
    expect(container.textContent).not.toContain('catalog-offline');
  });

  it('link direto busca sessão ausente e usa sua conta, não o servidor ativo', async () => {
    useSessions.setState({ rows: [], byServerRecord: { own: [] }, byServer: [{ server: own, sessions: [], loaded: false, error: null }] });
    const { container } = await mount();
    expect(container.textContent).toContain('alpha');
    expect(network.mock.calls.some(([url]) => url === 'https://owner.test/api/sessions')).toBe(true);
    expect(button(container, 'beta')).not.toBeNull();
  });
});
