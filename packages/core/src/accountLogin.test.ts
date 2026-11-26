import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from './apiEnv';
import { _limparEsfriamentoParaTestes } from './esfriamento';
import { answerAntigravityTermsForServer, antigravityLoginLink, cancelAntigravityLoginForServer, cancelAntigravityTermsForServer,
  getAntigravityTermsForServer, startAntigravityTermsForServer, cancelClaudeAccountLoginForServer, createAntigravityAccountForServer,
  deleteAntigravityAccountForServer, getAntigravityAccountsForServer, getAntigravityLoginForServer, getClaudeAccountLoginStepForServer,
  modelOptionsForServer, rotaGenerica, signOutAntigravityAccountForServer, startAntigravityLoginForServer,
  startClaudeAccountLoginForServer, submitAntigravityLoginCodeForServer, submitClaudeAccountLoginCodeForServer } from './index';

const server = { id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'token-b' };
const unauthorized = vi.fn();

beforeEach(() => {
  configureApi({ getBaseUrl: () => 'https://a.test', getToken: () => 'token-a',
    onUnauthorized: unauthorized, origin: null, createEventSource: () => { throw new Error('unused'); } });
  unauthorized.mockClear();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
  _limparEsfriamentoParaTestes();
});

const actions = [
  { name: 'início', request: () => startClaudeAccountLoginForServer(server, 'work'), timeout: 30_000 },
  { name: 'passo', request: () => getClaudeAccountLoginStepForServer(server, 'work'), timeout: 10_000 },
  { name: 'código', request: () => submitClaudeAccountLoginCodeForServer(server, 'work', 'code'), timeout: 310_000 },
  { name: 'cancelamento', request: () => cancelClaudeAccountLoginForServer(server, 'work'), timeout: 15_000 },
];

function pendingFetch() {
  return vi.spyOn(globalThis, 'fetch').mockImplementation((_url, init) => new Promise<Response>((_resolve, reject) => {
    const signal = init?.signal;
    if (signal?.aborted) reject(signal.reason);
    else signal?.addEventListener('abort', () => reject(signal.reason), { once: true });
  }));
}

describe('login Claude por servidor', () => {
  it('inicia no servidor capturado com o rótulo codificado e bearer próprio', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ ok: true }));
    const timeout = vi.spyOn(AbortSignal, 'timeout');

    await expect(startClaudeAccountLoginForServer(server, 'work /')).resolves.toEqual({ ok: true });

    expect(fetcher).toHaveBeenCalledWith('https://b.test/api/conta-estado/work%20%2F/login', expect.objectContaining({
      method: 'POST', headers: expect.objectContaining({ Authorization: 'Bearer token-b' }),
      signal: expect.any(AbortSignal),
    }));
    expect(timeout).toHaveBeenCalledWith(30_000);
  });

  it.each(['idle', 'aguardando', 'concluido'] as const)('lê o passo %s com os campos do servidor', async (etapa) => {
    const step = { etapa, url: etapa === 'aguardando' ? 'https://claude.ai/login' : null,
      email: etapa === 'concluido' ? 'user@example.test' : null, plano: etapa === 'concluido' ? 'max' : null };
    const fetcher = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json(step));

    await expect(getClaudeAccountLoginStepForServer(server, 'work /')).resolves.toEqual(step);

    expect(fetcher).toHaveBeenCalledWith('https://b.test/api/conta-estado/work%20%2F/login/passo', expect.objectContaining({
      method: 'GET', headers: expect.objectContaining({ Authorization: 'Bearer token-b' }),
    }));
  });

  it('envia o código no corpo e preserva email e plano confirmados', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({
      ok: true, email: 'user@example.test', plano: 'max',
    }));

    await expect(submitClaudeAccountLoginCodeForServer(server, 'work /', 'code/?#')).resolves.toEqual({
      ok: true, email: 'user@example.test', plano: 'max',
    });

    expect(fetcher).toHaveBeenCalledWith('https://b.test/api/conta-estado/work%20%2F/login/codigo', expect.objectContaining({
      method: 'POST', body: '{"codigo":"code/?#"}',
      headers: expect.objectContaining({ Authorization: 'Bearer token-b', 'Content-Type': 'application/json' }),
    }));
  });

  it('cancela no servidor capturado sem inventar attempt_id', async () => {
    const fetcher = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ ok: true }));

    await expect(cancelClaudeAccountLoginForServer(server, 'work /')).resolves.toEqual({ ok: true });

    expect(fetcher).toHaveBeenCalledWith('https://b.test/api/conta-estado/work%20%2F/login/cancelar', expect.objectContaining({
      method: 'POST', headers: expect.objectContaining({ Authorization: 'Bearer token-b' }),
    }));
    expect(fetcher.mock.calls[0][1]?.body).toBeUndefined();
  });

  it.each(actions)('401 no $name de B preserva a credencial de A e informa status e causa', async ({ request }) => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ detail: 'token de B recusado' }, { status: 401 }));

    await expect(request()).rejects.toMatchObject({ status: 401, message: '401: token de B recusado' });
    expect(unauthorized).not.toHaveBeenCalled();
  });

  it('preserva a causa de um erro HTTP sem JSON', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('janela indisponível', { status: 503 }));

    await expect(submitClaudeAccountLoginCodeForServer(server, 'work', 'code')).rejects.toMatchObject({
      status: 503, message: '503: janela indisponível',
    });
  });

  it.each([null, false, {}, { ok: false, erro: 'login recusado' }, { erro: 'janela fechada' },
    { ok: 'true' }, { ok: true, email: 42 }])('não confirma uma resposta inválida: %j', async (body) => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json(body));

    await expect(startClaudeAccountLoginForServer(server, 'work')).rejects.toMatchObject({ status: 200 });
  });

  it.each(actions.filter(({ name }) => name !== 'passo'))('rejeita ok falso no $name sem esconder a causa', async ({ request }) => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ ok: false, erro: 'janela fechada' }));

    await expect(request()).rejects.toMatchObject({ status: 200, message: expect.stringContaining('janela fechada') });
  });

  it('não confirma corpo que não é JSON', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('resposta inválida'));

    await expect(startClaudeAccountLoginForServer(server, 'work')).rejects.toMatchObject({
      status: 200, message: '200: resposta inválida',
    });
  });

  it.each([null, {}, { etapa: 'erro', url: null, email: null, plano: null },
    { etapa: 'concluido', email: null, plano: null },
    { etapa: 'concluido', url: null, email: 42, plano: null }])('rejeita um passo fora do contrato: %j', async (body) => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json(body));

    await expect(getClaudeAccountLoginStepForServer(server, 'work')).rejects.toMatchObject({ status: 200 });
  });

  it.each([false, true])('respeita cancelamento da leitura, inclusive já cancelada: %s', async (cancelledBefore) => {
    const fetcher = pendingFetch();
    const controller = new AbortController();
    const reason = new DOMException('troca de tela', 'AbortError');
    if (cancelledBefore) controller.abort(reason);
    const result = getClaudeAccountLoginStepForServer(server, 'work', controller.signal);
    const rejected = expect(result).rejects.toBe(reason);

    if (!cancelledBefore) controller.abort(reason);

    await rejected;
    expect(fetcher.mock.calls[0][1]?.signal?.aborted).toBe(true);
    expect(unauthorized).not.toHaveBeenCalled();
  });

  it('preserva o cancelamento depois dos cabeçalhos, durante a leitura do JSON', async () => {
    const controller = new AbortController();
    const reason = new DOMException('troca de tela', 'AbortError');
    let abortBody: () => void;
    const body = new ReadableStream<Uint8Array>({
      start(stream) {
        stream.enqueue(new TextEncoder().encode('{"etapa":'));
        abortBody = () => stream.error(reason);
      },
    });
    vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => {
      init?.signal?.addEventListener('abort', () => abortBody(), { once: true });
      return new Response(body);
    });
    const result = getClaudeAccountLoginStepForServer(server, 'work', controller.signal);
    const rejected = expect(result).rejects.toBe(reason);
    await vi.waitFor(() => expect(body.locked).toBe(true));

    controller.abort(reason);

    await rejected;
  });

  it.each(actions)('mantém $name pendente até seu prazo e aborta ao atingir $timeout ms', async ({ request, timeout }) => {
    vi.useFakeTimers();
    vi.spyOn(AbortSignal, 'timeout').mockImplementation((ms) => {
      const controller = new AbortController();
      setTimeout(() => controller.abort(new DOMException('prazo esgotado', 'TimeoutError')), ms);
      return controller.signal;
    });
    const fetcher = pendingFetch();
    const result = request();
    const rejected = expect(result).rejects.toMatchObject({ name: 'TimeoutError' });
    const signal = fetcher.mock.calls[0][1]?.signal;

    await vi.advanceTimersByTimeAsync(timeout - 1);
    expect(signal?.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);

    await rejected;
    expect(signal?.aborted).toBe(true);
  });
});

describe('contas Google do Antigravity por servidor', () => {
  const ok = (body: unknown) => vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json(body));
  const call = (fetcher: ReturnType<typeof ok>) => ({ url: fetcher.mock.calls[0][0], init: fetcher.mock.calls[0][1] });

  it('lista, cria e apaga no servidor capturado, com o bearer dele', async () => {
    const lista = { instalado: true, contas: [{ id: 'default', padrao: true, pasta: '/h/.gemini', credencial: 'antigravity:/h/.gemini',
      logado: null, termos_aceitos: true, em_uso: false }] };
    let fetcher = ok(lista);
    await expect(getAntigravityAccountsForServer(server)).resolves.toEqual(lista);
    expect(call(fetcher).url).toBe('https://b.test/api/antigravity-contas');
    expect(call(fetcher).init).toMatchObject({ headers: expect.objectContaining({ Authorization: 'Bearer token-b' }) });
    fetcher.mockRestore();

    fetcher = ok({ id: 'trabalho', pasta: '/h/.antigravity-trabalho' });
    await expect(createAntigravityAccountForServer(server, 'trabalho')).resolves.toEqual({ id: 'trabalho', pasta: '/h/.antigravity-trabalho' });
    expect(call(fetcher)).toMatchObject({ url: 'https://b.test/api/antigravity-contas', init: { method: 'POST', body: '{"name":"trabalho"}' } });
    fetcher.mockRestore();

    fetcher = ok({ ok: true });
    await deleteAntigravityAccountForServer(server, 'a b');
    expect(call(fetcher)).toMatchObject({ url: 'https://b.test/api/antigravity-contas/a%20b', init: { method: 'DELETE' } });
  });

  it('a lista espera o agy models de cada conta, além dos 8 s de sempre', async () => {
    vi.useFakeTimers();
    vi.spyOn(AbortSignal, 'timeout').mockImplementation((ms) => {
      const controller = new AbortController();
      setTimeout(() => controller.abort(new DOMException('prazo esgotado', 'TimeoutError')), ms);
      return controller.signal;
    });
    const fetcher = pendingFetch();
    const result = getAntigravityAccountsForServer(server);
    const rejected = expect(result).rejects.toThrow();
    const signal = fetcher.mock.calls[0][1]?.signal;
    await vi.advanceTimersByTimeAsync(8_001);
    expect(signal?.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(22_000);
    await rejected;
    expect(signal?.aborted).toBe(true);
  });

  it('login: inicia, consulta e cancela pela tentativa, e manda o código com ela', async () => {
    let fetcher = ok({ attempt_id: 'a1', estado: 'iniciando' });
    await expect(startAntigravityLoginForServer(server, 'default')).resolves.toEqual({ attempt_id: 'a1', estado: 'iniciando' });
    expect(call(fetcher)).toMatchObject({ url: 'https://b.test/api/antigravity-contas/default/login', init: { method: 'POST' } });
    fetcher.mockRestore();

    const waiting = { attempt_id: 'a1', estado: 'aguardando_codigo', link: 'https://accounts.google.com/o/oauth2/auth?x=1' };
    fetcher = ok(waiting);
    await expect(getAntigravityLoginForServer(server, 'default', 'a 1')).resolves.toEqual(waiting);
    expect(call(fetcher).url).toBe('https://b.test/api/antigravity-contas/default/login?attempt_id=a%201');
    fetcher.mockRestore();

    fetcher = ok({ ok: true });
    await submitAntigravityLoginCodeForServer(server, 'trabalho', 'a1', '4/0AVG?#');
    expect(call(fetcher)).toMatchObject({ url: 'https://b.test/api/antigravity-contas/trabalho/login/codigo',
      init: { method: 'POST', body: '{"attempt_id":"a1","codigo":"4/0AVG?#"}' } });
    fetcher.mockRestore();

    fetcher = ok({ ok: true });
    await cancelAntigravityLoginForServer(server, 'trabalho', 'a1');
    expect(call(fetcher)).toMatchObject({ url: 'https://b.test/api/antigravity-contas/trabalho/login?attempt_id=a1', init: { method: 'DELETE' } });
    fetcher.mockRestore();

    fetcher = ok({ ok: true });
    await signOutAntigravityAccountForServer(server, 'trabalho');
    expect(call(fetcher)).toMatchObject({ url: 'https://b.test/api/antigravity-contas/trabalho/logout', init: { method: 'POST' } });
  });

  it('termos: inicia, consulta e cancela pela tentativa, e manda a opção tocada com ela', async () => {
    let fetcher = ok({ attempt_id: 't1', estado: 'iniciando' });
    await expect(startAntigravityTermsForServer(server, 'default')).resolves.toEqual({ attempt_id: 't1', estado: 'iniciando' });
    expect(call(fetcher)).toMatchObject({ url: 'https://b.test/api/antigravity-contas/default/termos', init: { method: 'POST' } });
    fetcher.mockRestore();

    const terms = { estado: 'termos', texto: 'Termos https://policies.google.com/terms' };
    fetcher = ok(terms);
    await expect(getAntigravityTermsForServer(server, 'default', 't 1')).resolves.toEqual(terms);
    expect(call(fetcher).url).toBe('https://b.test/api/antigravity-contas/default/termos?attempt_id=t%201');
    fetcher.mockRestore();

    fetcher = ok({ ok: true });
    await answerAntigravityTermsForServer(server, 'default', 't1', 2);
    expect(call(fetcher)).toMatchObject({ url: 'https://b.test/api/antigravity-contas/default/termos/resposta',
      init: { method: 'POST', body: '{"attempt_id":"t1","opcao":2}' } });
    fetcher.mockRestore();

    fetcher = ok({ ok: true });
    await cancelAntigravityTermsForServer(server, 'default', 't1');
    expect(call(fetcher)).toMatchObject({ url: 'https://b.test/api/antigravity-contas/default/termos?attempt_id=t1', init: { method: 'DELETE' } });
  });

  it('código recusado chega com o código do servidor, e o 401 de B não derruba A', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json(
      { detail: { code: 'erro_codigo_invalido', params: {}, msg: 'isso não parece o código' } }, { status: 422 }));
    await expect(submitAntigravityLoginCodeForServer(server, 'default', 'a1', 'x')).rejects.toMatchObject({ status: 422, code: 'erro_codigo_invalido' });
    vi.restoreAllMocks();
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ detail: 'token de B recusado' }, { status: 401 }));
    await expect(startAntigravityLoginForServer(server, 'default')).rejects.toMatchObject({ status: 401 });
    expect(unauthorized).not.toHaveBeenCalled();
  });

  it('o catálogo de modelos vai pela conta Google, sem conta Codex nem pasta Claude', async () => {
    const fetcher = ok({ kind: 'antigravity', reduced: false, models: [{ id: 'gemini-4-neon-high', name: 'Gemini 4 Neon (High)' }] });
    await modelOptionsForServer(server, 'antigravity', null, null, 'trabalho');
    expect(call(fetcher).url).toBe('https://b.test/api/model-options?provider=antigravity&conta=trabalho');
    fetcher.mockRestore();
    const codex = ok({ kind: 'codex', reduced: false, models: [] });
    await modelOptionsForServer(server, 'codex', null, null, 'work');
    expect(call(codex).url).toBe('https://b.test/api/model-options?provider=codex&codex_account=work');
  });

  it('o diário não guarda o nome da conta Google', () => {
    expect(rotaGenerica('/api/antigravity-contas/trabalho/login/codigo')).toBe('/api/antigravity-contas/*/login/codigo');
  });

  it.each([
    ['https://accounts.google.com/o/oauth2/auth?redirect_uri=https%3A%2F%2Fantigravity.google%2Foauth-callback', true],
    ['http://accounts.google.com/o/oauth2/auth', false],
    ['https://accounts.google.com.evil.test/o/oauth2/auth', false],
    ['https://user:pass@accounts.google.com/o/oauth2/auth', false],
    ['javascript:alert(1)', false],
    ['', false],
  ])('só abre o link do Google: %s', (link, valid) => {
    expect(antigravityLoginLink(link) !== null).toBe(valid);
  });
});
