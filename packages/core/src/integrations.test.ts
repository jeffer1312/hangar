import { afterEach, describe, expect, it, vi } from 'vitest';
import * as m from './paraglide/messages';
import {
  RESERVED_HTTP_HEADERS,
  answerIntegrationApproval,
  cancelIntegrationApproval,
  createIntegration,
  deleteIntegration,
  formatCommandArgs,
  integrationFailureText,
  isValidHeaderName,
  isValidIntegrationUrl,
  listCatalog,
  listIntegrationTools,
  listIntegrations,
  openIntegrationOAuthOnDesktop,
  parseCommandArgs,
  parseEnvVars,
  parseHeaders,
  parseIntegrationApprovalEvent,
  parseLines,
  parseScopes,
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

describe('updateIntegration delta de allowed_tools', () => {
  it('envia add_allowed_tools e remove_allowed_tools mantendo allowed_tools opcional', async () => {
    const view = sampleView();
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response(JSON.stringify({ integration: { ...view, allowed_tools: ['create_issue'] } })))
      .mockResolvedValueOnce(new Response(JSON.stringify({ integration: view })));

    await updateIntegration(server, 'github', { add_allowed_tools: ['create_issue'] });
    expect(JSON.parse(String(fetchMock.mock.calls[0][1]?.body))).toEqual({
      add_allowed_tools: ['create_issue'],
    });

    await updateIntegration(server, 'github', { remove_allowed_tools: ['create_issue'] });
    expect(JSON.parse(String(fetchMock.mock.calls[1][1]?.body))).toEqual({
      remove_allowed_tools: ['create_issue'],
    });
  });
});

describe('parsers e validadores compartilhados de integrações', () => {
  it('parseLines e parseScopes normalizam linhas e preservam vírgulas em escopos OAuth', () => {
    expect(parseLines('  proj-a \r\n\r\n  proj-b\n  ')).toEqual(['proj-a', 'proj-b']);
    expect(parseLines('  \n \r\n ')).toEqual([]);

    expect(parseScopes('openid  profile\temail\nread:user reports,read')).toEqual([
      'openid',
      'profile',
      'email',
      'read:user',
      'reports,read',
    ]);
    expect(parseScopes('  \t \n ')).toEqual([]);
  });

  it('isValidHeaderName segue tchar RFC 9110 e rejeita cabeçalhos reservados', () => {
    expect(isValidHeaderName('X-Api-Key')).toBe(true);
    expect(isValidHeaderName("x-custom_1!#$%&'*+-.^_`|~")).toBe(true);
    expect(isValidHeaderName('')).toBe(false);
    expect(isValidHeaderName('a'.repeat(257))).toBe(false);
    expect(isValidHeaderName('X(Key)')).toBe(false);
    expect(isValidHeaderName('X Key')).toBe(false);
    expect(isValidHeaderName('X:Key')).toBe(false);
    expect(isValidHeaderName('X-Café')).toBe(false);

    for (const reserved of RESERVED_HTTP_HEADERS) {
      expect(isValidHeaderName(reserved)).toBe(false);
    }
    expect(isValidHeaderName('Authorization')).toBe(false);
    expect(isValidHeaderName('Mcp-Session-Id')).toBe(false);
  });

  it('parseHeaders diferencia linha malformada, nome inválido/reservado e valor inválido', () => {
    expect(parseHeaders('')).toEqual({ ok: true, names: [], map: {} });
    expect(parseHeaders('  X-Api-Key: secret-1 \r\nX-Tenant:  acme\t1 \nX-Api-Key: secret-2')).toEqual({
      ok: true,
      names: ['x-api-key', 'x-tenant'],
      map: { 'x-api-key': 'secret-2', 'x-tenant': 'acme\t1' },
    });

    expect(parseHeaders('sem-separador')).toEqual({ ok: false, reason: 'invalid_line' });
    expect(parseHeaders(': valor')).toEqual({ ok: false, reason: 'invalid_line' });
    expect(parseHeaders('   : valor')).toEqual({ ok: false, reason: 'invalid_line' });
    expect(parseHeaders('X(Key): valor')).toEqual({ ok: false, reason: 'invalid_name', name: 'x(key)' });
    expect(parseHeaders('Authorization: Bearer x')).toEqual({ ok: false, reason: 'reserved_name', name: 'authorization' });
    expect(parseHeaders('Mcp-Protocol-Version: 2025-03-26')).toEqual({
      ok: false,
      reason: 'reserved_name',
      name: 'mcp-protocol-version',
    });
    expect(parseHeaders('X-Empty:   ')).toEqual({ ok: false, reason: 'invalid_value', name: 'x-empty' });
    expect(parseHeaders('X-Ctrl: ok\u0000bad')).toEqual({ ok: false, reason: 'invalid_value', name: 'x-ctrl' });

    const protoHeader = parseHeaders('__proto__: segredo\nX-Normal: ok');
    expect(protoHeader.ok).toBe(true);
    if (protoHeader.ok) {
      expect(Object.getPrototypeOf(protoHeader.map)).toBeNull();
      expect(protoHeader.names).toEqual(['__proto__', 'x-normal']);
      expect(JSON.stringify(protoHeader.map)).toBe('{"__proto__":"segredo","x-normal":"ok"}');
    }
  });

  it('parseEnvVars preserva espaços finais do valor e remove só \\r de CRLF', () => {
    expect(parseEnvVars('  \n\r\n')).toEqual({ ok: true, keys: [], map: {} });
    expect(parseEnvVars('LABEL=production \r\n  SPACED=  mid  \nEMPTY=\nEXPR=a=b=c')).toEqual({
      ok: true,
      keys: ['LABEL', 'SPACED', 'EMPTY', 'EXPR'],
      map: {
        LABEL: 'production ',
        SPACED: '  mid  ',
        EMPTY: '',
        EXPR: 'a=b=c',
      },
    });

    const protoEnv = parseEnvVars('__proto__=valor\nNORMAL=1');
    expect(protoEnv.ok).toBe(true);
    if (protoEnv.ok) {
      expect(Object.getPrototypeOf(protoEnv.map)).toBeNull();
      expect(protoEnv.keys).toEqual(['__proto__', 'NORMAL']);
      expect(JSON.stringify(protoEnv.map)).toBe('{"__proto__":"valor","NORMAL":"1"}');
    }

    expect(parseEnvVars('SEM_IGUAL')).toEqual({ ok: false, reason: 'invalid_line' });
    expect(parseEnvVars('=sem-chave')).toEqual({ ok: false, reason: 'invalid_line' });
    expect(parseEnvVars('  =sem-chave')).toEqual({ ok: false, reason: 'invalid_line' });
    expect(parseEnvVars('CHAVE INVALIDA=1')).toEqual({ ok: false, reason: 'invalid_key', key: 'CHAVE INVALIDA' });
    expect(parseEnvVars('BAD=val\u0000x')).toEqual({ ok: false, reason: 'invalid_value', key: 'BAD' });
  });

  it('formatCommandArgs e parseCommandArgs fazem ida e volta sem perda e rejeitam aspas abertas', () => {
    const roundTrips: string[][] = [
      [],
      ['npx', '-y', '@modelcontextprotocol/server-filesystem'],
      ['/opt/My Tools/server'],
      ['/opt/My Tools/server', '--label', 'hello world', 'a"b', "c'd"],
      ['C:\\Program Files\\tool\\mcp.exe', 'C:\\foo\\bar', 'end\\'],
      ['node', '-e', 'console.log("hi")'],
      ['cmd', ''],
      ['sh', '-c', 'line1\nline2'],
    ];
    for (const command of roundTrips) {
      const formatted = formatCommandArgs(command);
      expect(parseCommandArgs(formatted)).toEqual({ ok: true, args: command });
    }

    expect(parseCommandArgs('  ')).toEqual({ ok: true, args: [] });
    expect(parseCommandArgs(`--flag="my val" 'single arg' escaped\\ space`)).toEqual({
      ok: true,
      args: ['--flag=my val', 'single arg', 'escaped space'],
    });
    expect(parseCommandArgs('srv arg\\ ')).toEqual({ ok: true, args: ['srv', 'arg '] });
    expect(parseCommandArgs('srv arg   ')).toEqual({ ok: true, args: ['srv', 'arg'] });
    expect(parseCommandArgs('srv "arg "')).toEqual({ ok: true, args: ['srv', 'arg '] });
    expect(parseCommandArgs('npx -y server\n')).toEqual({ ok: true, args: ['npx', '-y', 'server'] });
    expect(parseCommandArgs('\n  npx -y server \n')).toEqual({ ok: true, args: ['npx', '-y', 'server'] });
    expect(parseCommandArgs('C:\\tools\\mcp.exe --port 3000')).toEqual({
      ok: true,
      args: ['C:\\tools\\mcp.exe', '--port', '3000'],
    });
    expect(parseCommandArgs('/opt/My Tools/server\r\n--dir\r\n/Users/me/My Docs')).toEqual({
      ok: true,
      args: ['/opt/My Tools/server', '--dir', '/Users/me/My Docs'],
    });
    expect(parseCommandArgs('npx "unclosed')).toEqual({ ok: false, unclosedQuote: '"' });
    expect(parseCommandArgs("npx 'unclosed")).toEqual({ ok: false, unclosedQuote: "'" });
    expect(parseCommandArgs('npx "escaped\\"')).toEqual({ ok: false, unclosedQuote: '"' });
    expect(parseCommandArgs('npx\n"unclosed')).toEqual({ ok: false, unclosedQuote: '"' });
  });

  it('isValidIntegrationUrl exige HTTPS ou loopback em HTTP quando há autenticação ou cabeçalhos', () => {
    expect(isValidIntegrationUrl('https://mcp.example.com/mcp', { hasAuthOrHeaders: true })).toEqual({ valid: true });
    expect(isValidIntegrationUrl('http://mcp.example.com/mcp', { hasAuthOrHeaders: false })).toEqual({ valid: true });

    for (const loopback of [
      'http://localhost/mcp',
      'http://localhost:8080/mcp',
      'http://127.0.0.1:3000/mcp',
      'http://[::1]:3000/mcp',
    ]) {
      expect(isValidIntegrationUrl(loopback, { hasAuthOrHeaders: true })).toEqual({ valid: true });
    }

    expect(isValidIntegrationUrl('http://mcp.example.com/mcp', { hasAuthOrHeaders: true })).toEqual({
      valid: false,
      insecure: true,
    });
    expect(isValidIntegrationUrl('http://192.168.1.50:8080/mcp', { hasAuthOrHeaders: true })).toEqual({
      valid: false,
      insecure: true,
    });

    expect(isValidIntegrationUrl('', { hasAuthOrHeaders: false })).toEqual({ valid: false });
    expect(isValidIntegrationUrl('ftp://mcp.example.com/mcp', { hasAuthOrHeaders: false })).toEqual({ valid: false });
    expect(isValidIntegrationUrl('https://user:pass@mcp.example.com/mcp', { hasAuthOrHeaders: false })).toEqual({
      valid: false,
    });
    expect(isValidIntegrationUrl('https://mcp.example.com/a b', { hasAuthOrHeaders: false })).toEqual({ valid: false });
    expect(isValidIntegrationUrl('https://mcp.example.com:0/mcp', { hasAuthOrHeaders: false })).toEqual({ valid: false });
  });
});
