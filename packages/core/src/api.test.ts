import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureDiag, _resetDiagForTests } from './diag';
import { estaDesligado, registrarFalha, _limparEsfriamentoParaTestes } from './esfriamento';
afterEach(_resetDiagForTests);
import { overwriteGetLocale as overwriteFront } from './paraglide/runtime';
import { configureLocale } from './i18n';
function overwriteGetLocale(fn: () => 'en' | 'pt') {
  overwriteFront(fn);
  configureLocale({ getLocale: fn });
}
import { configureApi } from './apiEnv';
// `getHistoryDesde` veio da main junto com o histórico condicional (304 + ETag).
import { getConfig, getConfigForServer, patchConfig, patchConfigForServer, createSession, getHistory, getHistoryDesde, isAbortError, transcribeFile, transcribeFileForServer, transcribeUploaded, transcribeUploadedForServer, uploadFile, uploadFileForServer, getModelOptions, setEngineModel, rotaGenerica, pairSession } from './api';
import { createSessionForServer, getFolderGitForServer, folderGitActionForServer, defaultBase } from './api';
import { mensagemDeErro, formataErro } from './errosApi';
import { passarBastao, getSyncSetupForServer, setupSyncForServer, disableSyncForServer } from './api';
import { probeServerResponse } from './api';
import { scanDir, scanDirForServer, listClaudeConfigs, listClaudeConfigsForServer } from './api';
import { answerQuestions, closePluginPane, inputPluginField, interrupt, openEventStreamForServer, pressPluginButton, sendInputForServer, showPluginPane, skipQuestion } from './api';
import { discardFile, fileAuthHeader, fileUrlNative, getPairContract, getPlans, listFiles, pathDiff, readFile, searchFiles, setPlanPin, unpairSession, writeFile } from './api';
import type { Server } from './servers';
import { exportShortcuts } from './api';
import { fileUrl, uploadUrl, uploadUrlNative } from './api';
import { editTranscriptionProviderKey, editTranscriptionProviderTarget, moveTranscriptionProvider, parseTranscriptionProviders, transcriptionProviderKeepsKey, transcriptionProviderLabel, transcriptionProvidersMissingKey, testTranscriptionProvider } from './api';
const server = { id: 'a', label: 'Servidor A', baseUrl: 'https://a.test', token: 'token-a' };
/** O campo `V18-campo` da vitrine, como o `inputControl` o tira da árvore. */
const CAMPO = { plugin: 'vitrine', key: 'V18-campo' };

it('teste de transcrição informa o prazo real do servidor selecionado', async () => {
  vi.spyOn(globalThis, 'fetch').mockRejectedValue(new DOMException('prazo excedido', 'TimeoutError'));
  await expect(testTranscriptionProvider('p', new Blob(['audio']), 'fala.wav', server))
    .rejects.toThrow('Servidor A não respondeu em 300s');
});

it('exportação leva IDs selecionados ao servidor escolhido e distingue seleção vazia', async () => {
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{}'));
  const other = { id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'token-b' };
  await exportShortcuts(other, { ids: ['um', 'dois com espaço'], includeScripts: true });
  const url = new URL(String(fetchMock.mock.calls[0][0]));
  expect(url.origin).toBe('https://b.test');
  expect(url.searchParams.getAll('ids')).toEqual(['um', 'dois com espaço']);
  expect(url.searchParams.get('include_scripts')).toBe('true');
  expect(fetchMock.mock.calls[0][1]?.headers).toEqual(expect.objectContaining({ Authorization: 'Bearer token-b' }));
  fetchMock.mockResolvedValue(new Response('{}'));
  await exportShortcuts(null, { ids: [], includeScripts: false });
  const empty = new URL(String(fetchMock.mock.calls[1][0]));
  expect(empty.searchParams.getAll('ids')).toEqual(['']);
  expect(empty.searchParams.get('include_scripts')).toBe('false');
});

it.each(['https://hangar.example', 'https://desktop.example.ts.net', 'http://192.168.1.25:8765'])(
  'download de documento usa a conexão selecionada (%s), não a origem do PWA', (baseUrl) => {
  configureApi({ getBaseUrl: () => baseUrl, getToken: () => 'token-a', onUnauthorized: () => {},
    origin: 'https://pwa.example', createEventSource: () => stubEventSource() });
  const normal = new URL(fileUrl('session name', '/tmp/Relatório final.docx'));
  const download = new URL(fileUrl('session name', '/tmp/Relatório final.docx', true));
  expect(download.origin).toBe(baseUrl);
  expect(download.pathname).toBe('/api/sessions/session%20name/file');
  expect(download.searchParams.get('path')).toBe('/tmp/Relatório final.docx');
  expect(download.searchParams.get('token')).toBe('token-a');
  expect(download.searchParams.get('download')).toBe('1');
  expect(normal.searchParams.has('download')).toBe(false);
  expect(new URL(uploadUrl('s', 'Relatório.pdf', true)).searchParams.get('download')).toBe('1');
});

it('largura pede miniatura com w=; sem largura a URL fica igual', () => {
  configureApi({ getBaseUrl: () => 'https://hangar.example', getToken: () => 'token-a', onUnauthorized: () => {},
    origin: 'https://pwa.example', createEventSource: () => stubEventSource() });
  expect(new URL(fileUrl('s', '/tmp/a.png', false, null, 192)).searchParams.get('w')).toBe('192');
  expect(new URL(uploadUrl('s', 'a.png', false, null, 192)).searchParams.get('w')).toBe('192');
  expect(fileUrl('s', '/tmp/a.png', false, null, undefined)).toBe(fileUrl('s', '/tmp/a.png'));
  expect(uploadUrl('s', 'a.png')).toBe('https://hangar.example/api/sessions/s/uploads/a.png?token=token-a');
});

it('antes do prazo só a verificação explícita consulta o offline; resposta retira a marca', async () => {
  registrarFalha(server.id);
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{}'));
  await expect(getConfigForServer(server)).rejects.toThrow();
  expect(fetchMock).not.toHaveBeenCalled();
  await probeServerResponse(server, '/api/peers/identificador');
  expect(estaDesligado(server.id)).toBe(false);
  expect(fetchMock).toHaveBeenCalledTimes(1);
});

it('consulta normal volta a alcançar o servidor após o prazo persistido', async () => {
  const clock = vi.spyOn(Date, 'now').mockReturnValue(1000);
  registrarFalha(server.id);
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{}'));
  await expect(getConfigForServer(server)).rejects.toThrow();
  clock.mockReturnValue(31000);
  await getConfigForServer(server);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(estaDesligado(server.id)).toBe(false);
});

it('timeout apresentado como AbortError pelo navegador marca offline sem confundir cancelamento', async () => {
  const abort = new DOMException('aborted', 'AbortError');
  vi.spyOn(globalThis, 'fetch').mockRejectedValue(abort);
  const cancelled = new AbortController();
  cancelled.abort();
  await expect(probeServerResponse(server, '/api/alcance', { signal: cancelled.signal })).rejects.toBe(abort);
  expect(estaDesligado(server.id)).toBe(false);
  const timedOut = new AbortController();
  timedOut.abort(new DOMException('timeout', 'TimeoutError'));
  await expect(probeServerResponse(server, '/api/alcance', { signal: timedOut.signal })).rejects.toBe(abort);
  expect(estaDesligado(server.id)).toBe(true);
});
it('configura sincronização no servidor escolhido sem trocar nem apagar o servidor ativo', async () => {
  const b = { id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'token-b' };
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{}'));
  await getSyncSetupForServer(b);
  await setupSyncForServer(b);
  await disableSyncForServer(b);
  expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
    'https://b.test/api/sync/setup', 'https://b.test/api/sync/setup', 'https://b.test/api/sync/setup/disable',
  ]);
  for (const [, init] of fetchMock.mock.calls) {
    expect(init?.headers).toEqual(expect.objectContaining({ Authorization: 'Bearer token-b' }));
    expect(init?.signal).toBeInstanceOf(AbortSignal);
  }
  fetchMock.mockResolvedValue(new Response('{}', { status: 401 }));
  await expect(disableSyncForServer(b)).rejects.toThrow();
  expect(onUnauthorizedSpy).not.toHaveBeenCalled();
});
it('bastão captura servidor B e preserva a chamada antiga em A', async () => {
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{}', { status: 200 }));
  const b = { id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'token-b' };
  const body = { name: 'next', provider: 'codex' as const, codex_account: 'work' };
  await passarBastao('origin', body, b);
  await passarBastao('origin', { name: 'legacy' });
  expect(fetchMock.mock.calls[0][0]).toBe('https://b.test/api/sessions/origin/bastao');
  expect(fetchMock.mock.calls[0][1]?.headers).toEqual(expect.objectContaining({ Authorization: 'Bearer token-b' }));
  expect(JSON.parse(fetchMock.mock.calls[0][1]?.body as string)).toEqual(body);
  expect(fetchMock.mock.calls[1][0]).toBe('https://a.test/api/sessions/origin/bastao');
});
let onUnauthorizedSpy: ReturnType<typeof vi.fn<() => void>>;
function stubEventSource() {
  return { addEventListener() {}, removeEventListener() {}, close() {}, onerror: null, onopen: null, readyState: 0 } as unknown as import('./apiEnv').EventSourceLike;
}
beforeEach(() => {
  vi.restoreAllMocks();
  // Uma falha de rede marca o servidor como desligado, e a marca é do módulo: sem limpar, o
  // primeiro caso que simula queda faz os seguintes receberem "está marcado como desligado".
  _limparEsfriamentoParaTestes();
  onUnauthorizedSpy = vi.fn<() => void>();
  configureApi({
    getBaseUrl: () => 'https://a.test',
    getToken: () => 'token-a',
    onUnauthorized: onUnauthorizedSpy as unknown as () => void,
    origin: 'https://app.test',
    createEventSource: () => stubEventSource(),
  });
});

describe('rotaGenerica — o que pode ir pro diário de uso', () => {
  it('apaga o nome da sessão (agrupa, e não interessa: o campo `sessao` existe pra isso)', () => {
    expect(rotaGenerica('/api/sessions/api-front/select')).toBe('/api/sessions/*/select');
    expect(rotaGenerica('/api/sessions/api-front/history?limit=40')).toBe('/api/sessions/*/history');
  });

  it('apaga o projeto do Archive: ele É o caminho real do projeto na máquina', () => {
    // Achado da revisão de 25/08/2026. `resumeArchivedConversation` é POST, então entrava no
    // diário em TODO uso, dando certo — e o diário promete não guardar caminho de projeto.
    expect(rotaGenerica('/api/archive/-home-fulano-Projetos-cliente-x'))
      .toBe('/api/archive/*');
    expect(rotaGenerica('/api/archive/-home-fulano-Projetos-cliente-x/abc-123/resume'))
      .toBe('/api/archive/*/abc-123/resume');
  });

  it('apaga o nome da conta em claude-configs', () => {
    expect(rotaGenerica('/api/claude-configs/conta-do-cliente')).toBe('/api/claude-configs/*');
  });

  it('rota sem segmento variável passa inteira', () => {
    expect(rotaGenerica('/api/diag')).toBe('/api/diag');
    expect(rotaGenerica('/api/sessions')).toBe('/api/sessions');
  });
});

describe('explicit server settings API', () => {
  it('correlaciona o servidor explícito sem registrar corpo, credencial ou URL', async () => {
    const registrar = vi.fn();
    configureDiag({ registrar, novoReq: () => 'req-teste' });
    const b = { ...server, baseUrl: 'https://b.test', token: 'segredo-b' };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({
      detail: { code: 'erro_teste', message: 'segredo https://privado.test/?token=secreto' },
    }), { status: 409 }));
    await expect(patchConfigForServer(b, { automations: false })).rejects.toThrow();
    expect(fetchMock.mock.calls[0][1]?.headers).toEqual(expect.objectContaining({ 'X-Hangar-Req': 'req-teste' }));
    expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ req: 'req-teste', codigo: '409',
      detalhe: expect.stringContaining('POST /api/config') }), 'https://b.test');
    const eventos = JSON.stringify(registrar.mock.calls.map(([ev]) => ev));
    expect(eventos).not.toMatch(/segredo|privado|secreto|https:/);
  });
  it('usa base e token explícitos sem depender do servidor ativo', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ campos: {}, somente_leitura: {} }), { status: 200 }),
    );

    await getConfigForServer(server);

    expect(fetchMock).toHaveBeenCalledWith('https://a.test/api/config', expect.objectContaining({
      headers: expect.objectContaining({ Authorization: 'Bearer token-a' }),
    }));
  });

  it('401 explícito vira erro e não chama onUnauthorized nem remove credencial global', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'token inválido' }), { status: 401 }),
    );

    await expect(patchConfigForServer(server, { automations: false }))
      .rejects.toThrow('401: token inválido');

    expect(onUnauthorizedSpy).not.toHaveBeenCalled();
  });

  it('401 global chama onUnauthorized', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'não autorizado' }), { status: 401 }),
    );
    await expect(getConfig()).rejects.toThrow();
    expect(onUnauthorizedSpy).toHaveBeenCalled();
  });

  // 502 de infra (proxy Tailscale) sem corpo JSON e sem statusText (comum atras de HTTP/2): sem o
  // fallback, `.message` fica '' e telas que testam `if (erro)` (TtsBar, ServerSettings) desenham a
  // UI de sucesso por cima de uma falha real.
  it('erro sem corpo e sem statusText ainda produz mensagem não-vazia', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response('', { status: 502, statusText: '' }),
    );

    await expect(getConfigForServer(server)).rejects.toThrow(/502/);
  });
});

// O sintoma real: com um servidor da malha OFFLINE, abrir Configuracoes dele prendia a folha em
// "Carregando..." pra sempre. VPN pra no morto nao RECUSA a conexao — o socket fica pendurado e a
// promessa nunca resolve, entao nem o catch nem o finally da tela rodavam.
describe('chamada a outro servidor tem prazo', () => {
  it('manda um signal por padrão', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ campos: {}, somente_leitura: {} }), { status: 200 }),
    );
    await getConfigForServer(server);
    expect(fetchMock.mock.calls[0][1]).toHaveProperty('signal');
  });

  it('estourar o prazo vira erro legível, com o nome do servidor', async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(
      new DOMException('signal timed out', 'TimeoutError'),
    );
    await expect(getConfigForServer(server)).rejects.toThrow(/Servidor A.*não respondeu/);
  });

  it('quem CANCELA por conta própria recebe o abort cru, não a mensagem de servidor fora do ar', async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new DOMException('aborted', 'AbortError'));
    await expect(getConfigForServer(server)).rejects.toSatisfy(
      (e: unknown) => e instanceof DOMException && (e as DOMException).name === 'AbortError',
    );
  });
});

// Round 4: o caminho GLOBAL (servidor ativo) também tinha o mesmo buraco do ForServer — servidor
// atrás de VPN não recusa conexão e o socket pendurava a folha de Configurações pra sempre.
describe('config global tem prazo (round 4)', () => {

  it('getConfig e patchConfig globais mandam signal por padrão', async () => {
    // Response nova por chamada: reusar o MESMO objeto faria o 2º res.json() estourar
    // ("Body has already been read").
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response(JSON.stringify({ campos: {}, somente_leitura: {} }), { status: 200 }),
    );
    await getConfig();
    expect(fetchMock.mock.calls[0][1]).toHaveProperty('signal');
    await patchConfig({ automations: false });
    expect(fetchMock.mock.calls[1][1]).toHaveProperty('signal');
  });
});

describe('getHistory', () => {
  // fetch que respeita o signal, como o do browser: só termina quando a resposta chega OU quando o
  // signal aborta — é o que permite provar que o download PARA, e não só que a resposta é ignorada.
  function fetchQueRespeitaOSignal() {
    return vi.spyOn(globalThis, 'fetch').mockImplementation((_url, init) =>
      new Promise((_resolve, reject) => {
        const s = (init as RequestInit).signal!;
        s.addEventListener('abort', () => reject(s.reason));
      }),
    );
  }

  it('abortar cancela o fetch de verdade e NÃO vira erro de tela', async () => {
    const fetchMock = fetchQueRespeitaOSignal();
    const ctl = new AbortController();

    const p = getHistory('sessao', undefined, ctl.signal);
    ctl.abort();
    const err = await p.catch((e) => e);

    // 1. o fetch recebeu um signal que de fato abortou -> a requisição para na rede
    const passado = (fetchMock.mock.calls[0][1] as RequestInit).signal!;
    expect(passado.aborted).toBe(true);
    // 2. o rejeito é cancelamento, não falha -> o Chat retorna sem pintar pílula de erro
    expect(isAbortError(err)).toBe(true);
    // 3. falha de verdade continua sendo falha (inclusive o timeout de 45s, que é TimeoutError)
    expect(isAbortError(new Error('500: boom'))).toBe(false);
    expect(isAbortError(new DOMException('demorou', 'TimeoutError'))).toBe(false);
  });

  it('limit=0 pede zero eventos, não o arquivo inteiro', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response('[]', { status: 200 }),
    );

    await getHistory('sessao', 0);

    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/sessions/sessao/history?limit=0');
  });
});

// No core o servidor vem do ambiente injetado (`configureApi` no beforeEach global), não do
// localStorage que a versão da PWA montava à mão.
describe('getHistoryDesde', () => {
  it('sem validador guardado, pede a cauda e devolve o ETag pra guardar', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response('[{"kind":"user_msg","id":"1"}]', { status: 200, headers: { ETag: '"v1"' } }),
    );

    const r = await getHistoryDesde('sessao', 400, null);

    expect((fetchMock.mock.calls[0][1] as RequestInit).headers).not.toHaveProperty('If-None-Match');
    expect(r).toEqual({ eventos: [{ kind: 'user_msg', id: '1' }], etag: '"v1"' });
  });

  it('validador igual -> 304 vira "igual", sem tentar ler corpo nenhum', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(null, { status: 304, headers: { ETag: '"v1"' } }),
    );

    expect(await getHistoryDesde('sessao', 400, '"v1"')).toBe('igual');
    expect((fetchMock.mock.calls[0][1] as RequestInit).headers)
      .toMatchObject({ 'If-None-Match': '"v1"' });
  });

  // Cross-origin (o PWA da VPS falando com o backend de casa) o navegador só entrega header que o
  // servidor exponha. Sem o ETag legível o cache não tem o que perguntar depois — e isso não pode
  // virar exceção: é só perder a economia, baixando tudo como antes.
  it('resposta sem ETag legível ainda entrega os eventos, com validador nulo', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('[]', { status: 200 }));
    expect(await getHistoryDesde('sessao', 400, '"v1"')).toEqual({ eventos: [], etag: null });
  });

  it('erro de verdade continua subindo (304 não é a única resposta sem json)', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'sumiu' }), { status: 404 }),
    );
    await expect(getHistoryDesde('sessao', 400, '"v1"')).rejects.toThrow('sumiu');
  });
});

describe('contratos de conversa com servidor explícito', () => {
  const target = { id: 'b', label: 'Servidor B', baseUrl: 'https://b.test', token: 'token-b' };
  const mutations = [
    { path: '/interrupt', body: {}, run: (s?: Server) => interrupt('mesma/sessão', false, s) },
    { path: '/interrupt?clear=true', body: {}, run: (s?: Server) => interrupt('mesma/sessão', true, s) },
    { path: '/plugin/press', body: { site: 'above-prompt', key: 'rv-1', plugin: 'pm-review' }, run: (s?: Server) => pressPluginButton('mesma/sessão', 'above-prompt', { plugin: 'pm-review', key: 'rv-1' }, s) },
    { path: '/plugin/close', body: { site: 'pm-mock-mr' }, run: (s?: Server) => closePluginPane('mesma/sessão', 'pm-mock-mr', s) },
    { path: '/plugin/show', body: { site: 'pm-mock-mr' }, run: (s?: Server) => showPluginPane('mesma/sessão', 'pm-mock-mr', s) },
    { path: '/plugin/input', body: { site: 'vitrine-campos', plugin: 'vitrine', key: 'V18-campo', kind: 'change', value: 'oi' }, run: (s?: Server) => inputPluginField('mesma/sessão', 'vitrine-campos', CAMPO, 'change', 'oi', s) },
    { path: '/answer', body: { answers: [], request_id: 0 }, run: (s?: Server) => answerQuestions('mesma/sessão', [], 0, s) },
    { path: '/answer', body: { answers: [] }, run: (s?: Server) => answerQuestions('mesma/sessão', [], undefined, s) },
    { path: '/question/skip', body: { request_id: 'req-b' }, run: (s?: Server) => skipQuestion('mesma/sessão', 'req-b', s) },
  ];

  it.each(mutations)('$path mantém payload e chamada antiga, mas dirige o destino explícito a B', async ({ path, body, run }) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{"ok":true,"fallback":true}'));
    await run(target);
    await run();
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      `https://b.test/api/sessions/mesma%2Fsess%C3%A3o${path}`,
      `https://a.test/api/sessions/mesma%2Fsess%C3%A3o${path}`,
    ]);
    expect(fetchMock.mock.calls[0][1]?.headers).toMatchObject({ Authorization: 'Bearer token-b' });
    expect(fetchMock.mock.calls[1][1]?.headers).toMatchObject({ Authorization: 'Bearer token-a' });
    if (path === '/answer' || path === '/question/skip') {
      expect(fetchMock.mock.calls[0][1]?.signal).toBeUndefined();
    }
    for (const [, init] of fetchMock.mock.calls) {
      expect(init?.method).toBe('POST');
      expect(JSON.parse(init?.body as string)).toEqual(body);
    }
  });

  it('resposta textual preserva fallback, respostas e request_id string', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"ok":true,"fallback":true}'));
    const answers = [{ kind: 'text' as const, question_id: 'q-b', value: 'outro caminho', type_index: 1, labels: [] }];
    expect(await answerQuestions('sessao', answers, 'req-b', target)).toEqual({ ok: true, fallback: true });
    expect(JSON.parse(fetchMock.mock.calls[0][1]?.body as string)).toEqual({ answers, request_id: 'req-b' });
  });

  it.each(mutations)('$path recusado por B não remove a credencial ativa nem repete POST', async ({ run }) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"detail":"recusado"}', { status: 401 }));
    await expect(run(target)).rejects.toMatchObject({ status: 401, message: '401: recusado' });
    expect(onUnauthorizedSpy).not.toHaveBeenCalled();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it.each(mutations)('$path com transporte incerto conserva o erro e não repete POST', async ({ run }) => {
    const error = new TypeError('rede interrompida');
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockRejectedValue(error);
    await expect(run(target)).rejects.toBe(error);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('chamada antiga de interrupção conserva clear=false e tratamento de 401', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{}', { status: 401 }));
    await expect(interrupt('sessao')).rejects.toMatchObject({ status: 401 });
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/sessions/sessao/interrupt');
    expect(onUnauthorizedSpy).toHaveBeenCalledTimes(1);
  });

  it('histórico explícito conserva limit=0, prazo customizado e servidor legado', async () => {
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('[]'));
    expect(await getHistory('mesma/sessão', 0, undefined, 12000, target)).toEqual([]);
    await getHistory('mesma/sessão');
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      'https://b.test/api/sessions/mesma%2Fsess%C3%A3o/history?limit=0',
      'https://a.test/api/sessions/mesma%2Fsess%C3%A3o/history',
    ]);
    expect(fetchMock.mock.calls[0][1]?.headers).toMatchObject({ Authorization: 'Bearer token-b' });
    expect(timeout).toHaveBeenCalledWith(12000);
    expect(timeout).toHaveBeenCalledWith(45000);
  });

  it('histórico condicional em B mantém ETag/304 sem ler corpo', async () => {
    const unchanged = new Response(null, { status: 304 });
    const json = vi.spyOn(unchanged, 'json');
    const fetchMock = vi.spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(new Response('[{"kind":"user_msg","id":"b-1"}]', { headers: { ETag: '"b-v1"' } }))
      .mockResolvedValueOnce(unchanged);
    expect(await getHistoryDesde('sessao', 400, null, undefined, undefined, target))
      .toEqual({ eventos: [{ kind: 'user_msg', id: 'b-1' }], etag: '"b-v1"' });
    expect(await getHistoryDesde('sessao', 400, '"b-v1"', undefined, undefined, target)).toBe('igual');
    expect(json).not.toHaveBeenCalled();
    expect(fetchMock.mock.calls[0][1]?.headers).not.toHaveProperty('If-None-Match');
    expect(fetchMock.mock.calls[1][1]?.headers).toMatchObject({ Authorization: 'Bearer token-b', 'If-None-Match': '"b-v1"' });
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      'https://b.test/api/sessions/sessao/history?limit=400',
      'https://b.test/api/sessions/sessao/history?limit=400',
    ]);
  });

  it('histórico explícito sem ETag devolve validador nulo', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('[]'));
    expect(await getHistoryDesde('sessao', 400, null, undefined, undefined, target)).toEqual({ eventos: [], etag: null });
  });

  const reads = [
    (signal?: AbortSignal) => getHistory('sessao', 400, signal, undefined, target),
    (signal?: AbortSignal) => getHistoryDesde('sessao', 400, '"b-v1"', signal, undefined, target),
  ];

  it.each(reads)('cancelamento da leitura explícita aborta o fetch sem marcar B offline', async (read) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation((_url, init) => new Promise((_resolve, reject) => {
      init?.signal?.addEventListener('abort', () => reject(init.signal?.reason), { once: true });
    }));
    const controller = new AbortController();
    const pending = read(controller.signal);
    controller.abort();
    const error = await pending.catch((e: unknown) => e);
    expect(isAbortError(error)).toBe(true);
    expect(fetchMock.mock.calls[0][1]?.signal?.aborted).toBe(true);
    expect(estaDesligado(target.id)).toBe(false);
  });

  it.each(reads)('401 da leitura explícita fica em B sem apagar o servidor ativo', async (read) => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"detail":"token recusado"}', { status: 401 }));
    await expect(read()).rejects.toMatchObject({ status: 401, message: '401: token recusado' });
    expect(onUnauthorizedSpy).not.toHaveBeenCalled();
  });

  it.each(reads)('timeout da leitura explícita conserva a identidade do erro', async (read) => {
    const error = new DOMException('prazo excedido', 'TimeoutError');
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(error);
    await expect(read()).rejects.toBe(error);
  });

  it.each([401, 404, 409, 500])('envio em B preserva status %i e detalhe legível sem timeout/repetição', async (status) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"detail":"recusado"}', { status }));
    await expect(sendInputForServer(target, 'sessao', 'texto')).rejects.toMatchObject({ status, message: `${status}: recusado` });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][0]).toBe('https://b.test/api/sessions/sessao/input');
    expect(fetchMock.mock.calls[0][1]?.signal).toBeUndefined();
    expect(onUnauthorizedSpy).not.toHaveBeenCalled();
  });

  it.each([400, 408, 409, 502])('criação em B preserva status %i numérico e não repete o POST', async (status) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"detail":"recusado"}', { status }));
    await expect(createSessionForServer(target, { name: 'repo', cwd: '/repo', provider: 'codex' }))
      .rejects.toMatchObject({ status, message: `${status}: recusado` });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][0]).toBe('https://b.test/api/sessions');
  });

  it('criação com transporte incerto não inventa status nem tenta de novo', async () => {
    const error = new TypeError('rede interrompida');
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockRejectedValue(error);
    await expect(createSessionForServer(target, { name: 'repo', cwd: '/repo' })).rejects.toBe(error);
    expect(error).not.toHaveProperty('status');
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('envio com transporte incerto não inventa status nem tenta de novo', async () => {
    const error = new TypeError('rede interrompida');
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockRejectedValue(error);
    await expect(sendInputForServer(target, 'sessao', 'texto')).rejects.toBe(error);
    expect(error).not.toHaveProperty('status');
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it.each([
    { baseUrl: 'https://b.test', credentials: false, token: 'token-b' },
    { baseUrl: 'https://app.test', credentials: true, token: null },
  ])('SSE em $baseUrl mantém req terceiro e cursor quarto', ({ baseUrl, credentials, token }) => {
    const create = vi.fn<import('./apiEnv').ApiEnv['createEventSource']>(() => stubEventSource());
    configureApi({ getBaseUrl: () => 'https://a.test', getToken: () => 'token-a', onUnauthorized: onUnauthorizedSpy,
      origin: 'https://app.test', createEventSource: create });
    const source = openEventStreamForServer({ ...target, baseUrl }, 'mesma/sessão', 'diag-b', 'arquivo:42 +');
    expect(source).toBe(create.mock.results[0].value);
    const url = new URL(create.mock.calls[0][0]);
    expect(url.origin).toBe(baseUrl);
    expect(url.pathname).toBe('/api/sessions/mesma%2Fsess%C3%A3o/events');
    expect(url.searchParams.get('diag_req')).toBe('diag-b');
    expect(url.searchParams.get('last_event_id')).toBe('arquivo:42 +');
    expect(url.searchParams.get('token')).toBe(token);
    expect(create.mock.calls[0][1]).toEqual({ withCredentials: credentials });
    openEventStreamForServer(target, 'sessao', 'req-legado');
    expect(new URL(create.mock.calls[1][0]).searchParams.has('last_event_id')).toBe(false);
  });
});

describe('createSession', () => {
  it.each([undefined, false, true])('preserva o modo explícito ou omitido: %s', async (mode) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"name":"x"}'));
    await createSession('x', '/tmp', null, 'claude', null, null, null, null, null, null, mode);
    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    if (mode === undefined) expect(body).not.toHaveProperty('headless');
    else expect(body.headless).toBe(mode);
  });

  // O backend so aceita provider em ("claude", "codex", "pi", "kimi") e devolve 400 se vier `engine`
  // com provider != claude. O sheet manda engine/config_dir nulos fora do Claude — aqui garantimos que
  // o provider viaja LITERAL (a versao anterior tipava 'claude' | 'codex' e uma sessao Pi nem compilava).
  it('manda o provider escolhido no corpo, sem motor', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ name: 'x', state: 'idle' }), { status: 200 }),
    );

    await createSession('x', '/home/eu/proj', null, 'pi', null);

    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/sessions');
    expect(body).toMatchObject({ name: 'x', cwd: '/home/eu/proj', provider: 'pi', config_dir: null, engine: null });
  });

  it('manda provider kimi literal no corpo (quarto provider)', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ name: 'x', state: 'idle' }), { status: 200 }),
    );

    await createSession('x', '/home/eu/proj', null, 'kimi', null);

    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    expect(body).toMatchObject({ name: 'x', cwd: '/home/eu/proj', provider: 'kimi', config_dir: null, engine: null });
  });

  it('manda headless ao criar uma sessão Codex sem terminal', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ name: 'x', state: 'idle' }), { status: 200 }),
    );

    await createSession('x', '/home/eu/proj', null, 'codex', null, null, null, null, null, null, true);

    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    expect(body).toMatchObject({ provider: 'codex', headless: true });
  });
});

// Task 10 — errorDetail entende as DUAS formas do detail: string (endpoint nao migrado) e dict
// {code, params, msg} (backend/app/mensagens.py). O segundo caminho e o mais importante: backend
// novo com front velho (build em cache do service worker) cai no msg em portugues, nunca num codigo
// cru na tela.
describe('errorDetail (Task 10)', () => {
  beforeEach(() => {
    overwriteGetLocale(() => 'pt'); // mensagens m.* traduzidas no idioma fixado
  });

  it('detail em dict com code conhecido vira a mensagem traduzida', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: { code: 'erro_sem_paleta', params: {}, msg: 'sem paleta' } }), { status: 404 }),
    );
    // O caminho ForServer prefixa o status ('404: '); o sufixo ancorado garante que a mensagem
    // e a traduzida, nao o JSON cru (que terminaria em } e nao casaria).
    await expect(getConfigForServer(server)).rejects.toThrow(/sem paleta$/);
  });

  it('detail em dict com code DESCONHECIDO cai no msg em portugues', async () => {
    // Backend mais novo que o front: o code nao esta no mapa, mas o msg sempre chega junto.
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: { code: 'code_futuro', params: { x: 1 }, msg: 'algo deu errado' } }), { status: 400 }),
    );
    await expect(getConfigForServer(server)).rejects.toThrow(/algo deu errado$/);
  });

  it('detail em string (endpoint nao migrado) continua funcionando como hoje', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'sessao nao existe' }), { status: 404 }),
    );
    await expect(getConfigForServer(server)).rejects.toThrow('sessao nao existe');
  });

  it('detail com code que e nome herdado do prototipo cai no msg, nao em [object Undefined]', async () => {
    // Parecer task 10, bloqueador 2: ERROS['toString'] devolve a funcao HERDADA do prototipo e a
    // chamada retorna '[object Undefined]' — so a leitura com hasOwnProperty faz codigo ausente
    // cair no msg em portugues, que e o contrato do mecanismo.
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: { code: 'toString', params: {}, msg: 'mensagem fallback' } }), { status: 400 }),
    );
    await expect(getConfigForServer(server)).rejects.toThrow(/mensagem fallback$/);
  });
});

// Parecer task 10, bloqueador 3: sete valores pt entraram sem acento (o mapa os entrega direto na
// UI quando o code e conhecido). O teste fixa o locale pt e exige o texto com acento — e que
// codigos herdados do prototipo devolvem undefined em vez de quebrar ou chamar funcao errada.
describe('mensagemDeErro (parecer task 10)', () => {
  beforeEach(() => {
    overwriteGetLocale(() => 'pt');
  });

  it('erro_motor_invalido vem com acento em pt', () => {
    expect(mensagemDeErro('erro_motor_invalido')).toBe('provedor inválido');
  });

  it('erro_tts_sem_cache vem com acento em pt', () => {
    expect(mensagemDeErro('erro_tts_sem_cache')).toBe('áudio não está mais em cache');
  });

  it('codes do convidado com login próprio têm texto no idioma do app', () => {
    expect(mensagemDeErro('erro_fora_do_convidado')).toBe('fora do acesso do convidado');
    expect(mensagemDeErro('erro_usuario_em_uso')).toBe('esse usuário já existe');
    expect(mensagemDeErro('erro_shortcut_hangar_convidado')).toBe(
      'Convidado não pode abrir atalho no Hangar do dono.',
    );
  });

  it('recusa por branch nomeia o alvo do 409, e main quando o servidor não manda', () => {
    expect(mensagemDeErro('erro_atualizacao_branch', { branch: 'outra', alvo: 'teste' })).toBe(
      'A atualização só roda na teste. Este checkout está noutra branch.');
    expect(mensagemDeErro('erro_atualizacao_branch', { branch: 'outra' })).toBe(
      'A atualização só roda na main. Este checkout está noutra branch.');
  });

  it('code herdado do prototipo devolve undefined, nao quebra nem chama funcao errada', () => {
    expect(mensagemDeErro('constructor')).toBeUndefined();
    expect(mensagemDeErro('__proto__')).toBeUndefined();
    expect(mensagemDeErro('hasOwnProperty')).toBeUndefined();
  });
});

// Só o pedido do mic (`limpar: true`) pode acender `limpar=1`. Este e o elo mais barato de quebrar (um `{ditado:
// true}` esquecido no caminho do anexo manda audio de 10min pro LLM) e o unico sem teste algum.
describe('transcribeFile', () => {

  it('so manda ?limpar=1 quando pedido explicitamente', async () => {
    // Response.json() so le o corpo uma vez — mockResolvedValue reusaria a MESMA Response nas 3
    // chamadas, então uma nova resposta a cada invocação.
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response(JSON.stringify({ path: 'p', text: 't' }), { status: 200 }),
    );
    const file = new File(['x'], 'audio.webm');

    await transcribeFile('sessao', file, { limpar: true });
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/sessions/sessao/transcribe?limpar=1');

    await transcribeFile('sessao', file);
    expect(fetchMock.mock.calls[1][0]).toBe('https://a.test/api/sessions/sessao/transcribe');

    await transcribeFile('sessao', file, {});
    expect(fetchMock.mock.calls[2][0]).toBe('https://a.test/api/sessions/sessao/transcribe');
  });

  it('?arquivo= vai sem corpo, com a limpeza e o nome escapado', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response(JSON.stringify({ path: '/u/gravação 1.webm', text: 't', provider: 'ElevenLabs' }), { status: 200 }),
    );
    const r = await transcribeUploaded('sessao', 'gravação 1.webm', { limpar: true });
    expect(fetchMock.mock.calls[0][0])
      .toBe('https://a.test/api/sessions/sessao/transcribe?limpar=1&arquivo=grava%C3%A7%C3%A3o%201.webm');
    expect(fetchMock.mock.calls[0][1]?.body).toBeUndefined();
    expect(r.provider).toBe('ElevenLabs');

    await transcribeUploaded('sessao', '/home/u/.hangar/uploads/p/velho/a.webm');
    expect(fetchMock.mock.calls[1][0])
      .toBe('https://a.test/api/sessions/sessao/transcribe?arquivo=%2Fhome%2Fu%2F.hangar%2Fuploads%2Fp%2Fvelho%2Fa.webm');

    await transcribeUploadedForServer(server, 'sessao', 'a.webm', { limpar: true, estilo: 'prosa' });
    expect(fetchMock.mock.calls[2][0])
      .toBe('https://a.test/api/sessions/sessao/transcribe?limpar=1&estilo=prosa&arquivo=a.webm');
    expect(fetchMock.mock.calls[2][1]?.body).toBeUndefined();
    expect(JSON.stringify(fetchMock.mock.calls[2][1]?.headers)).not.toContain('application/json');
  });

  it('upload do ditado pede só áudio; os outros uploads não mudam', async () => {
    const abertos: string[] = [];
    vi.stubGlobal('XMLHttpRequest', class {
      status = 200; responseText = '{"path":"/up/g.webm"}'; upload = {}; timeout = 0;
      onload?: () => void;
      open(_m: string, url: string) { abertos.push(url); }
      setRequestHeader() {}
      send() { queueMicrotask(() => this.onload?.()); }
    });
    await uploadFile('sessao', new File(['a'], 'g.webm'), undefined, null, { audioOnly: true });
    await uploadFile('sessao', new File(['a'], 'f.png'));
    expect(abertos).toEqual([
      'https://a.test/api/sessions/sessao/upload?audio_only=1',
      'https://a.test/api/sessions/sessao/upload',
    ]);
    vi.unstubAllGlobals();

    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response('{"path":"/up/g.webm"}', { status: 200 }));
    await uploadFileForServer(server, 'sessao', new File(['a'], 'g.webm'), { audioOnly: true });
    await uploadFileForServer(server, 'sessao', new File(['a'], 'f.png'));
    expect(fetchMock.mock.calls.map((c) => c[0])).toEqual([
      'https://a.test/api/sessions/sessao/upload?audio_only=1',
      'https://a.test/api/sessions/sessao/upload',
    ]);
  });

  it('falha de ?arquivo= leva o status', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response(JSON.stringify({ detail: 'provedor fora' }), { status: 502 }));
    await expect(transcribeUploaded('sessao', 'a.webm')).rejects.toMatchObject({ status: 502, message: 'provedor fora' });
    await expect(transcribeUploadedForServer(server, 'sessao', 'a.webm')).rejects.toMatchObject({ status: 502 });
  });
});

describe('formataErro (Task 11 round 2: envelopes nos helpers de envio e avisos do pareamento)', () => {
  it('string crua passa direto (endpoint antigo)', () => {
    expect(formataErro('sessao nao encontrada')).toBe('sessao nao encontrada');
  });

  it('envelope traduz pelo code no idioma do app', () => {
    overwriteGetLocale(() => 'en');
    const e = { code: 'erro_terminal_aberto', params: {}, msg: 'Terminal aberto nesta sessao' };
    expect(formataErro(e)).toBe('Terminal open in this session. Close the panel to reply here.');
  });

  it('envelope com code desconhecido cai no msg (rede)', () => {
    overwriteGetLocale(() => 'en');
    const e = { code: 'erro_desconhecido_futuro', params: {}, msg: 'texto legado cru' };
    expect(formataErro(e)).toBe('texto legado cru');
  });

  it('code futuro de conta Codex também preserva o texto do backend', () => {
    overwriteGetLocale(() => 'en');
    const e = { code: 'codex_account_future', params: {}, msg: 'readable server message' };
    expect(formataErro(e)).toBe('readable server message');
  });

  it('erro aninhado em params.erro traduz pelo contrato (fallback do /answer)', () => {
    overwriteGetLocale(() => 'en');
    const interno = { code: 'erro_fila_nao_entregue', params: {}, msg: 'fila indisponivel' };
    const t = mensagemDeErro('erro_drive_fallback_falhou', { erro: interno });
    expect(t).toBe('drive failed and text fallback too: queue unavailable and the prompt was not delivered');
  });

  it('lista de avisos {sessao, erro} formata cada erro traduzido (pareamento)', () => {
    overwriteGetLocale(() => 'pt');
    const avisos = [
      { sessao: 'me', erro: { code: 'erro_fila_nao_digitada', params: {}, msg: 'fila indisponivel' } },
      { sessao: 'voce', erro: 'falha de rede' },
    ];
    const t = mensagemDeErro('erro_pareamento_grupo_falha', { avisos });
    expect(t).toBe('falha em: me: fila indisponível e o prompt não foi digitado; voce: falha de rede');
  });

  it('um unico aviso nao deixa ; sobrando', () => {
    overwriteGetLocale(() => 'pt');
    const t = mensagemDeErro('erro_pareamento_aviso_parcial', {
      avisos: [{ sessao: 'voce', erro: 'falha de rede' }],
    });
    expect(t).toBe('aviso falhou em: voce: falha de rede');
  });

  it('array misto em ingles: envelope traduz, string de peer fica crua (parecer c0fc8a84)', () => {
    overwriteGetLocale(() => 'en');
    const t = mensagemDeErro('erro_pareamento_saida_falhou', {
      avisos: [
        { sessao: 'srv-a::x', erro: { code: 'erro_pareamento_server_id_ausente', params: {}, msg: 'CP_SERVER_ID ausente' } },
        { sessao: 'peer2', erro: 'rede caiu' },
      ],
    });
    expect(t).toBe(
      'leave notification failed: srv-a::x: CP_SERVER_ID missing in backend/.env — required for cross-server pairing (it\'s the reply address srv::sessao); peer2: rede caiu',
    );
  });

  it('não-envelope devolve undefined', () => {
    expect(formataErro(undefined)).toBeUndefined();
    expect(formataErro(42)).toBeUndefined();
  });
});

// O mic do card (board/canvas) usa a versao por-servidor e ficou de fora do `limpar` por dois
// meses: o MESMO botao devolvia texto limpo no chat e cru no card. Este teste existe pra prender
// as duas na mesma regra — a versao por-servidor tem que aceitar e montar a query igualzinho.
describe('transcribeFileForServer', () => {
  it('monta ?limpar=1 igual a versao do servidor ativo', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response(JSON.stringify({ path: 'p', text: 't' }), { status: 200 }),
    );
    const file = new File(['x'], 'audio.webm');

    await transcribeFileForServer(server, 'sessao', file, { limpar: true });
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/sessions/sessao/transcribe?limpar=1');

    await transcribeFileForServer(server, 'sessao', file);
    expect(fetchMock.mock.calls[1][0]).toBe('https://a.test/api/sessions/sessao/transcribe');
  });

  it('repassa o aviso de que a limpeza desistiu', async () => {
    // Sem isto o card mostraria o texto CRU como se fosse o limpo — a falha calada que a regra do
    // projeto proibe. O texto entra do mesmo jeito; o que nao pode e o motivo sumir.
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response(
        JSON.stringify({ path: 'p', text: 'cru', raw: 'cru', aviso: 'provedor 502' }),
        { status: 200 },
      ),
    );
    const r = await transcribeFileForServer(server, 'sessao', new File(['x'], 'a.webm'), { limpar: true });
    expect(r.aviso).toBe('provedor 502');
  });
});

// O popover de modelo abria em "Carregando…" a cada toque: toda abertura era um GET novo.
// O cache curto (60s) + dedupe de em-voo + prefetch do Composer resolvem; estes três cobrem o
// contrato do _catalogo. Nomes de sessão únicos por teste: o cache é module-level e sobrevive
// entre os its deste arquivo.
describe('cache curto dos catálogos dos seletores', () => {

  it('segunda leitura dentro do TTL sai do cache — um fetch só', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ kind: 'claude', models: [] }), { status: 200 }),
    );
    await getModelOptions('sessao-cat-1');
    await getModelOptions('sessao-cat-1');
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('duas leituras em voo dividem o mesmo GET', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ kind: 'claude', models: [] }), { status: 200 }),
    );
    await Promise.all([getModelOptions('sessao-cat-2'), getModelOptions('sessao-cat-2')]);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('aplicar um modelo invalida o cache da sessão', async () => {
    // mockImplementation (uma Response NOVA por chamada): o body de uma Response reutilizada
    // só pode ser lido uma vez.
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response(JSON.stringify({ ok: true, model: 'x', result: null }), { status: 200 }),
    );
    await getModelOptions('sessao-cat-3');
    await setEngineModel('sessao-cat-3', { model: 'x' });
    await getModelOptions('sessao-cat-3');   // sem invalidação, este viria do cache (2 fetches)
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });
});

describe('pairSession', () => {
  it('manda replace_task no corpo, default false quando o caller não passa', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{}', { status: 200 }));
    await pairSession('sessao', ['outra'], 'tarefa');
    const [, init] = fetchMock.mock.calls[0];
    expect(JSON.parse(init?.body as string)).toEqual({ peers: ['outra'], task: 'tarefa', replace_task: false });
  });

  it('repassa replaceTask=true (409 da API vira caminho de confirmar a troca)', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{}', { status: 200 }));
    await pairSession('sessao', ['outra'], 'tarefa', true);
    const [, init] = fetchMock.mock.calls[0];
    expect(JSON.parse(init?.body as string)).toEqual({ peers: ['outra'], task: 'tarefa', replace_task: true });
  });
});

describe('catálogos da criação com servidor explícito', () => {
  const target = { id: 'b', label: 'Servidor B', baseUrl: 'https://b.test', token: 'token-b' };

  it('scanDirForServer consulta B e a chamada antiga continua no ativo', async () => {
    const body = { entries: [{ name: 'app', path: 'C:\\proj\\app', is_git: true, has_claude_md: false }] };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify(body)));
    await expect(scanDirForServer(target, 'C:\\proj', 'C:\\proj\\app')).resolves.toEqual(body);
    await scanDir('/home');
    expect(fetchMock.mock.calls[0][0]).toBe(`https://b.test/api/fs/scan?${new URLSearchParams({ root: 'C:\\proj', path: 'C:\\proj\\app' })}`);
    expect(fetchMock.mock.calls[0][1]?.headers).toMatchObject({ Authorization: 'Bearer token-b' });
    expect(fetchMock.mock.calls[0][1]?.signal).toBeInstanceOf(AbortSignal);
    expect(fetchMock.mock.calls[1][0]).toBe('https://a.test/api/fs/scan?root=%2Fhome');
    expect(fetchMock.mock.calls[1][1]?.headers).toMatchObject({ Authorization: 'Bearer token-a' });
  });

  it.each([400, 403, 404, 500])('recusa %i em B vira o mesmo error do scanDir global', async (status) => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{"detail":"x"}', { status }));
    const explicit = await scanDirForServer(target, '/r');
    expect(explicit).toEqual(await scanDir('/r'));
    expect(explicit.entries).toEqual([]);
  });

  it('401 em B borbulha com status e não derruba a credencial ativa', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"detail":"token"}', { status: 401 }));
    await expect(scanDirForServer(target, '/r')).rejects.toMatchObject({ status: 401 });
    expect(onUnauthorizedSpy).not.toHaveBeenCalled();
  });

  // Falha de rede arma o esfriamento de B até o próximo beforeEach: o cancelamento fica em outro it.
  it('falha de rede borbulha sem virar recusa de pasta', async () => {
    const network = new TypeError('rede interrompida');
    vi.spyOn(globalThis, 'fetch').mockRejectedValueOnce(network);
    await expect(scanDirForServer(target, '/r')).rejects.toBe(network);
    expect(network).not.toHaveProperty('status');
  });

  it('cancelamento de quem chamou borbulha cru', async () => {
    const cancelled = new AbortController();
    cancelled.abort();
    const abort = new DOMException('aborted', 'AbortError');
    vi.spyOn(globalThis, 'fetch').mockRejectedValueOnce(abort);
    await expect(scanDirForServer(target, '/r', undefined, cancelled.signal)).rejects.toBe(abort);
  });

  it('listClaudeConfigsForServer consulta B sem mexer no ativo nem no 401 global', async () => {
    const configs = [{ label: 'padrao', path: '/home/b/.claude', active: true }];
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify(configs)));
    await expect(listClaudeConfigsForServer(target)).resolves.toEqual(configs);
    await listClaudeConfigs();
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      'https://b.test/api/claude-configs',
      'https://a.test/api/claude-configs',
    ]);
    expect(fetchMock.mock.calls[0][1]?.headers).toMatchObject({ Authorization: 'Bearer token-b' });
    fetchMock.mockImplementation(async () => new Response('{}', { status: 401 }));
    await expect(listClaudeConfigsForServer(target)).rejects.toMatchObject({ status: 401 });
    expect(onUnauthorizedSpy).not.toHaveBeenCalled();
  });
});

describe('arquivos, planos e contrato do par com servidor explícito', () => {
  const target = { id: 'b', label: 'Servidor B', baseUrl: 'https://b.test', token: 'token-b' };
  const s = 'mesma/sessão';
  const enc = 'mesma%2Fsess%C3%A3o';
  const calls = [
    { rota: '/files/list?so_modificados=true', run: (x?: Server) => listFiles(s, undefined, true, x) },
    { rota: '/files/list?so_modificados=false&path=src', run: (x?: Server) => listFiles(s, 'src', false, x) },
    { rota: '/files/read?path=a.md', run: (x?: Server) => readFile(s, 'a.md', x) },
    { rota: '/files/search?q=foo&mode=contents', run: (x?: Server) => searchFiles(s, 'foo', 'contents', x) },
    { rota: '/git/path-diff', body: { path: 'a.md', escopo: 'branch' }, run: (x?: Server) => pathDiff(s, 'a.md', 'branch', x) },
    { rota: '/files/write', body: { path: 'a.md', text: 't', digest: 'd1' }, run: (x?: Server) => writeFile(s, 'a.md', 't', 'd1', x) },
    { rota: '/git/discard', body: { path: 'a.md' }, run: (x?: Server) => discardFile(s, 'a.md', x) },
    { rota: '/plans', run: (x?: Server) => getPlans(s, x) },
    { rota: '/plan-pin', body: { stem: null }, run: (x?: Server) => setPlanPin(s, null, x) },
    { rota: '/pair/contract', run: (x?: Server) => getPairContract(s, x) },
    { rota: '/pair', body: { peers: ['outra'], task: 't', replace_task: true }, run: (x?: Server) => pairSession(s, ['outra'], 't', true, x) },
    { rota: '/pair', method: 'DELETE', run: (x?: Server) => unpairSession(s, x) },
  ];

  it.each(calls)('$rota vai a B com o token de B e a chamada antiga segue no ativo', async ({ rota, body, method, run }) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{"ok":true}'));
    await expect(run(target)).resolves.toEqual({ ok: true });
    await run();
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      `https://b.test/api/sessions/${enc}${rota}`,
      `https://a.test/api/sessions/${enc}${rota}`,
    ]);
    expect(fetchMock.mock.calls[0][1]?.headers).toMatchObject({ Authorization: 'Bearer token-b' });
    expect(fetchMock.mock.calls[1][1]?.headers).toMatchObject({ Authorization: 'Bearer token-a' });
    for (const [, init] of fetchMock.mock.calls) {
      expect(init?.method ?? 'GET').toBe(method ?? (body ? 'POST' : 'GET'));
      if (body) expect(JSON.parse(init?.body as string)).toEqual(body);
    }
    // Chamada antiga sem prazo imposto; a explícita tem teto pra não pendurar em servidor desligado.
    expect(fetchMock.mock.calls[0][1]?.signal).toBeInstanceOf(AbortSignal);
    expect(fetchMock.mock.calls[1][1]?.signal).toBeUndefined();
  });

  it('busca, gravação e par em B usam teto de 30s; leitura de arquivo da sessão, 60s', async () => {
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{}'));
    await searchFiles(s, 'foo', 'names', target);
    await writeFile(s, 'a.md', 't', null, target);
    await readFile(s, 'a.md', target);
    await pairSession(s, ['outra'], 't', false, target);
    await unpairSession(s, target);
    expect(timeout.mock.calls.map(([ms]) => ms)).toEqual([30_000, 30_000, 60_000, 30_000, 30_000]);
  });

  it('conflito de digest em B chega com status, sem repetir o POST nem tocar a credencial ativa', async () => {
    const msg = 'Nao deu pra acessar esse arquivo ou pasta.';
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({
      detail: { code: 'erro_arq_mudou_no_disco', params: { msg }, msg },
    }), { status: 409 }));
    await expect(writeFile(s, 'a.md', 't', 'velho', target)).rejects.toMatchObject({ status: 409, message: expect.stringMatching(/^409: /) });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    fetchMock.mockResolvedValue(new Response('{}', { status: 401 }));
    await expect(readFile(s, 'a.md', target)).rejects.toMatchObject({ status: 401 });
    expect(onUnauthorizedSpy).not.toHaveBeenCalled();
  });

  it('visor nativo aponta URL e token para B; sem servidor, para o ativo; token nunca na URL', () => {
    expect(fileUrlNative(s, 'a.html', target)).toBe(`https://b.test/api/sessions/${enc}/file?path=a.html`);
    expect(fileAuthHeader(target)).toEqual({ Authorization: 'Bearer token-b' });
    expect(fileUrlNative(s, 'a.html')).toBe(`https://a.test/api/sessions/${enc}/file?path=a.html`);
    expect(fileAuthHeader()).toEqual({ Authorization: 'Bearer token-a' });
  });

  it('404 de arquivo em B conserva o status que o filesStore lê', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"detail":"não existe"}', { status: 404 }));
    await expect(readFile(s, 'x.md', target)).rejects.toMatchObject({ status: 404 });
    await expect(listFiles(s, 'sumiu', true, target)).rejects.toMatchObject({ status: 404 });
  });
});

describe('prazos e raiz explícita (sessão e git de pasta)', () => {
  it('abrir sessão em outro servidor espera 120 s, não os 8 s padrão', async () => {
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"name":"x"}'));
    await createSessionForServer(server, { name: 'x' });
    expect(timeout).toHaveBeenCalledWith(120_000);
  });

  it('git de pasta com raiz explícita não consulta /api/fs/roots', async () => {
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{"repo":false}'));
    await getFolderGitForServer(server, '/home/a/x', undefined, '/home/a');
    await folderGitActionForServer(server, '/home/a/x', 'fetch', '/home/a');
    const urls = fetchMock.mock.calls.map(c => String(c[0]));
    expect(urls).toHaveLength(2);
    expect(urls.some(u => u.includes('/api/fs/roots'))).toBe(false);
    expect(new URL(urls[0]).searchParams.get('root')).toBe('/home/a');
    expect(timeout).toHaveBeenCalledWith(30_000);
    expect(timeout).toHaveBeenCalledWith(150_000);
  });
});

describe('defaultBase', () => {
  it('usa a branch atual e, com HEAD solto, a primeira local', () => {
    expect(defaultBase({ current: 'main', branches: ['dev', 'main'], remotes: [], dirty: false })).toBe('main');
    expect(defaultBase({ current: null, branches: ['dev', 'main'], remotes: ['r'], dirty: false })).toBe('dev');
    expect(defaultBase({ current: null, branches: [], remotes: ['r'], dirty: false })).toBe('');
  });
});

it('custos da tela inicial pedem a visão resumida; a tela de Custos continua com o relatório inteiro', async () => {
  const { fetchCostsForServer } = await import('./api');
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{"applied":{"period":"7d"}}'));
  await fetchCostsForServer(server, '7d', false, true);
  await fetchCostsForServer(server, '7d', true);
  const [summary, full] = fetchMock.mock.calls.map(([url]) => new URL(String(url)));
  expect(summary.pathname).toBe('/api/costs');
  expect(summary.searchParams.get('view')).toBe('summary');
  expect(summary.searchParams.get('period')).toBe('7d');
  expect(full.searchParams.has('view')).toBe(false);
  expect(full.searchParams.get('fresco')).toBe('1');
});

it('503 de custos vira a frase traduzida com o código; sem envelope fica o status', async () => {
  const { fetchCostsForServer, fetchUsoForServer, motivoDoServidor } = await import('./index');
  const envelope = { ok: false, error_code: 'costs_no_disk', message: 'índice de custos indisponível',
    detail: { code: 'costs_no_disk', params: { motivo: 'índice de custos indisponível' }, msg: 'índice de custos indisponível — costs_no_disk' } };
  vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify(envelope), { status: 503 }));
  for (const chamada of [() => fetchCostsForServer(server, 'all', false, true), () => fetchUsoForServer(server, 'all')]) {
    const erro = await chamada().catch((e: unknown) => e);
    expect(erro).toMatchObject({ status: 503, code: 'costs_no_disk' });
    expect(motivoDoServidor(erro)).toContain('(costs_no_disk)');
  }
  vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('', { status: 502 }));
  const semEnvelope = await fetchCostsForServer(server, 'all').catch((e: unknown) => e);
  expect(motivoDoServidor(semEnvelope)).toBeNull();
  expect((semEnvelope as Error).message).toBe('502');
});

it.each([404, 405])('plugin/show num servidor sem a rota rejeita com status %i, que isMissingRoute reconhece', async (status) => {
  const { isMissingRoute } = await import('./pluginUi');
  vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ detail: 'Not Found' }), { status }));
  const erro = await showPluginPane('sessao', 'pm-mock-mr', server).catch((e: unknown) => e);
  expect(isMissingRoute(erro)).toBe(true);
});

describe('servidor de antes de o pedido levar o mod', () => {
  const corpos = (fetchMock: { mock: { calls: unknown[][] } }) =>
    fetchMock.mock.calls.map(([url, init]) => [String(url).replace(/^.*\/plugin\//, ''), JSON.parse(String((init as RequestInit).body))]);

  it.each([undefined, server])('press e input recusados com 422 repetem uma vez sem o mod (servidor %#)', async (s) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) =>
      String((init as RequestInit).body).includes('"plugin"')
        ? new Response(JSON.stringify({ detail: [{ type: 'extra_forbidden' }] }), { status: 422 })
        : new Response('{"ok":true}'));
    expect(await pressPluginButton('sessao', 'above-prompt', { plugin: 'pm-mock', key: 'abrir' }, s)).toEqual({ ok: true });
    expect(await inputPluginField('sessao', 'painel', CAMPO, 'change', 'a', s)).toEqual({ ok: true });
    expect(corpos(fetchMock)).toEqual([
      ['press', { site: 'above-prompt', plugin: 'pm-mock', key: 'abrir' }], ['press', { site: 'above-prompt', key: 'abrir' }],
      ['input', { site: 'painel', plugin: CAMPO.plugin, key: CAMPO.key, kind: 'change', value: 'a' }],
      ['input', { site: 'painel', key: CAMPO.key, kind: 'change', value: 'a' }]]);
  });

  it('a segunda recusa sobe ao app, sem outra tentativa', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{"detail":"x"}', { status: 422 }));
    const erro = await pressPluginButton('sessao', 'above-prompt', { plugin: 'pm-mock', key: 'abrir' }, server).catch((e: unknown) => e);
    expect(erro).toMatchObject({ status: 422 });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it.each([404, 405])('close sem a rota (%i) fecha pelo press com a key reservada', async (status) => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) =>
      String(url).endsWith('/plugin/close') ? new Response('{"detail":"Not Found"}', { status }) : new Response('{"ok":true}'));
    expect(await closePluginPane('sessao', 'painel', server)).toEqual({ ok: true });
    expect(corpos(fetchMock)).toEqual([['close', { site: 'painel' }], ['press', { site: 'painel', key: '__close__' }]]);
  });

  it('recusa do close que não é falta da rota não vira press', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response('{"detail":"x"}', { status: 409 }));
    await expect(closePluginPane('sessao', 'painel', server)).rejects.toMatchObject({ status: 409 });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});

it('plugin/press, plugin/close, plugin/show e plugin/input com servidor explícito levam o código do servidor no erro', async () => {
  const envelope = { ok: false, error_code: 'erro_mod_guarda_indisponivel', message: 'motivo',
    detail: { code: 'erro_mod_guarda_indisponivel', params: { motivo: 'motivo' }, msg: 'motivo — erro_mod_guarda_indisponivel' } };
  vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify(envelope), { status: 503 }));
  for (const chamada of [() => pressPluginButton('sessao', 'above-prompt', { plugin: 'pm-mock', key: 'abrir' }, server),
                         () => closePluginPane('sessao', 'painel', server),
                         () => showPluginPane('sessao', 'painel', server),
                         () => inputPluginField('sessao', 'painel', CAMPO, 'change', 'a', server)]) {
    const erro = await chamada().catch((e: unknown) => e);
    expect(erro).toMatchObject({ status: 503, code: 'erro_mod_guarda_indisponivel' });
  }
});

it('plugin/input sem resposta e sem servidor explícito é cortado em 8 s, e a fila do campo segue', async () => {
  const { fieldSender } = await import('./pluginField');
  vi.useFakeTimers();
  // O `AbortSignal.timeout` do Node não anda com o relógio falso: o mesmo prazo, pelo `setTimeout` falso.
  const timeout = vi.spyOn(AbortSignal, 'timeout').mockImplementation((ms) => {
    const c = new AbortController();
    setTimeout(() => c.abort(new DOMException('signal timed out', 'TimeoutError')), ms);
    return c.signal;
  });
  try {
    // Servidor que aceita a conexão e nunca responde: o pedido só termina pelo sinal.
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation((_url, init) => new Promise((_resolve, reject) => {
      init?.signal?.addEventListener('abort', () => reject(init.signal!.reason));
    }));
    const errors: unknown[] = [];
    const input = fieldSender((kind, value) => inputPluginField('sessao', 'vitrine-campos', CAMPO, kind, value),
      (err) => errors.push(err));
    input('change', 'a');
    input('submit', 'a');
    await vi.advanceTimersByTimeAsync(7999);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(errors).toHaveLength(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(JSON.parse(fetchMock.mock.calls[1][1]?.body as string)).toMatchObject({ kind: 'submit', value: 'a' });
    expect(timeout).toHaveBeenCalledWith(8000);
  } finally {
    vi.useRealTimers();
  }
});

describe('lista de serviços de transcrição', () => {
  const ELEVEN = { id: 'a', kind: 'elevenlabs' as const, name: '', base_url: '', api_key: 'xi_••••', model: '' };

  it('preserva caminhos e idioma do Whisper local sem exigir chave', () => {
    const local = { id: 'local', kind: 'whisper_cpp', name: '', base_url: '', api_key: '', model: '',
      executable_path: '/opt/Whisper local/whisper-server', model_path: '/opt/Whisper local/ggml-small.bin',
      language: 'pt', converter_path: '' };
    expect(parseTranscriptionProviders([local])).toEqual([local]);
  });

  it('aceita serviço compatível sem chave e mantém a exigência do ElevenLabs', () => {
    const openai = { ...ELEVEN, kind: 'openai' as const, api_key: '', base_url: 'http://localhost:8000/v1' };
    expect(transcriptionProvidersMissingKey([openai])).toBe(false);
    expect(transcriptionProvidersMissingKey([{ ...ELEVEN, api_key: '' }])).toBe(true);
  });

  it('lê só itens válidos e completa campos ausentes', () => {
    expect(parseTranscriptionProviders('x')).toEqual([]);
    expect(parseTranscriptionProviders([{ id: 'a', kind: 'elevenlabs', api_key: 'xi_••••' }, { id: 'b', kind: 'outro' }, null]))
      .toEqual([ELEVEN]);
  });

  it('rótulo: nome dado, ElevenLabs, ou host · modelo', () => {
    expect(transcriptionProviderLabel({ kind: 'openai', name: ' Meu ', base_url: '', model: '' })).toBe('Meu');
    expect(transcriptionProviderLabel({ kind: 'elevenlabs', name: '', base_url: 'https://x', model: '' })).toBe('ElevenLabs');
    expect(transcriptionProviderLabel({ kind: 'openai', name: '', base_url: 'https://api.groq.com/openai/v1', model: 'whisper-large-v3-turbo' }))
      .toBe('api.groq.com · whisper-large-v3-turbo');
    expect(transcriptionProviderLabel({ kind: 'openai', name: '', base_url: 'htt', model: '' })).toBe('htt · whisper-large-v3');
    expect(transcriptionProviderLabel({ kind: 'openai', name: '', base_url: ' ', model: '' })).toBe('api.groq.com · whisper-large-v3');
    expect(transcriptionProviderLabel({ kind: 'openai', name: '', base_url: 'http://LocalHost:8000/v1', model: 'm' })).toBe('localhost · m');
  });

  it('mover troca com o vizinho e não sai da lista na ponta', () => {
    expect(moveTranscriptionProvider(['a', 'b', 'c'], 0, 1)).toEqual(['b', 'a', 'c']);
    expect(moveTranscriptionProvider(['a', 'b', 'c'], 2, -1)).toEqual(['a', 'c', 'b']);
    expect(moveTranscriptionProvider(['a', 'b'], 0, -1)).toEqual(['a', 'b']);
    expect(moveTranscriptionProvider(['a', 'b'], 1, 1)).toEqual(['a', 'b']);
    expect(moveTranscriptionProvider(['a', 'b'], 5, -1)).toEqual(['a', 'b']);
  });

  it('chave apagada volta à máscara; digitada troca', () => {
    expect(editTranscriptionProviderKey(ELEVEN, 'nova', 'xi_••••').api_key).toBe('nova');
    expect(editTranscriptionProviderKey({ ...ELEVEN, api_key: 'nova' }, '', 'xi_••••')).toEqual(ELEVEN);
    expect(editTranscriptionProviderKey({ ...ELEVEN, api_key: 'x' }, '', undefined).api_key).toBe('');
  });

  it('trocar tipo ou endpoint de item salvo pede a chave de novo; voltar devolve a máscara', () => {
    const SALVO = { ...ELEVEN, kind: 'openai' as const, base_url: 'https://a/v1' };
    const outroTipo = editTranscriptionProviderTarget(SALVO, { kind: 'elevenlabs' }, SALVO);
    expect(outroTipo.api_key).toBe('');
    expect(transcriptionProvidersMissingKey([outroTipo])).toBe(true);
    expect(editTranscriptionProviderTarget(outroTipo, { kind: 'openai' }, SALVO)).toEqual(SALVO);
    expect(editTranscriptionProviderTarget(SALVO, { base_url: 'https://b/v1' }, SALVO).api_key).toBe('');
    expect(editTranscriptionProviderTarget(SALVO, { base_url: ' https://a/v1 ' }, SALVO).api_key).toBe(SALVO.api_key);
    // Chave digitada é do serviço novo: fica.
    expect(editTranscriptionProviderTarget({ ...SALVO, api_key: 'nova' }, { kind: 'elevenlabs' }, SALVO).api_key).toBe('nova');
    // ElevenLabs não tem endpoint: o texto que sobrou nele não conta.
    expect(transcriptionProviderKeepsKey({ kind: 'elevenlabs', base_url: 'x' }, { kind: 'elevenlabs', base_url: '' })).toBe(true);
    // Item novo não tem máscara a perder.
    expect(editTranscriptionProviderTarget({ ...SALVO, api_key: '' }, { kind: 'elevenlabs' }, undefined).api_key).toBe('');
  });

  it('falta chave quando algum item está sem chave', () => {
    expect(transcriptionProvidersMissingKey([])).toBe(false);
    expect(transcriptionProvidersMissingKey([ELEVEN])).toBe(false);
    expect(transcriptionProvidersMissingKey([ELEVEN, { ...ELEVEN, id: 'b', api_key: '' }])).toBe(true);
  });
});

it('uploadUrlNative usa o servidor da conversa quando recebe um', () => {
  expect(uploadUrlNative('sessao', 'ditado 1.m4a', server)).toBe('https://a.test/api/sessions/sessao/uploads/ditado%201.m4a');
});
