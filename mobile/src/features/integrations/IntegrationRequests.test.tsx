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
import { openIntegrationApproval, useIntegrationApprovalTarget } from './target';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';

const mocks = vi.hoisted(() => ({
  answerIntegrationApproval: vi.fn(),
  createIntegration: vi.fn(),
  updateIntegration: vi.fn(),
  deleteIntegration: vi.fn(),
  startIntegrationOAuth: vi.fn(),
  openIntegrationOAuthOnDesktop: vi.fn(),
  testIntegration: vi.fn(),
  listIntegrationTools: vi.fn(),
  openURL: vi.fn(),
  toastOk: vi.fn(),
  toastErro: vi.fn(),
}));

vi.mock('expo-linking', () => ({ openURL: mocks.openURL }));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  answerIntegrationApproval: mocks.answerIntegrationApproval,
  createIntegration: mocks.createIntegration,
  updateIntegration: mocks.updateIntegration,
  deleteIntegration: mocks.deleteIntegration,
  startIntegrationOAuth: mocks.startIntegrationOAuth,
  openIntegrationOAuthOnDesktop: mocks.openIntegrationOAuthOnDesktop,
  testIntegration: mocks.testIntegration,
  listIntegrationTools: mocks.listIntegrationTools,
}));
vi.mock('../machines/parts', () => ({
  MachineSheet: ({ title, children }: { title: string; children: ReactNode }) =>
    createElement('section', { 'aria-label': title }, children),
  Notice: ({ text }: { text: string }) => text || null,
}));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('../../ui/Icon', () => ({ Icon: ({ name }: { name: string }) => createElement('i', { 'data-icon': name }) }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: mocks.toastOk, erro: mocks.toastErro } }));
vi.mock('../pair/identities', () => ({ useIdentities: () => ({ identities: new Map([['a', 'a']]), loading: new Set() }) }));

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
  container = document.createElement('div');
  root = createRoot(container);
  openIntegrationApproval(null);
  useSessions.setState({ integrationApprovals: { a: makeEvent('r1') } });
});

afterEach(async () => {
  await act(async () => { root.unmount(); });
  useSessions.setState({ integrationApprovals: {}, byServer: [] });
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
});

it('pedido expirado ou respondido fora (approval_conflict / approval_lost) descarta cartão e mostra erro', async () => {
  useSessions.setState({ byServer: [bucket('a')], integrationApprovals: { a: makeEvent('stale-1') } });
  mocks.answerIntegrationApproval.mockRejectedValueOnce(Object.assign(new Error('409: já respondido'), { status: 409, code: 'approval_answered' }));
  openIntegrationApproval({ serverId: 'a', requestId: 'stale-1' });
  await act(async () => { root.render(<IntegrationRequestSheet />); });

  const button = container.querySelector<HTMLButtonElement>(`button[aria-label="${m.integrations_approval_allow_once()}"]`);
  await act(async () => { button!.click(); });

  expect(mocks.toastErro).toHaveBeenCalledWith(m.integrations_err_answered());
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

  expect(mocks.updateIntegration).toHaveBeenCalledWith(srv, 'notion', { allowed_tools: ['create_issue'] });
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
