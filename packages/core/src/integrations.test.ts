import { afterEach, describe, expect, it, vi } from 'vitest';
import * as m from './paraglide/messages';
import {
  answerIntegrationApproval,
  cancelIntegrationApproval,
  createIntegration,
  deleteIntegration,
  integrationFailureText,
  listCatalog,
  listIntegrationTools,
  listIntegrations,
  openIntegrationOAuthOnDesktop,
  parseIntegrationApprovalEvent,
  resolveMobilePublicBaseUrl,
  startIntegrationOAuth,
  testIntegration,
  updateIntegration,
  type IntegrationApprovalRequest,
  type IntegrationPreset,
  type IntegrationToolSummary,
  type IntegrationView,
} from './integrations';

const server = { id: 'a', label: 'Servidor A', baseUrl: 'https://a.test', token: 'token-a' };

const sampleView = (): IntegrationView => ({
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
});

const sampleTool = (): IntegrationToolSummary => ({
  name: 'list_messages',
  description: 'Lista e-mails',
  input_schema: { type: 'object' },
  read_only_hint: true,
  effective_access: 'read',
});

const sampleCard = (extra: Partial<IntegrationApprovalRequest> = {}): IntegrationApprovalRequest => ({
  id: 'req-1',
  interaction_id: 'int-1',
  origin: 'pc::sessao-1',
  integration_id: 'github',
  integration_name: 'GitHub',
  tool: 'create_issue',
  arguments_preview: '{"title":"Bug"}',
  reason: 'Abrir issue',
  expires_at: 9999999,
  ...extra,
});

afterEach(() => vi.restoreAllMocks());

describe('clientes HTTP de integrações', () => {
  it('listCatalog busca presets do catálogo', async () => {
    const preset: IntegrationPreset = {
      id: 'notion',
      name: 'Notion',
      category: 'productivity',
      description: 'Páginas e bases',
      url: 'https://mcp.notion.com/mcp',
      url_editable: false,
      auth_kind: 'oauth_dcr',
      default_scopes: [],
      requires_loopback_redirect: false,
      setup_guide: null,
    };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(new Response(JSON.stringify({ presets: [preset] })));
    await expect(listCatalog(server)).resolves.toEqual([preset]);
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/integrations/catalog');
    expect(fetchMock.mock.calls[0][1]?.method ?? 'GET').toBe('GET');
  });

  it('listIntegrations, createIntegration, updateIntegration e deleteIntegration respeitam rotas, corpo e prazo de keyring', async () => {
    const view = sampleView();
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ integrations: [view] })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ integration: view }), { status: 201 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ integration: { ...view, pinned: true } })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, id: 'a/b c' })));

    await expect(listIntegrations(server)).resolves.toEqual([view]);
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/integrations');

    const created = await createIntegration(server, {
      id: view.id,
      name: view.name,
      preset: view.preset,
      transport: { kind: 'remote_http', url: 'https://gmailmcp.googleapis.com/mcp/v1' },
      auth: view.auth,
      secrets: { oauth_client_secret: 'sec-1' },
    });
    expect(created).toEqual(view);
    expect(fetchMock.mock.calls[1][1]?.method).toBe('POST');
    expect(JSON.parse(String(fetchMock.mock.calls[1][1]?.body))).toMatchObject({
      id: view.id,
      secrets: { oauth_client_secret: 'sec-1' },
    });

    const updated = await updateIntegration(server, 'a/b c', { pinned: true });
    expect(updated.pinned).toBe(true);
    expect(fetchMock.mock.calls[2][0]).toBe('https://a.test/api/integrations/a%2Fb%20c');
    expect(fetchMock.mock.calls[2][1]?.method).toBe('PATCH');

    await expect(deleteIntegration(server, 'a/b c')).resolves.toEqual({ ok: true, id: 'a/b c' });
    expect(fetchMock.mock.calls[3][0]).toBe('https://a.test/api/integrations/a%2Fb%20c');
    expect(fetchMock.mock.calls[3][1]?.method).toBe('DELETE');
    expect(timeout).toHaveBeenCalledWith(45_000);
  });

  it('startIntegrationOAuth e openIntegrationOAuthOnDesktop atendem mobile open_url e handoff_desktop', async () => {
    const handoff = {
      mode: 'handoff_desktop' as const,
      authorize_url: 'https://accounts.google.com/o/oauth2/v2/auth?state=abc',
      redirect_uri: 'http://127.0.0.1:8765/api/integrations/oauth/callback',
      reason: 'este provedor exige redirecionamento em localhost',
    };
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify(handoff)))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true })));

    await expect(startIntegrationOAuth(server, 'gmail-pessoal', {
      origin_client: 'mobile',
      public_base_url: 'https://a.test',
    })).resolves.toEqual(handoff);
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/integrations/gmail-pessoal/oauth/start');
    expect(JSON.parse(String(fetchMock.mock.calls[0][1]?.body))).toEqual({
      origin_client: 'mobile',
      public_base_url: 'https://a.test',
    });

    await expect(openIntegrationOAuthOnDesktop(server, 'gmail-pessoal', handoff.authorize_url)).resolves.toEqual({ ok: true });
    expect(fetchMock.mock.calls[1][0]).toBe('https://a.test/api/integrations/gmail-pessoal/oauth/open-on-desktop');
    expect(JSON.parse(String(fetchMock.mock.calls[1][1]?.body))).toEqual({ authorize_url: handoff.authorize_url });
  });

  it('testIntegration trata 200 e 424 auth_required/auth_expired sem perder falhas reais', async () => {
    const tool = sampleTool();
    vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ status: 'ready', tools: [tool] })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ detail: { code: 'auth_required', params: {}, msg: 'precisa autenticar' } }), { status: 424 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ detail: { code: 'auth_expired', params: {}, msg: 'token expirado' } }), { status: 424 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ detail: { code: 'integration_disabled', params: {}, msg: 'desabilitada' } }), { status: 409 }));

    await expect(testIntegration(server, 'notion')).resolves.toEqual({ status: 'ready', tools: [tool] });
    await expect(testIntegration(server, 'notion')).resolves.toEqual({ status: 'auth_required', tools: [], reason: 'precisa autenticar' });
    await expect(testIntegration(server, 'notion')).resolves.toEqual({ status: 'auth_expired', tools: [], reason: 'token expirado' });
    await expect(testIntegration(server, 'notion')).rejects.toMatchObject({ status: 409, code: 'integration_disabled' });
  });

  it('listIntegrationTools, answerIntegrationApproval e cancelIntegrationApproval usam os caminhos corretos', async () => {
    const tool = sampleTool();
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ tools: [tool] })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, decision: 'allow_always' })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ ok: true })));

    await expect(listIntegrationTools(server, 'notion')).resolves.toEqual([tool]);
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/integrations/notion/tools');

    await expect(answerIntegrationApproval(server, 'req-1', { interaction_id: 'int-1', decision: 'allow_always' })).resolves.toEqual({
      ok: true,
      decision: 'allow_always',
    });
    expect(fetchMock.mock.calls[1][0]).toBe('https://a.test/api/integrations/approvals/req-1');
    expect(JSON.parse(String(fetchMock.mock.calls[1][1]?.body))).toEqual({
      interaction_id: 'int-1',
      decision: 'allow_always',
    });

    await expect(cancelIntegrationApproval(server, 'req-1')).resolves.toEqual({ ok: true });
    expect(fetchMock.mock.calls[2][0]).toBe('https://a.test/api/integrations/approvals/req-1');
    expect(fetchMock.mock.calls[2][1]?.method).toBe('DELETE');
  });
});

describe('resolveMobilePublicBaseUrl', () => {
  it('escolhe remote HTTPS antes de connect e baseUrl HTTPS e ignora URLs HTTP ou inválidas', () => {
    expect(resolveMobilePublicBaseUrl({
      baseUrl: 'https://base.example.com/',
      remote: { url: 'https://tail.ts.net/', id: 'pc' },
      connect: { url: 'https://pc1.acme.hangar.dev.br/', id: 'pc' },
    })).toBe('https://tail.ts.net');

    expect(resolveMobilePublicBaseUrl({
      baseUrl: 'http://192.168.1.5:8765',
      connect: { url: 'https://pc1.acme.hangar.dev.br/' },
    })).toBe('https://pc1.acme.hangar.dev.br');

    expect(resolveMobilePublicBaseUrl({
      baseUrl: 'https://base.example.com/',
      remote: { url: 'http://100.64.0.2:8765', id: 'pc' },
      connect: { url: 'https://user:pass@evil.example.com/', id: 'pc' },
    })).toBe('https://base.example.com');

    expect(resolveMobilePublicBaseUrl({
      baseUrl: 'http://192.168.1.5:8765',
      remote: { url: 'http://100.64.0.2:8765', id: 'pc' },
      connect: { url: 'http://pc1.local:8765', id: 'pc' },
    })).toBeUndefined();

    expect(resolveMobilePublicBaseUrl({
      baseUrl: 'https://user:pass@evil.example.com',
    })).toBeUndefined();
  });
});

describe('parseIntegrationApprovalEvent', () => {
  it('lê snapshot válido e descarta somente cartões malformados', () => {
    const valid = sampleCard();
    const raw = JSON.stringify({
      requests: [
        valid,
        { id: '', interaction_id: 'x' },
        { id: 'r2', interaction_id: 'i2', origin: 'pc::s', integration_id: 'g', integration_name: 'G', tool: 't', arguments_preview: '{}', reason: 'r', expires_at: Number.NaN },
      ],
    });
    expect(parseIntegrationApprovalEvent(raw)).toEqual({ requests: [valid] });
  });

  it('rejeita payloads de topo malformados retornando null', () => {
    expect(parseIntegrationApprovalEvent('{')).toBeNull();
    expect(parseIntegrationApprovalEvent('[]')).toBeNull();
    expect(parseIntegrationApprovalEvent('null')).toBeNull();
    expect(parseIntegrationApprovalEvent(JSON.stringify({ requests: 'nope' }))).toBeNull();
    expect(parseIntegrationApprovalEvent(JSON.stringify({}))).toBeNull();
  });
});

describe('integrationFailureText', () => {
  it('mapeia todos os códigos conhecidos e ignora desconhecidos e propriedades herdadas', () => {
    expect(integrationFailureText({ code: 'auth_required' })).toBe(m.integrations_err_auth_required());
    expect(integrationFailureText({ code: 'auth_expired' })).toBe(m.integrations_err_auth_expired());
    expect(integrationFailureText({ code: 'integration_owner_only' })).toBe(m.integrations_err_owner_only());
    expect(integrationFailureText({ code: 'integration_disabled' })).toBe(m.integrations_err_disabled());
    expect(integrationFailureText({ code: 'integration_already_exists' })).toBe(m.integrations_err_already_exists());
    expect(integrationFailureText({ code: 'integration_secrets_key_lost' })).toBe(m.integrations_err_keyring());
    expect(integrationFailureText({ code: 'integration_secrets_unavailable' })).toBe(m.integrations_err_keyring());
    expect(integrationFailureText({ code: 'approval_stale' })).toBe(m.integrations_err_stale());
    expect(integrationFailureText({ code: 'approval_answered' })).toBe(m.integrations_err_answered());
    expect(integrationFailureText({ code: 'approval_lost' })).toBe(m.integrations_approval_lost());
    expect(integrationFailureText({ code: 'approval_not_found' })).toBe(m.integrations_approval_lost());
    expect(integrationFailureText({ code: 'approval_conflict', message: '409: o cartão de aprovação foi atualizado; tente novamente' })).toBe(m.integrations_err_stale());
    expect(integrationFailureText({ code: 'approval_conflict', message: '409: este pedido de aprovação já foi respondido' })).toBe(m.integrations_err_answered());
    expect(integrationFailureText({ code: 'toString' })).toBeUndefined();
    expect(integrationFailureText({ code: 'unknown_code' })).toBeUndefined();
    expect(integrationFailureText(null)).toBeUndefined();
  });
});
