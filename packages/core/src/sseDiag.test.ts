import { afterEach, expect, it, vi } from 'vitest';
import { configureApi, _resetApiEnvForTests, type EventSourceLike } from './apiEnv';
import { configureDiag, _resetDiagForTests } from './diag';
import { openEventStream, openEventStreamForServer, openSessionsStream } from './api';

afterEach(() => { _resetApiEnvForTests(); _resetDiagForTests(); });

it.each([true, false])('SSE preserva autenticação e retomada com o mesmo ID (mesma origem=%s)', mesmaOrigem => {
  const server = { id: 'a', label: 'A', baseUrl: 'https://server.test', token: 'token-privado' };
  const create = vi.fn((_url: string, _opts: { withCredentials: boolean }) => ({} as EventSourceLike));
  const novoReq = vi.fn(() => 'nao-deve-gerar');
  configureDiag({ registrar: vi.fn(), novoReq });
  configureApi({ getBaseUrl: () => server.baseUrl, getToken: () => server.token,
    origin: mesmaOrigem ? server.baseUrl : 'https://app.test', onUnauthorized() {}, createEventSource: create });

  openEventStream('sess', 'thread:42', 'chat-1');
  openSessionsStream(server, 'lista-1');
  openEventStreamForServer(server, 'outra', 'comparacao-1');

  const urls = create.mock.calls.map(([url]) => new URL(url));
  expect(urls.map(url => url.searchParams.get('diag_req'))).toEqual(['chat-1', 'lista-1', 'comparacao-1']);
  expect(urls[0].searchParams.get('last_event_id')).toBe('thread:42');
  expect(urls.map(url => url.pathname)).toEqual([
    '/api/sessions/sess/events', '/api/sessions/events', '/api/sessions/outra/events',
  ]);
  for (const url of urls) expect(url.searchParams.get('token')).toBe(mesmaOrigem ? null : server.token);
  for (const [, opts] of create.mock.calls) expect(opts).toEqual({ withCredentials: mesmaOrigem });
  expect(novoReq).not.toHaveBeenCalled();
});

it('gera um ID por abertura quando o chamador não forneceu um', () => {
  const create = vi.fn((_url: string) => ({} as EventSourceLike));
  let sequence = 0;
  configureDiag({ registrar: vi.fn(), novoReq: () => `conexao-${++sequence}` });
  configureApi({ getBaseUrl: () => 'https://server.test', getToken: () => 'token',
    origin: null, onUnauthorized() {}, createEventSource: create });
  openEventStream('sess');
  openEventStream('sess');
  expect(create.mock.calls.map(([url]) => new URL(url).searchParams.get('diag_req')))
    .toEqual(['conexao-1', 'conexao-2']);
});
