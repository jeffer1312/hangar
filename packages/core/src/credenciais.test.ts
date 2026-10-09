import { beforeEach, afterEach, describe, it, expect, vi } from 'vitest';
import { credentialAuth, credentialGroup, codexAccountMessage, contaCodexParaEntrar, codexCliAusente, defaultCodexAccount,
  type Credencial, type CodexAccount } from './credenciais';
import { configureApi } from './apiEnv';
import { configureLocale } from './i18n';
import { overwriteGetLocale } from './paraglide/runtime';
import { formataErro } from './errosApi';
import { _limparEsfriamentoParaTestes } from './esfriamento';
import { getCredentialsForServer, listarCredenciais, getCodexAccountsForServer, createCodexAccountForServer,
  prepareCodexAccountForServer, getCodexPreparationForServer, startCodexAccountLoginForServer,
  getCodexAccountLoginForServer, cancelCodexAccountLoginForServer, deleteCodexAccountForServer,
  consumeCodexRateLimitResetForServer,
  createSessionForServer, createSession,
  modelOptionsForServer, modelOptions, getArchiveHistory, getArchivePorCwd, resumeArchivedConversation,
  passarBastao, errorDetail } from './api';

const server = { id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'token-b' };
const unauthorized = vi.fn();
beforeEach(() => {
  configureApi({ getBaseUrl: () => 'https://a.test', getToken: () => 'token-a',
    onUnauthorized: unauthorized, origin: null, createEventSource: () => { throw new Error('unused'); } });
  unauthorized.mockClear();
  overwriteGetLocale(() => 'pt');
  configureLocale({ getLocale: () => 'pt' });
});
afterEach(() => {
  vi.restoreAllMocks();
  // Timeout simulado esfria o servidor B e contaminaria os testes seguintes.
  _limparEsfriamentoParaTestes();
});

describe('Codex não instalado', () => {
  it('é lido da credencial e da conta, que chegam por rotas diferentes', () => {
    expect(codexCliAusente({ tipo: 'codex', login: { estado: 'indisponivel', motivo: 'cli-ausente' } } as Credencial)).toBe(true);
    expect(codexCliAusente({ tipo: 'codex', login: { estado: 'indisponivel' } } as Credencial)).toBe(false);
    expect(codexCliAusente({ auth: { method: 'unknown', status: 'unavailable', email: null, plan: null, reason: 'cli_missing' } } as CodexAccount)).toBe(true);
    expect(codexCliAusente({ auth: { method: 'none', status: 'disconnected', email: null, plan: null } } as CodexAccount)).toBe(false);
  });
});

describe('autenticação explícita', () => {
  it('separa autenticação de provedor', () => {
    expect(credentialGroup({ tipo: 'codex', auth_method: 'oauth' } as Credencial)).toBe('subscription');
    expect(credentialGroup({ tipo: 'codex', auth_method: 'api_key' } as Credencial)).toBe('api_key');
    expect(credentialAuth({ tipo: 'chave', id: 'codex:/old' } as Credencial)).toBe('unknown');
    expect(credentialGroup({ tipo: 'codex', auth_method: 'none' } as Credencial)).toBe('unknown');
    expect(credentialGroup({ tipo: 'chave', auth_method: 'api_key', usos: ['claude_code'] } as Credencial)).toBe('claude_engine');
  });
});

describe('contas e servidor explícito', () => {
  it.each(['active', 'server', 'baton'])('mostra avisos sem perder a sessão criada (%s)', async (source) => {
    const warnings = ['preferencia-nao-salva'];
    const notify = vi.fn();
    configureApi({ getBaseUrl: () => 'https://a.test', getToken: () => 'token-a',
      onUnauthorized: unauthorized, onSessionWarnings: notify, origin: null,
      createEventSource: () => { throw new Error('unused'); } });
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ name: 'created', avisos: warnings }));
    const result = source === 'active' ? await createSession('created')
      : source === 'server' ? await createSessionForServer(server, { name: 'created', provider: 'codex' })
        : await passarBastao('origin', { name: 'created' }, server);
    expect(result.name).toBe('created');
    expect(notify).toHaveBeenCalledWith(warnings);
  });

  it('falha ao exibir aviso não transforma criação confirmada em falha', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {});
    configureApi({ getBaseUrl: () => 'https://a.test', getToken: () => 'token-a',
      onUnauthorized: unauthorized, onSessionWarnings: () => { throw new Error('display failed'); }, origin: null,
      createEventSource: () => { throw new Error('unused'); } });
    const fetcher = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ name: 'created', avisos: ['preferencia-nao-salva'] }));
    await expect(createSession('created')).resolves.toMatchObject({ name: 'created' });
    expect(fetcher).toHaveBeenCalledTimes(1);
    expect(error).toHaveBeenCalled();
  });

  it('propaga o cancelamento ao carregar credenciais de outro servidor', async () => {
    const controller = new AbortController();
    const fetcher = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json([]));

    await getCredentialsForServer(server, false, controller.signal);

    const signal = fetcher.mock.calls[0][1]?.signal;
    expect(signal?.aborted).toBe(false);
    controller.abort();
    expect(signal?.aborted).toBe(true);
  });

  it('serializa todos os endpoints com token B e prazo', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => Response.json({}));
    await getCredentialsForServer(server, true);
    await getCodexAccountsForServer(server);
    await createCodexAccountForServer(server, 'work');
    await prepareCodexAccountForServer(server, 'work /', true);
    await getCodexPreparationForServer(server, 'work /');
    await startCodexAccountLoginForServer(server, 'work /');
    await getCodexAccountLoginForServer(server, 'work /');
    await cancelCodexAccountLoginForServer(server, 'work /', 'attempt /?');
    expect(fetcher.mock.calls.map(([url]) => url)).toEqual([
      'https://b.test/api/credenciais?forcar=true', 'https://b.test/api/codex-contas',
      'https://b.test/api/codex-contas', 'https://b.test/api/codex-contas/work%20%2F/prepare?forcar=true',
      'https://b.test/api/codex-contas/work%20%2F/prepare', 'https://b.test/api/codex-contas/work%20%2F/login',
      'https://b.test/api/codex-contas/work%20%2F/login',
      'https://b.test/api/codex-contas/work%20%2F/login?attempt_id=attempt%20%2F%3F',
    ]);
    for (const [, init] of fetcher.mock.calls) {
      expect(init?.headers).toMatchObject({ Authorization: 'Bearer token-b', 'Content-Type': 'application/json' });
      expect(init?.signal).toBeInstanceOf(AbortSignal);
    }
    expect(fetcher.mock.calls[2][1]).toMatchObject({ method: 'POST', body: '{"name":"work"}' });
    expect(fetcher.mock.calls[7][1]?.method).toBe('DELETE');
  });

  it('propaga cancelamento sem remover o prazo', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => Response.json([]));
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    const controller = new AbortController();
    await getCodexAccountsForServer(server, controller.signal);
    const signal = fetcher.mock.calls[0][1]?.signal;
    controller.abort('changed');
    expect(signal?.aborted).toBe(true);
    expect(signal?.reason).toBe('changed');
    expect(timeout).toHaveBeenCalledWith(8000);
  });

  it('apagar conta Codex espera a remoção da pasta, que passa de 8s', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => Response.json({ ok: true }));
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    await deleteCodexAccountForServer(server, 'work');
    expect(fetcher.mock.calls[0][0]).toBe('https://b.test/api/codex-contas/work?keep_transcripts=1');
    await deleteCodexAccountForServer(server, 'work', false);
    expect(fetcher.mock.calls[1][0]).toBe('https://b.test/api/codex-contas/work?keep_transcripts=0');
    expect(fetcher.mock.calls[0][1]?.method).toBe('DELETE');
    expect(timeout).toHaveBeenCalledWith(120_000);
    expect(timeout).not.toHaveBeenCalledWith(8000);
  });

  it('consome uma redefinição Codex com id e chave idempotente', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      Response.json({ outcome: 'reset' }));
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    const key = '123e4567-e89b-12d3-a456-426614174000';

    await consumeCodexRateLimitResetForServer(server, 'work /', 'credit /', key);

    expect(fetcher.mock.calls[0][0]).toBe(
      'https://b.test/api/codex-contas/work%20%2F/rate-limit-reset');
    expect(fetcher.mock.calls[0][1]).toMatchObject({
      method: 'POST',
      body: JSON.stringify({ credit_id: 'credit /', idempotency_key: key }),
    });
    expect(timeout).toHaveBeenCalledWith(90_000);
    expect(timeout).not.toHaveBeenCalledWith(8000);
  });

  it('prazo estourado cita o prazo que valeu, não 8s fixo', async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new DOMException('signal timed out', 'TimeoutError'));
    await expect(deleteCodexAccountForServer(server, 'work')).rejects.toThrow('B não respondeu em 120s');
    _limparEsfriamentoParaTestes();
    await expect(getCodexAccountsForServer(server)).rejects.toThrow('B não respondeu em 8s');
  });

  it('401 de B não desloga A; null conserva servidor ativo', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => Response.json({ detail: 'denied' }, { status: 401 }));
    await expect(listarCredenciais(server)).rejects.toMatchObject({ status: 401 });
    expect(unauthorized).not.toHaveBeenCalled();
    await expect(listarCredenciais(null)).rejects.toThrow();
    expect(fetcher.mock.calls[1][0]).toBe('https://a.test/api/credenciais');
    expect(unauthorized).toHaveBeenCalledOnce();
  });

  it('criação posicional e explícita compartilham corpo e preservam o legado', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => Response.json({}));
    await createSession('chat', '/repo', null, 'codex', null, 'model', 'low', null, null, 'work');
    const body = JSON.parse(fetcher.mock.calls[0][1]?.body as string);
    expect(body.codex_account).toBe('work');
    await createSessionForServer(server, body);
    expect(fetcher.mock.calls[1][1]?.body).toBe(fetcher.mock.calls[0][1]?.body);
    await createSessionForServer(server, { name: 'claude', provider: 'claude', codex_account: 'work' });
    expect(JSON.parse(fetcher.mock.calls[2][1]?.body as string)).not.toHaveProperty('codex_account');
    await createSession('old');
    expect(JSON.parse(fetcher.mock.calls[3][1]?.body as string)).toEqual({ name: 'old', config_dir: null, engine: null, model: null, effort: null, remember_provider: true });
  });

  it('catálogo, Arquivo e bastão carregam a conta', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => Response.json([]));
    await modelOptionsForServer(server, 'codex', null, null, 'work');
    await getArchivePorCwd('/repo a', null, 'codex', 'work', server);
    await getArchiveHistory('project /', 'sid /', 5, null, 'codex', 'work', server);
    await resumeArchivedConversation('project /', 'sid /', null, null, 'codex', 'work', server);
    await passarBastao('old', { name: 'new', provider: 'codex', codex_account: 'work' });
    expect(fetcher.mock.calls[0][0]).toBe('https://b.test/api/model-options?provider=codex&codex_account=work');
    expect(fetcher.mock.calls[1][0]).toBe('https://b.test/api/archive-por-cwd?cwd=%2Frepo+a&provider=codex&codex_account=work');
    expect(fetcher.mock.calls[2][0]).toBe('https://b.test/api/archive/project%20%2F/sid%20%2F/history?tail=5&provider=codex&codex_account=work');
    for (const i of [3, 4]) expect(JSON.parse(fetcher.mock.calls[i][1]?.body as string).codex_account).toBe('work');
    await modelOptions('claude');
    await getArchiveHistory('p', 's');
    expect(fetcher.mock.calls[5][0]).toBe('https://a.test/api/model-options?provider=claude');
    expect(fetcher.mock.calls[6][0]).toBe('https://a.test/api/archive/p/s/history');
  });

  it('traduz erros do corpo, detail e avisos sem vazar código desconhecido', async () => {
    for (const locale of ['pt', 'en'] as const) {
      overwriteGetLocale(() => locale);
      configureLocale({ getLocale: () => locale });
      const issue = { code: 'codex_account_in_use', params: { account_id: 'work' } };
      const message = codexAccountMessage(issue);
      expect(message).not.toContain(issue.code);
      expect(formataErro(issue)).toBe(message);
      expect(await errorDetail(Response.json(issue))).toBe(message);
      expect(await errorDetail(Response.json({ detail: issue }))).toBe(message);
      const unknown = { code: 'codex_account_future', params: {} };
      expect(formataErro(unknown)).toBe(codexAccountMessage(unknown));
      expect(codexAccountMessage(unknown)).not.toContain(unknown.code);
    }
  });
});

describe('contaCodexParaEntrar', () => {
  const conta = (id: string, is_default: boolean, status: CodexAccount['auth']['status']): CodexAccount => ({
    id, credential_id: `codex:${id}`, name: id, home: `/h/${id}`, is_default,
    auth: { method: status === 'connected' ? 'oauth' : 'none', status, email: null, plan: null },
    sync: { status: 'idle', trust_pending: false, issues: [] },
  });
  it('padrao sem login e a conta a entrar; padrao logada ou desconhecida cria adicional', () => {
    expect(contaCodexParaEntrar([conta('default', true, 'disconnected')])).toBe('default');
    expect(contaCodexParaEntrar([conta('default', true, 'connected'), conta('work', false, 'disconnected')])).toBeUndefined();
    expect(contaCodexParaEntrar([conta('default', true, 'unavailable')])).toBeUndefined();
    expect(contaCodexParaEntrar(undefined)).toBeUndefined();
  });
  it('escolhe a única conta GPT conectada mesmo com padrão deslogada', () => {
    const accounts = [conta('default', true, 'disconnected'), conta('gpt', false, 'connected')];
    expect(defaultCodexAccount(accounts)?.id).toBe('gpt');
    expect(defaultCodexAccount(accounts, 'default')?.id).toBe('gpt');
  });
  it('preserva a conta lembrada conectada e mantém o caminho de login sem contas conectadas', () => {
    const accounts = [conta('default', true, 'connected'), conta('gpt', false, 'connected')];
    expect(defaultCodexAccount(accounts, 'gpt')?.id).toBe('gpt');
    expect(defaultCodexAccount([conta('default', true, 'disconnected')])?.id).toBe('default');
    expect(defaultCodexAccount([])).toBeUndefined();
  });
});
