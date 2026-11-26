// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { IntegrationApprovalEvent, IntegrationPreset, IntegrationToolSummary, IntegrationView, ServerBucket } from '@hangar/core';
import {
  IntegrationRequestSheet,
  IntegrationRequests,
  IntegrationRequestsButton,
  SessionIntegrationRequests,
} from './IntegrationRequests';
import { IntegrationDetailSheet } from './IntegrationSheets';
import { IntegrationEditorSheet } from './IntegrationEditorSheet';
import IntegrationsScreen from '../../../app/config/integrations';
import { openIntegrationApproval, useIntegrationApprovalTarget } from './target';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';

const mocks = vi.hoisted(() => ({
  answerIntegrationApproval: vi.fn(),
  listCatalog: vi.fn(),
  listIntegrations: vi.fn(),
  createIntegration: vi.fn(),
  updateIntegration: vi.fn(),
  deleteIntegration: vi.fn(),
  startIntegrationOAuth: vi.fn(),
  openIntegrationOAuthOnDesktop: vi.fn(),
  testIntegration: vi.fn(),
  listIntegrationTools: vi.fn(),
  registrarDiag: vi.fn(),
  openURL: vi.fn(),
  toastOk: vi.fn(),
  toastErro: vi.fn(),
  appStateListener: null as ((state: string) => void) | null,
  useIdentities: vi.fn(() => ({ identities: new Map([['a', 'a']]), loading: new Set<string>() })),
}));

vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return {
    ...native,
    AppState: {
      currentState: 'active',
      addEventListener: (_event: string, cb: (state: string) => void) => {
        mocks.appStateListener = cb;
        return { remove: () => { if (mocks.appStateListener === cb) mocks.appStateListener = null; } };
      },
    },
    RefreshControl: () => null,
    TextInput: ({ value, onChangeText, accessibilityLabel, editable, placeholder }: {
      value?: string; onChangeText?: (value: string) => void; accessibilityLabel?: string; editable?: boolean; placeholder?: string;
    }) => createElement('textarea', {
      value: value ?? '',
      placeholder,
      'aria-label': accessibilityLabel,
      disabled: editable === false,
      onChange: (event: { currentTarget: { value: string } }) => onChangeText?.(event.currentTarget.value),
    }),
  };
});
vi.mock('expo-router', () => ({ Stack: { Screen: () => null } }));
vi.mock('../config/Pagina', () => ({ Pagina: ({ children }: { children: ReactNode }) => createElement('main', null, children) }));
vi.mock('expo-linking', () => ({ openURL: mocks.openURL }));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  answerIntegrationApproval: mocks.answerIntegrationApproval,
  listCatalog: mocks.listCatalog,
  listIntegrations: mocks.listIntegrations,
  createIntegration: mocks.createIntegration,
  updateIntegration: mocks.updateIntegration,
  deleteIntegration: mocks.deleteIntegration,
  startIntegrationOAuth: mocks.startIntegrationOAuth,
  openIntegrationOAuthOnDesktop: mocks.openIntegrationOAuthOnDesktop,
  testIntegration: mocks.testIntegration,
  listIntegrationTools: mocks.listIntegrationTools,
  registrarDiag: mocks.registrarDiag,
}));
vi.mock('../machines/parts', () => ({
  MachineSheet: ({ title, children }: { title: string; children: ReactNode }) =>
    createElement('section', { 'aria-label': title }, children),
  Notice: ({ text }: { text: string }) => (text ? createElement('div', { role: 'alert' }, text) : null),
}));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('../../ui/Icon', () => ({ Icon: ({ name }: { name: string }) => createElement('i', { 'data-icon': name }) }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: mocks.toastOk, erro: mocks.toastErro } }));
vi.mock('../pair/identities', () => ({ useIdentities: () => mocks.useIdentities() }));

const makeEvent = (id = 'r1'): IntegrationApprovalEvent => ({
  requests: [{
    id,
    interaction_id: `int-${id}`,
    origin: 'a::sessao',
    integration_id: 'github',
    integration_name: 'GitHub',
    tool: 'create_issue',
    arguments_preview: '{"title":"Bug"}',
    reason: 'Abrir issue no repositório',
    expires_at: 9999999,
  }],
});

const bucket = (id: string, extra: Partial<ServerBucket> = {}) => ({
  server: { id, label: id, baseUrl: `https://${id}.test`, token: 't' },
  sessions: [],
  error: null,
  loaded: true,
  ...extra,
}) as unknown as ServerBucket;

let container: HTMLElement;
let root: Root;

const render = (byServer: ServerBucket[]) => act(async () => {
  useSessions.setState({ byServer });
  root.render(<><IntegrationRequests byServer={byServer} /><IntegrationRequestSheet /></>);
});

const title = (label: string, count = 1) => m.integrations_approval_requests_title({ label, count });

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.clearAllMocks();
  mocks.appStateListener = null;
  mocks.listCatalog.mockResolvedValue([]);
  mocks.listIntegrations.mockResolvedValue([]);
  mocks.useIdentities.mockImplementation(() => ({ identities: new Map([['a', 'a']]), loading: new Set<string>() }));
  container = document.createElement('div');
  root = createRoot(container);
  openIntegrationApproval(null);
  useSessions.setState({ integrationApprovals: { a: makeEvent('r1') } });
});

afterEach(async () => {
  await act(async () => { root.unmount(); });
  useSessions.setState({ integrationApprovals: {}, byServer: [], servers: [] });
  useServers.setState({ servers: [], activeId: null, ready: true, vaultReadable: true });
  openIntegrationApproval(null);
});

it('tocar no aviso do pedido pendente abre a folha daquela máquina e ignora offline/carregando', async () => {
  await render([bucket('a'), bucket('b')]);
  const notices = [...container.querySelectorAll('button')].filter((b) => b.getAttribute('aria-label') === title('a'));
  expect(notices).toHaveLength(1);
  expect(container.querySelector(`button[aria-label="${title('b')}"]`)).toBeNull();
  expect(useIntegrationApprovalTarget.getState().target).toBeNull();

  await act(async () => { notices[0].click(); });

  expect(useIntegrationApprovalTarget.getState().target).toEqual({ serverId: 'a', requestId: null });
  expect(container.querySelector(`section[aria-label="${m.integrations_approval_title()}"]`)).not.toBeNull();
  expect(container.textContent).toContain('GitHub');
  expect(container.textContent).toContain('create_issue');
  expect(container.textContent).toContain('Abrir issue no repositório');

  await render([bucket('a', { error: 'offline' })]);
  expect(container.querySelector(`button[aria-label="${title('a')}"]`)).toBeNull();
  await render([bucket('a', { loaded: false })]);
  expect(container.querySelector(`button[aria-label="${title('a')}"]`)).toBeNull();
});

it('botão do tablet exibe contagem de aprovações pendentes e abre a folha ao tocar', async () => {
  useSessions.setState({ byServer: [bucket('a')], integrationApprovals: { a: makeEvent('btn-1') } });
  await act(async () => { root.render(<IntegrationRequestsButton />); });
  const btn = container.querySelector<HTMLButtonElement>(`button[aria-label="${title('a')}"]`);
  expect(btn).not.toBeNull();
  expect(btn!.textContent).toContain('1');

  await act(async () => { btn!.click(); });
  expect(useIntegrationApprovalTarget.getState().target).toEqual({ serverId: 'a', requestId: null });
});

it('no chat da sessão de origem abre o pedido; em outra sessão não mostra', async () => {
  useSessions.setState({ byServer: [bucket('a')], integrationApprovals: { a: makeEvent('chat-1') } });
  const banner = m.integrations_approval_session_notice({ integration: 'GitHub', tool: 'create_issue' });
  await act(async () => { root.render(<SessionIntegrationRequests serverId="a" name="outra" />); });
  expect(container.querySelector(`button[aria-label="${banner}"]`)).toBeNull();

  await act(async () => { root.render(<><SessionIntegrationRequests serverId="a" name="sessao" /><IntegrationRequestSheet /></>); });
  await act(async () => { container.querySelector<HTMLElement>(`button[aria-label="${banner}"]`)!.click(); });

  expect(useIntegrationApprovalTarget.getState().target).toEqual({ serverId: 'a', requestId: 'chat-1' });
  expect(container.textContent).toContain('{"title":"Bug"}');
});

it.each([
  ['allow_once', m.integrations_approval_allow_once, m.integrations_approval_allowed_once],
  ['allow_always', m.integrations_approval_allow_always, m.integrations_approval_allowed_always],
  ['deny', m.integrations_approval_deny, m.integrations_approval_denied],
] as const)('ação %s envia decisão ao servidor, avisa e retira da fila local', async (decision, buttonLabel, toastLabel) => {
  const id = `act-${decision}`;
  useSessions.setState({ byServer: [bucket('a')], integrationApprovals: { a: makeEvent(id) } });
  mocks.answerIntegrationApproval.mockResolvedValueOnce({ ok: true, decision });
  openIntegrationApproval({ serverId: 'a', requestId: id });
  await act(async () => { root.render(<IntegrationRequestSheet />); });

  const button = container.querySelector<HTMLButtonElement>(`button[aria-label="${buttonLabel()}"]`);
  expect(button).not.toBeNull();
  await act(async () => { button!.click(); });

  expect(mocks.answerIntegrationApproval).toHaveBeenCalledWith(
    bucket('a').server,
    id,
    { interaction_id: `int-${id}`, decision },
  );
  expect(mocks.toastOk).toHaveBeenCalledWith(toastLabel());
  expect(useSessions.getState().integrationApprovals.a?.requests.map((r) => r.id) ?? []).not.toContain(id);
});

it('pedido expirado ou respondido fora (approval_conflict / approval_lost) descarta cartão e mostra erro', async () => {
  useSessions.setState({ byServer: [bucket('a')], integrationApprovals: { a: makeEvent('stale-1') } });
  mocks.answerIntegrationApproval.mockRejectedValueOnce(Object.assign(new Error('409: já respondido'), { status: 409, code: 'approval_answered' }));
  openIntegrationApproval({ serverId: 'a', requestId: 'stale-1' });
  await act(async () => { root.render(<IntegrationRequestSheet />); });

  const button = container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_approval_allow_once()}"]`);
  await act(async () => { button!.click(); });

  expect(mocks.toastErro).toHaveBeenCalledWith(m.integrations_err_answered());
  expect(useSessions.getState().integrationApprovals.a?.requests.map((r) => r.id) ?? []).not.toContain('stale-1');
});

it('approval_in_progress mantém cartão na fila local e múltiplos pedidos no chat viram um único aviso agregado', async () => {
  const twoCards: IntegrationApprovalEvent = {
    requests: [
      makeEvent('p1').requests[0],
      { ...makeEvent('p2').requests[0], tool: 'add_comment' },
    ],
  };
  useSessions.setState({ byServer: [bucket('a')], integrationApprovals: { a: twoCards } });
  mocks.answerIntegrationApproval.mockRejectedValueOnce(
    Object.assign(new Error('409: em andamento'), { status: 409, code: 'approval_in_progress' }),
  );

  await act(async () => { root.render(<><SessionIntegrationRequests serverId="a" name="sessao" /><IntegrationRequestSheet /></>); });
  const chatButtons = [...container.querySelectorAll('button')];
  expect(chatButtons).toHaveLength(1);
  expect(chatButtons[0].getAttribute('aria-label')).toBe(title('a', 2));

  await act(async () => { chatButtons[0].click(); });
  expect(useIntegrationApprovalTarget.getState().target).toEqual({ serverId: 'a', requestId: null });

  const allowBtn = container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_approval_allow_once()}"]`);
  await act(async () => { allowBtn!.click(); });
  expect(useSessions.getState().integrationApprovals.a?.requests.map((r) => r.id)).toEqual(['p1', 'p2']);
});

const oauthView = (extra: Partial<IntegrationView> = {}): IntegrationView => ({
  id: 'notion',
  name: 'Notion',
  preset: 'notion',
  enabled: true,
  transport: { kind: 'remote_http', url: 'https://mcp.notion.com/mcp', header_names: [] },
  auth: { kind: 'oauth_dcr' },
  policy: 'read_free_ask_write',
  pinned: false,
  projects: [],
  allowed_tools: [],
  created_at: 1000,
  updated_at: 1000,
  status: 'auth_required',
  ...extra,
});

it('fluxo OAuth mobile abre authorize_url com Linking.openURL quando mode === open_url', async () => {
  const srv = { id: 'a', label: 'A', baseUrl: 'http://192.168.1.10:8765', remote: { url: 'https://pc.tail.ts.net', id: 'pc' }, token: 't' };
  mocks.listIntegrationTools.mockResolvedValueOnce([]);
  mocks.startIntegrationOAuth.mockResolvedValueOnce({
    mode: 'open_url',
    authorize_url: 'https://mcp.notion.com/authorize?state=xyz',
    redirect_uri: 'https://pc.tail.ts.net/api/integrations/oauth/callback',
  });
  mocks.openURL.mockResolvedValueOnce(undefined);

  await act(async () => {
    root.render(
      <IntegrationDetailSheet
        server={srv}
        integration={oauthView()}
        onClose={vi.fn()}
        onChanged={vi.fn()}
        onEdit={vi.fn()}
      />,
    );
  });

  const authBtn = container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_authorize()}"]`);
  expect(authBtn).not.toBeNull();
  await act(async () => { authBtn!.click(); });

  expect(mocks.startIntegrationOAuth).toHaveBeenCalledWith(srv, 'notion', {
    origin_client: 'mobile',
    public_base_url: 'https://pc.tail.ts.net',
  });
  expect(mocks.openURL).toHaveBeenCalledWith('https://mcp.notion.com/authorize?state=xyz');
  expect(container.textContent).toContain(m.integrations_oauth_started());
});

it('fluxo handoff_desktop mostra aviso e botão que chama openIntegrationOAuthOnDesktop com authorize_url', async () => {
  const srv = { id: 'a', label: 'A', baseUrl: 'http://192.168.1.10:8765', token: 't' };
  const authorizeUrl = 'https://accounts.google.com/o/oauth2/v2/auth?state=g1';
  mocks.listIntegrationTools.mockResolvedValueOnce([]);
  mocks.startIntegrationOAuth.mockResolvedValueOnce({
    mode: 'handoff_desktop',
    authorize_url: authorizeUrl,
    redirect_uri: 'http://127.0.0.1:8765/api/integrations/oauth/callback',
    reason: 'este provedor exige redirecionamento em localhost (127.0.0.1)',
  });
  mocks.openIntegrationOAuthOnDesktop.mockResolvedValueOnce({ ok: true });

  await act(async () => {
    root.render(
      <IntegrationDetailSheet
        server={srv}
        integration={oauthView({ id: 'gmail-pessoal', name: 'Gmail Pessoal', preset: 'google-gmail' })}
        onClose={vi.fn()}
        onChanged={vi.fn()}
        onEdit={vi.fn()}
      />,
    );
  });

  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_authorize()}"]`)!.click();
  });

  expect(mocks.openURL).not.toHaveBeenCalled();
  expect(container.textContent).toContain('este provedor exige redirecionamento em localhost (127.0.0.1)');

  const desktopBtn = container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_oauth_open_desktop()}"]`);
  expect(desktopBtn).not.toBeNull();
  await act(async () => { desktopBtn!.click(); });

  expect(mocks.openIntegrationOAuthOnDesktop).toHaveBeenCalledWith(srv, 'gmail-pessoal', authorizeUrl);
  expect(mocks.toastOk).toHaveBeenCalledWith(m.integrations_oauth_desktop_opened());
});

it('testIntegration trata 424 auth_expired na folha e libera ferramentas ao aprovar sempre', async () => {
  const srv = bucket('a').server;
  const tools: IntegrationToolSummary[] = [
    { name: 'create_issue', description: 'Cria issue', input_schema: {}, read_only_hint: false, effective_access: 'ask_write' },
  ];
  mocks.listIntegrationTools.mockResolvedValueOnce(tools);
  mocks.testIntegration.mockResolvedValueOnce({ status: 'auth_expired', tools: [], reason: 'token venceu' });
  mocks.updateIntegration.mockResolvedValueOnce(oauthView({ status: 'ready', allowed_tools: ['create_issue'] }));
  mocks.listIntegrationTools.mockResolvedValueOnce([{ ...tools[0], effective_access: 'allowed_always' }]);

  const onChanged = vi.fn();
  await act(async () => {
    root.render(
      <IntegrationDetailSheet
        server={srv}
        integration={oauthView({ status: 'ready' })}
        onClose={vi.fn()}
        onChanged={onChanged}
        onEdit={vi.fn()}
      />,
    );
  });

  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_test()}"]`)!.click();
  });
  expect(container.textContent).toContain(m.integrations_err_auth_expired());
  expect(onChanged).toHaveBeenCalled();

  const allowBtn = container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_tool_allow_always()} create_issue"]`);
  expect(allowBtn).not.toBeNull();
  await act(async () => { allowBtn!.click(); });

  expect(mocks.updateIntegration).toHaveBeenCalledWith(srv, 'notion', { add_allowed_tools: ['create_issue'] });
});

it('editor de preset Google exibe guia curto e nota dos 7 dias em Testing', async () => {
  const googlePreset: IntegrationPreset = {
    id: 'google-gmail',
    name: 'Google Gmail',
    category: 'productivity',
    description: 'E-mails no Gmail',
    url: 'https://gmailmcp.googleapis.com/mcp/v1',
    url_editable: false,
    auth_kind: 'oauth_manual_client',
    default_scopes: ['https://www.googleapis.com/auth/gmail.readonly'],
    requires_loopback_redirect: true,
    setup_guide: {
      title: 'Configurar Client ID do Google Cloud para Gmail MCP',
      steps: ['Passo 1 do console', 'Passo 2 do consentimento'],
      docs_hint: 'Google Cloud Console -> Credenciais',
    },
  };

  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        server={bucket('a').server}
        preset={googlePreset}
        existingIds={['google-gmail']}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });

  expect(container.textContent).toContain('Configurar Client ID do Google Cloud para Gmail MCP');
  expect(container.textContent).toContain('Passo 1 do console');
  expect(container.textContent).toContain(m.integrations_google_testing_hint());
});

it('com 2 servidores próprios e identidade pendente não vaza pedido de sessão homônima entre máquinas', async () => {
  const srvA = bucket('a').server;
  const srvB = bucket('b').server;
  const inviteC = { ...bucket('c').server, invite: true };
  const banner = m.integrations_approval_session_notice({ integration: 'GitHub', tool: 'create_issue' });
  mocks.useIdentities.mockImplementation(() => ({ identities: new Map(), loading: new Set(['a', 'b']) }));

  useSessions.setState({
    servers: [srvA, srvB],
    byServer: [bucket('a'), bucket('b')],
    integrationApprovals: { a: makeEvent('homonima-1') },
  });

  await act(async () => { root.render(<SessionIntegrationRequests serverId="b" name="sessao" />); });
  expect(container.querySelector(`button[aria-label="${banner}"]`)).toBeNull();

  await act(async () => { root.render(<SessionIntegrationRequests serverId="a" name="sessao" />); });
  expect(container.querySelector(`button[aria-label="${banner}"]`)).toBeNull();

  // Com apenas 1 servidor próprio (convite externo não conta), só casa quando owner === serverId.
  await act(async () => {
    useSessions.setState({
      servers: [srvA, inviteC],
      byServer: [bucket('a')],
      integrationApprovals: { a: makeEvent('single-1') },
    });
  });
  await act(async () => { root.render(<SessionIntegrationRequests serverId="b" name="sessao" />); });
  expect(container.querySelector(`button[aria-label="${banner}"]`)).toBeNull();

  await act(async () => { root.render(<SessionIntegrationRequests serverId="a" name="sessao" />); });
  expect(container.querySelector(`button[aria-label="${banner}"]`)).not.toBeNull();
});

it('editar integração com preset preserva url_editable=false, modos de auth permitidos e guia Google', async () => {
  const srv = bucket('a').server;
  const googlePreset: IntegrationPreset = {
    id: 'google-gmail',
    name: 'Google Gmail',
    category: 'productivity',
    description: 'E-mails no Gmail',
    url: 'https://gmailmcp.googleapis.com/mcp/v1',
    url_editable: false,
    auth_kind: 'oauth_manual_client',
    default_scopes: ['https://www.googleapis.com/auth/gmail.readonly'],
    requires_loopback_redirect: true,
    setup_guide: {
      title: 'Configurar Client ID do Google Cloud para Gmail MCP',
      steps: ['Passo 1 do console'],
      docs_hint: 'Google Cloud Console',
    },
  };
  const integration: IntegrationView = {
    id: 'gmail-pessoal',
    name: 'Gmail Pessoal',
    preset: 'google-gmail',
    enabled: true,
    transport: { kind: 'remote_http', url: 'https://gmailmcp.googleapis.com/mcp/v1', header_names: [] },
    auth: { kind: 'oauth_manual_client', client_id: 'cid-1', scopes: ['https://www.googleapis.com/auth/gmail.readonly'] },
    policy: 'read_free_ask_write',
    pinned: false,
    projects: [],
    allowed_tools: [],
    created_at: 1000,
    updated_at: 1000,
    status: 'auth_required',
  };
  mocks.listIntegrations.mockResolvedValueOnce([integration]);
  mocks.listCatalog.mockResolvedValueOnce([googlePreset]);
  mocks.listIntegrationTools.mockResolvedValueOnce([]);
  useServers.setState({ servers: [srv], activeId: srv.id, ready: true, vaultReadable: true });

  await act(async () => { root.render(<IntegrationsScreen />); });
  await act(async () => {
    container.querySelector<HTMLButtonElement>('button[aria-label="Gmail Pessoal"]')!.click();
  });
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_edit()}"]`)!.click();
  });

  const urlField = container.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${m.integrations_field_url()}"]`);
  expect(urlField).not.toBeNull();
  expect(urlField!.disabled).toBe(true);
  expect(container.textContent).toContain('Configurar Client ID do Google Cloud para Gmail MCP');
  expect(container.textContent).toContain(m.integrations_google_testing_hint());
  expect(container.querySelector(`button[aria-label="${m.integrations_auth_oauth_dcr()}"]`)).toBeNull();
  expect(container.querySelector(`button[aria-label="${m.integrations_auth_bearer()}"]`)).toBeNull();
  expect(container.querySelector(`button[aria-label="${m.integrations_auth_none()}"]`)).toBeNull();
});

async function setField(label: string, value: string) {
  const el = container.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${label}"]`);
  expect(el, label).not.toBeNull();
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(el, value);
    el!.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

it('editor rejeita cabeçalhos e variáveis de ambiente malformados ou cabeçalhos reservados MCP', async () => {
  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        server={bucket('a').server}
        existingIds={[]}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });

  await setField(m.integrations_field_id(), 'custom-mcp');
  await setField(m.integrations_field_name(), 'Custom MCP');
  await setField(m.integrations_field_url(), 'https://mcp.example.com/mcp');

  for (const [badHeader, expectedError] of [
    ['sem-separador', m.integrations_field_headers()],
    [': valor-sem-nome', m.integrations_field_headers()],
    ['X-Empty:   ', `${m.integrations_field_headers()}: x-empty`],
    ['X(Key): valor', m.integrations_validation_header_name({ name: 'x(key)' })],
    ['Authorization: Bearer segredo', m.integrations_validation_header_name({ name: 'authorization' })],
    ['Accept: text/plain', m.integrations_validation_header_name({ name: 'accept' })],
    ['Mcp-Session-Id: 123', m.integrations_validation_header_name({ name: 'mcp-session-id' })],
    ['Mcp-Protocol-Version: 2025-03-26', m.integrations_validation_header_name({ name: 'mcp-protocol-version' })],
    ['Last-Event-Id: 9', m.integrations_validation_header_name({ name: 'last-event-id' })],
  ] as const) {
    await setField(m.integrations_field_headers(), badHeader);
    await act(async () => {
      container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_connect()}"]`)!.click();
    });
    expect(mocks.createIntegration).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent).toBe(expectedError);
  }

  // Troca para stdio e testa env malformada.
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_transport_local()}"]`)!.click();
  });
  await setField(m.integrations_field_command(), 'npx -y @org/mcp-server');
  for (const badEnv of ['SEM_IGUAL', '=valor-sem-chave']) {
    await setField(m.integrations_field_env(), badEnv);
    await act(async () => {
      container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
    });
    expect(mocks.createIntegration).not.toHaveBeenCalled();
    expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.integrations_field_env());
  }
});

it('edição mostra cabeçalhos e variáveis salvos e envia null em secrets ao remover chave', async () => {
  const srv = bucket('a').server;
  const remoteItem: IntegrationView = {
    id: 'custom-http',
    name: 'Custom HTTP',
    preset: null,
    enabled: true,
    transport: { kind: 'remote_http', url: 'https://mcp.example.com/mcp', header_names: ['x-tenant', 'x-region'] },
    auth: { kind: 'none' },
    policy: 'read_free_ask_write',
    pinned: false,
    projects: [],
    allowed_tools: [],
    created_at: 1000,
    updated_at: 1000,
    status: 'ready',
  };
  mocks.updateIntegration.mockResolvedValueOnce({
    ...remoteItem,
    transport: { kind: 'remote_http', url: 'https://mcp.example.com/mcp', header_names: ['x-region'] },
  });

  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        server={srv}
        editing={remoteItem}
        existingIds={['custom-http']}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });

  expect(container.textContent).toContain('x-tenant');
  expect(container.textContent).toContain('x-region');
  const rmTenantBtn = container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_remove()} x-tenant"]`);
  expect(rmTenantBtn).not.toBeNull();

  await act(async () => { rmTenantBtn!.click(); });
  expect(container.textContent).toContain(m.native_server_remove_on_save());

  const undoTenantBtn = container.querySelector<HTMLButtonElement>(`button[aria-label="${m.native_server_undo()} x-tenant"]`);
  expect(undoTenantBtn).not.toBeNull();
  await act(async () => { undoTenantBtn!.click(); });
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_remove()} x-tenant"]`)!.click();
  });

  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });

  expect(mocks.updateIntegration).toHaveBeenCalledWith(srv, 'custom-http', expect.objectContaining({
    transport: { kind: 'remote_http', url: 'https://mcp.example.com/mcp', header_names: ['x-region'] },
    secrets: { headers: { 'x-tenant': null } },
  }));

  // Agora testa remoção de env_key em transporte local_stdio.
  const stdioItem: IntegrationView = {
    ...remoteItem,
    id: 'local-fs',
    name: 'Local FS',
    transport: { kind: 'local_stdio', command: ['npx', 'fs-mcp'], cwd: null, env_keys: ['API_KEY', 'MODE'] },
  };
  mocks.updateIntegration.mockResolvedValueOnce({
    ...stdioItem,
    transport: { kind: 'local_stdio', command: ['npx', 'fs-mcp'], cwd: null, env_keys: ['MODE', 'NEW_VAR'] },
  });

  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        key="stdio"
        server={srv}
        editing={stdioItem}
        existingIds={['local-fs']}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });

  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_remove()} API_KEY"]`)!.click();
  });
  await setField(m.integrations_field_env(), 'NEW_VAR=ok');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });

  expect(mocks.updateIntegration).toHaveBeenLastCalledWith(srv, 'local-fs', expect.objectContaining({
    transport: { kind: 'local_stdio', command: ['npx', 'fs-mcp'], cwd: null, env_keys: ['MODE', 'NEW_VAR'] },
    secrets: { env: { API_KEY: null, NEW_VAR: 'ok' } },
  }));
});

it('folha de aprovação não sequestra modal trocado de máquina ou fechado durante envio lento', async () => {
  let resolveA!: (value: { ok: boolean; decision: 'allow_once' }) => void;
  mocks.answerIntegrationApproval.mockImplementationOnce(() => new Promise((resolve) => { resolveA = resolve; }));

  useSessions.setState({
    byServer: [bucket('a'), bucket('b')],
    integrationApprovals: {
      a: makeEvent('req-a'),
      b: {
        requests: [{
          id: 'req-b',
          interaction_id: 'int-b',
          origin: 'b::sessao',
          integration_id: 'notion',
          integration_name: 'Notion',
          tool: 'create_page',
          arguments_preview: '{}',
          reason: 'Criar página',
          expires_at: 9999999,
        }],
      },
    },
  });
  openIntegrationApproval({ serverId: 'a', requestId: 'req-a' });
  await act(async () => { root.render(<IntegrationRequestSheet />); });

  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_approval_allow_once()}"]`)!.click();
  });

  // Usuário troca a folha para a máquina B enquanto a resposta da máquina A ainda está em voo.
  await act(async () => { openIntegrationApproval({ serverId: 'b', requestId: 'req-b' }); });
  expect(container.textContent).toContain('Notion');

  await act(async () => { resolveA({ ok: true, decision: 'allow_once' }); });
  expect(useIntegrationApprovalTarget.getState().target).toEqual({ serverId: 'b', requestId: 'req-b' });
  expect(container.textContent).toContain('Notion');

  // Mesmo cenário ao fechar a folha antes de erro terminal da máquina B: não reabre a folha.
  let rejectB!: (err: unknown) => void;
  mocks.answerIntegrationApproval.mockImplementationOnce(() => new Promise((_, reject) => { rejectB = reject; }));
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_approval_allow_once()}"]`)!.click();
  });
  await act(async () => { openIntegrationApproval(null); });
  await act(async () => { rejectB(Object.assign(new Error('409'), { status: 409, code: 'approval_answered' })); });
  expect(useIntegrationApprovalTarget.getState().target).toBeNull();
});

it('editor valida aspas abertas, URL HTTP com credenciais, preserva comando com espaços e omite transport inalterado no PATCH', async () => {
  const srv = bucket('a').server;
  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        server={srv}
        existingIds={[]}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });

  await setField(m.integrations_field_id(), 'http-insecure');
  await setField(m.integrations_field_name(), 'HTTP Inseguro');
  await setField(m.integrations_field_url(), 'http://mcp.example.com/mcp');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_connect()}"]`)!.click();
  });
  expect(mocks.createIntegration).not.toHaveBeenCalled();
  expect(container.textContent).toContain(m.integrations_validation_url_insecure());

  // Sem autenticação e sem cabeçalhos, HTTP é permitido; com cabeçalho extra, HTTP não-loopback é bloqueado.
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_auth_none()}"]`)!.click();
  });
  await setField(m.integrations_field_headers(), 'X-Custom: segredo');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });
  expect(mocks.createIntegration).not.toHaveBeenCalled();
  expect(container.textContent).toContain(m.integrations_validation_url_insecure());

  // Loopback HTTP com auth continua aceito.
  await setField(m.integrations_field_url(), 'http://127.0.0.1:8080/mcp');
  mocks.createIntegration.mockResolvedValueOnce(oauthView({ id: 'http-insecure', auth: { kind: 'none' } }));
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });
  expect(mocks.createIntegration).toHaveBeenCalledTimes(1);

  // Local stdio: aspas sem fechamento bloqueiam envio.
  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        key="unclosed-cmd"
        server={srv}
        existingIds={[]}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });
  await setField(m.integrations_field_id(), 'local-cmd');
  await setField(m.integrations_field_name(), 'Local CMD');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_transport_local()}"]`)!.click();
  });
  await setField(m.integrations_field_command(), 'npx "unclosed');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });
  expect(mocks.createIntegration).toHaveBeenCalledTimes(1);
  expect(container.textContent).toContain(m.integrations_validation_command_quotes());

  // Edição stdio: mudar apenas o nome omite transport; mudar apenas cwd reutiliza command original com espaços/aspas.
  const stdioSpaces: IntegrationView = {
    ...oauthView({ id: 'local-spaces', name: 'Original', auth: { kind: 'none' } }),
    transport: {
      kind: 'local_stdio',
      command: ['/opt/My Tools/server', '--label', 'say "hi"'],
      cwd: '/tmp/old',
      env_keys: ['ENV_A'],
    },
  };
  mocks.updateIntegration.mockResolvedValue({ ...stdioSpaces, name: 'Renomeada' });
  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        key="edit-spaces-name"
        server={srv}
        editing={stdioSpaces}
        existingIds={['local-spaces']}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });
  await setField(m.integrations_field_name(), 'Renomeada');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });
  const firstPatch = mocks.updateIntegration.mock.calls.at(-1)![2];
  expect(firstPatch.name).toBe('Renomeada');
  expect('transport' in firstPatch).toBe(false);

  // Muda só cwd e env com espaço final: reutiliza command original e preserva espaço final.
  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        key="edit-spaces-cwd"
        server={srv}
        editing={stdioSpaces}
        existingIds={['local-spaces']}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });
  await setField(m.integrations_field_cwd(), '/tmp/new');
  await setField(m.integrations_field_env(), 'LABEL=production ');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });
  expect(mocks.updateIntegration).toHaveBeenLastCalledWith(srv, 'local-spaces', expect.objectContaining({
    transport: {
      kind: 'local_stdio',
      command: ['/opt/My Tools/server', '--label', 'say "hi"'],
      cwd: '/tmp/new',
      env_keys: ['ENV_A', 'LABEL'],
    },
    secrets: { env: { LABEL: 'production ' } },
  }));

  // Edição remote_http: atualizar valor de segredo sem mudar url nem header_names omite transport.
  const remoteItem = oauthView({
    id: 'remote-keep',
    name: 'Remote Keep',
    transport: { kind: 'remote_http', url: 'https://mcp.example.com/mcp', header_names: ['x-tenant'] },
    auth: { kind: 'none' },
  });
  mocks.updateIntegration.mockResolvedValueOnce(remoteItem);
  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        key="edit-remote-secret"
        server={srv}
        editing={remoteItem}
        existingIds={['remote-keep']}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });
  await setField(m.integrations_field_headers(), 'X-Tenant: novo-segredo');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });
  const remotePatch = mocks.updateIntegration.mock.calls.at(-1)![2];
  expect('transport' in remotePatch).toBe(false);
  expect(remotePatch.secrets).toEqual({ headers: { 'x-tenant': 'novo-segredo' } });
});

it('listagem de ferramentas antiga não sobrescreve ferramentas retornadas por testIntegration', async () => {
  const srv = bucket('a').server;
  let resolveStaleTools!: (tools: IntegrationToolSummary[]) => void;
  mocks.listIntegrationTools.mockImplementationOnce(() => new Promise((resolve) => { resolveStaleTools = resolve; }));
  mocks.testIntegration.mockResolvedValueOnce({
    status: 'ready',
    tools: [{ name: 'fresh_tool', description: 'Nova', input_schema: {}, read_only_hint: true, effective_access: 'read' }],
  });

  await act(async () => {
    root.render(
      <IntegrationDetailSheet
        server={srv}
        integration={oauthView({ status: 'ready' })}
        onClose={vi.fn()}
        onChanged={vi.fn()}
        onEdit={vi.fn()}
      />,
    );
  });

  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_test()}"]`)!.click();
  });
  expect(container.textContent).toContain('fresh_tool');

  await act(async () => {
    resolveStaleTools([{ name: 'stale_tool', description: 'Antiga', input_schema: {}, read_only_hint: true, effective_access: 'read' }]);
  });
  expect(container.textContent).toContain('fresh_tool');
  expect(container.textContent).not.toContain('stale_tool');
});

it('tela de integrações faz refresh silencioso no AppState active e descarta onSaved após troca de servidor', async () => {
  const srvA = bucket('a').server;
  const srvB = bucket('b').server;
  const itemA = oauthView({ id: 'srv-a-item', name: 'Item Servidor A', auth: { kind: 'none' } });
  mocks.listIntegrations.mockResolvedValueOnce([itemA]);
  mocks.listCatalog.mockResolvedValueOnce([]);
  useServers.setState({ servers: [srvA, srvB], activeId: 'a', ready: true, vaultReadable: true });

  await act(async () => { root.render(<IntegrationsScreen />); });
  expect(container.textContent).toContain('Item Servidor A');
  expect(mocks.appStateListener).not.toBeNull();

  // Volta ao app com erro transitório: preserva a lista, não exibe erro na tela e registra no diário.
  mocks.listIntegrations.mockRejectedValueOnce(new Error('offline transitório'));
  mocks.listCatalog.mockResolvedValueOnce([]);
  await act(async () => { mocks.appStateListener?.('active'); });
  expect(container.textContent).toContain('Item Servidor A');
  expect(container.textContent).not.toContain('offline transitório');
  expect(mocks.registrarDiag).toHaveBeenCalledWith(
    expect.objectContaining({ evento: 'integrations.refresh_failed', nivel: 'aviso', tela: 'integrations' }),
    srvA.baseUrl,
  );

  // Abre editor na máquina A, dispara criação lenta, troca para máquina B e resolve depois: não vaza item na máquina B.
  let resolveCreate!: (v: IntegrationView) => void;
  mocks.createIntegration.mockImplementationOnce(() => new Promise((resolve) => { resolveCreate = resolve; }));
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_add_custom()}"]`)!.click();
  });
  await setField(m.integrations_field_id(), 'vazamento-a');
  await setField(m.integrations_field_name(), 'Vazamento A');
  await setField(m.integrations_field_url(), 'https://a.example.com/mcp');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_auth_none()}"]`)!.click();
  });
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });

  mocks.listIntegrations.mockResolvedValueOnce([]);
  mocks.listCatalog.mockResolvedValueOnce([]);
  await act(async () => {
    useServers.setState({ activeId: 'b' });
  });
  expect(container.textContent).toContain(m.integrations_empty());

  await act(async () => {
    resolveCreate(oauthView({ id: 'vazamento-a', name: 'Vazamento A', auth: { kind: 'none' } }));
  });
  expect(container.textContent).not.toContain('Vazamento A');
  expect(container.textContent).toContain(m.integrations_empty());
});

it('editor omite policy/projects inalterados, preserva escopos com vírgula, limpa erro de validação e preserva __proto__', async () => {
  const srv = bucket('a').server;
  const existing: IntegrationView = oauthView({
    id: 'oauth-comma',
    name: 'OAuth Comma',
    preset: null,
    transport: { kind: 'remote_http', url: 'https://mcp.example.com/mcp', header_names: ['__proto__'] },
    auth: { kind: 'oauth_manual_client', client_id: 'cid-1', scopes: ['reports,read', 'openid'] },
    policy: 'read_only',
    projects: ['/repo/a'],
  });
  mocks.updateIntegration.mockResolvedValueOnce({ ...existing, name: 'Novo Nome' });

  await act(async () => {
    root.render(
      <IntegrationEditorSheet
        server={srv}
        editing={existing}
        existingIds={['oauth-comma']}
        onClose={vi.fn()}
        onSaved={vi.fn()}
      />,
    );
  });

  // Erro de validação limpa ao editar URL; erro real de API permanece ao editar campo.
  await setField(m.integrations_field_url(), 'ftp://invalido');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.integrations_validation_url());
  await setField(m.integrations_field_url(), 'https://mcp.example.com/mcp');
  expect(container.querySelector('[role="alert"]')).toBeNull();

  await setField(m.integrations_field_name(), 'Novo Nome');
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_remove()} __proto__"]`)!.click();
  });
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_save()}"]`)!.click();
  });

  const patch = mocks.updateIntegration.mock.calls[0][2];
  expect('policy' in patch).toBe(false);
  expect('projects' in patch).toBe(false);
  expect(patch.auth).toEqual({ kind: 'oauth_manual_client', client_id: 'cid-1', scopes: ['reports,read', 'openid'] });
  expect(JSON.stringify(patch.secrets?.headers)).toBe('{"__proto__":null}');
});

it('folha de detalhe aplica estado do PATCH antes de liberar controles, invalida tools em erro pós-política e limpa faixa OAuth ao ficar ready', async () => {
  const srv = bucket('a').server;
  const askTool: IntegrationToolSummary = {
    name: 'create_issue',
    description: 'Cria issue',
    input_schema: {},
    read_only_hint: false,
    effective_access: 'ask_write',
  };
  mocks.listIntegrationTools.mockResolvedValueOnce([askTool]);
  mocks.updateIntegration.mockResolvedValueOnce(oauthView({ status: 'ready', enabled: false }));
  mocks.updateIntegration.mockResolvedValueOnce(oauthView({ status: 'ready', enabled: true }));

  await act(async () => {
    root.render(
      <IntegrationDetailSheet
        server={srv}
        integration={oauthView({ status: 'ready', enabled: true })}
        onClose={vi.fn()}
        onChanged={vi.fn()}
        onEdit={vi.fn()}
      />,
    );
  });

  // Desativar e tocar novamente na chave antes de o pai recarregar deve enviar false e depois true.
  const toggle = () => container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_enabled()}"]`)!;
  await act(async () => { toggle().click(); });
  await act(async () => { toggle().click(); });
  expect(mocks.updateIntegration).toHaveBeenNthCalledWith(1, srv, 'notion', { enabled: false });
  expect(mocks.updateIntegration).toHaveBeenNthCalledWith(2, srv, 'notion', { enabled: true });

  // Mudar política quando listIntegrationTools falha deve remover ações antigas de ask_write.
  mocks.updateIntegration.mockResolvedValueOnce(oauthView({ status: 'ready', policy: 'read_only' }));
  mocks.listIntegrationTools.mockRejectedValueOnce(new Error('falha de rede'));
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_policy_read_only()}"]`)!.click();
  });
  expect(container.querySelector(`button[aria-label="${m.integrations_tool_allow_always()} create_issue"]`)).toBeNull();
  expect(mocks.registrarDiag).toHaveBeenCalledWith(
    expect.objectContaining({ evento: 'integrations.tools_refresh_failed', tela: 'integrations' }),
    srv.baseUrl,
  );

  // Abrir OAuth em auth_required mostra faixa e registra no diário; quando status vira ready, a faixa some.
  mocks.listIntegrationTools.mockResolvedValue([]);
  mocks.startIntegrationOAuth.mockResolvedValueOnce({
    mode: 'open_url',
    authorize_url: 'https://mcp.notion.com/authorize?state=ok',
    redirect_uri: 'https://a.test/api/integrations/oauth/callback',
  });
  mocks.openURL.mockResolvedValueOnce(undefined);
  await act(async () => {
    root.render(
      <IntegrationDetailSheet
        key="oauth-ready-clear"
        server={srv}
        integration={oauthView({ status: 'auth_required' })}
        onClose={vi.fn()}
        onChanged={vi.fn()}
        onEdit={vi.fn()}
      />,
    );
  });
  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_authorize()}"]`)!.click();
  });
  expect(container.textContent).toContain(m.integrations_oauth_started());
  expect(mocks.registrarDiag).toHaveBeenCalledWith(
    expect.objectContaining({ evento: 'integrations.oauth_browser_opened', tela: 'integrations' }),
    srv.baseUrl,
  );
  await act(async () => {
    root.render(
      <IntegrationDetailSheet
        key="oauth-ready-clear"
        server={srv}
        integration={oauthView({ status: 'ready' })}
        onClose={vi.fn()}
        onChanged={vi.fn()}
        onEdit={vi.fn()}
      />,
    );
  });
  expect(container.textContent).not.toContain(m.integrations_oauth_started());
});

it('tela encerra loading quando silencioso substitui manual, fecha detalhe removido e isola troca de servidor', async () => {
  const srvA = bucket('a').server;
  const srvB = bucket('b').server;
  const itemA = oauthView({ id: 'only-a', name: 'Só em A', auth: { kind: 'none' } });
  mocks.listIntegrations.mockResolvedValueOnce([itemA]);
  mocks.listCatalog.mockResolvedValueOnce([]);
  mocks.listIntegrationTools.mockResolvedValue([]);
  useServers.setState({ servers: [srvA, srvB], activeId: 'a', ready: true, vaultReadable: true });

  await act(async () => { root.render(<IntegrationsScreen />); });
  await act(async () => {
    container.querySelector<HTMLButtonElement>('button[aria-label="Só em A"]')!.click();
  });
  expect(container.querySelector('section[aria-label="Só em A"]')).not.toBeNull();

  // Refresh manual seguido de refresh silencioso: o silencioso encerra o loading e fecha detalhe se o item sumiu.
  let resolveManual!: (v: IntegrationView[]) => void;
  let resolveSilent!: (v: IntegrationView[]) => void;
  mocks.listIntegrations
    .mockImplementationOnce(() => new Promise((r) => { resolveManual = r; }))
    .mockImplementationOnce(() => new Promise((r) => { resolveSilent = r; }));
  mocks.listCatalog.mockResolvedValue([]);

  await act(async () => {
    container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_refresh()}"]`)!.click();
  });
  const refreshBtn = () => container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_refresh()}"]`)!;
  expect(refreshBtn().disabled).toBe(true);

  await act(async () => { mocks.appStateListener?.('active'); });
  await act(async () => { resolveManual([itemA]); });
  await act(async () => { resolveSilent([]); });
  expect(refreshBtn().disabled).toBe(false);
  expect(container.querySelector('section[aria-label="Só em A"]')).toBeNull();

  // Reabre detalhe em A e troca para B: nunca chama listIntegrationTools(srvB, 'only-a').
  mocks.listIntegrations.mockResolvedValueOnce([itemA]);
  mocks.listCatalog.mockResolvedValueOnce([]);
  await act(async () => { refreshBtn().click(); });
  await act(async () => {
    container.querySelector<HTMLButtonElement>('button[aria-label="Só em A"]')!.click();
  });
  mocks.listIntegrationTools.mockClear();
  mocks.listIntegrations.mockResolvedValueOnce([]);
  mocks.listCatalog.mockResolvedValueOnce([]);
  await act(async () => { useServers.setState({ activeId: 'b' }); });
  expect(mocks.listIntegrationTools).not.toHaveBeenCalledWith(srvB, 'only-a');
});
