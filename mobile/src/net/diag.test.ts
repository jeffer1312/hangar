import { afterEach, expect, it, vi } from 'vitest';
import { configureApi, configureDiag, registrarDiag, novoReqDiag, decidirRota, baseOf, _resetRotasForTests, type Server } from '@hangar/core';
import { hmacSha256Hex } from '../../../packages/core/src/hmac';

const estado = vi.hoisted(() => ({
  base: 'https://a.test',
  servidores: [{ id: 'a', label: 'fixture-a', baseUrl: 'https://a.test', token: 'token-a' }] as Server[],
  appState: vi.fn(),
}));
vi.mock('react-native', () => ({ Platform: { OS: 'android' }, AppState: { addEventListener: estado.appState } }));
vi.mock('expo-updates', () => ({ updateId: 'update-fixture' }));
vi.mock('expo-constants', () => ({ default: { expoConfig: { version: '0.1.0' } } }));
vi.mock('../stores/prefs', () => ({ prefs: { getString: () => undefined } }));
vi.mock('../stores/servers', () => ({ useServers: {
  getState: () => ({ servers: estado.servidores, active: () => ({ baseUrl: estado.base }) }),
  subscribe: vi.fn(),
} }));
import { iniciarDiag, withMachineRequestDiagnostics } from './diag';

// O servidor ativo é estado do módulo: um teste que o troca não pode valer para o seguinte.
afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.restoreAllMocks(); estado.base = 'https://a.test'; });

it('instala sink uma vez, grava plataforma e eventos no destino capturado', async () => {
  vi.useFakeTimers();
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"gravadas":2}'));
  iniciarDiag(() => ({ valores: { theme: 'dark' }, aparelho: 'dev-1' })); iniciarDiag();
  expect(estado.appState).toHaveBeenCalledTimes(1);
  const req = novoReqDiag();
  expect(req).not.toBe('');
  registrarDiag({ evento: 'acao', req });
  estado.base = 'https://b.test';
  await vi.advanceTimersByTimeAsync(4000);
  expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/diag');
  const eventos = JSON.parse(fetchMock.mock.calls[0][1]!.body as string).eventos;
  expect(eventos).toEqual([
    expect.objectContaining({ evento: 'app.abriu', so: 'android', versao: '0.1.0' }),
    expect.objectContaining({ evento: 'acao', req, cliente: 'android' }),
  ]);
  expect(JSON.stringify(eventos)).not.toMatch(/token-a|https:/);

  // A cada passada em primeiro plano, o tempo na tela vai ao servidor ativo.
  estado.base = 'https://a.test';
  fetchMock.mockClear();
  fetchMock.mockImplementation(async (_url, init) =>
    new Response(JSON.stringify({ gravadas: JSON.parse(init!.body as string).eventos.length })));
  await vi.advanceTimersByTimeAsync(300_000 - 4000);
  const visibleTime = fetchMock.mock.calls.flatMap(([url, init]) => JSON.parse(init!.body as string).eventos
    .filter((e: { evento: string }) => e.evento === 'app.visible_time').map((e: object) => [url, e]));
  expect(visibleTime).toEqual([['https://a.test/api/diag', expect.objectContaining({ ms: 300_000, cliente: 'android' })]]);

  // O retrato das preferências sai uma vez por dia, ao servidor ativo.
  const snapshots = () => fetchMock.mock.calls.flatMap(([, init]) => JSON.parse(init!.body as string).eventos
    .filter((e: { evento: string }) => e.evento === 'settings.snapshot'));
  expect(snapshots()).toEqual([expect.objectContaining({ valores: { theme: 'dark' }, aparelho: 'dev-1', cliente: 'android' })]);
  fetchMock.mockClear();
  await vi.advanceTimersByTimeAsync(300_000);
  expect(snapshots()).toEqual([]);
});

it.each(['401', 'timeout', 'network', 'cancelled'] as const)('registra falha %s do pedido sem token, corpo ou query', async (kind) => {
  const record = vi.fn();
  configureDiag({ registrar: record, novoReq: () => '' });
  const error = kind === 'network' ? new TypeError('token-secret')
    : new DOMException('token-secret', kind === 'timeout' ? 'TimeoutError' : 'AbortError');
  const response = new Response('{"detail":"token-secret"}', { status: 401 });
  const fetcher = vi.fn<typeof fetch>(async () => {
    if (kind === '401') return response;
    throw error;
  });
  const request = withMachineRequestDiagnostics(fetcher);
  const url = 'https://a.test/api/peers/app-list?token=token-secret';
  const init = { headers: { Authorization: 'Bearer token-secret' } };
  if (kind === '401') expect(await request(url, init)).toBe(response);
  else await expect(request(url, init)).rejects.toBe(error);
  expect(fetcher).toHaveBeenCalledWith(url, init);
  expect(record).toHaveBeenCalledWith(expect.objectContaining({
    evento: 'machines.request_failed', codigo: kind === '401' ? '401' : kind,
    detalhe: 'GET /api/peers/app-list',
    operacao: 'a.test',
  }), 'https://a.test');
  expect(JSON.stringify(record.mock.calls)).not.toContain('token-secret');
});

it('falha de candidata nunca envia diagnóstico a convite ativo', async () => {
  const original = estado.servidores;
  const record = vi.fn();
  configureDiag({ registrar: record, novoReq: () => '' });
  const request = withMachineRequestDiagnostics(vi.fn(async () => new Response('{}', { status: 503 })));
  const guest = { id: 'guest', label: 'fixture-guest', baseUrl: 'https://guest.test', token: 'guest-token', invite: true };
  try {
    estado.base = guest.baseUrl;
    estado.servidores = [guest, ...original];
    await request('https://candidate.test/api/sessions');
    expect(record).toHaveBeenCalledWith(expect.objectContaining({ operacao: 'candidate.test' }), original[0].baseUrl);
    record.mockClear();
    estado.servidores = [guest];
    await request('https://candidate.test/api/sessions');
    expect(record).not.toHaveBeenCalled();
  } finally { estado.servidores = original; }
});

it('preserva qual peer falhou ao obter a credencial, sem copiar a credencial', async () => {
  const record = vi.fn();
  configureDiag({ registrar: record, novoReq: () => '' });
  const request = withMachineRequestDiagnostics(vi.fn(async () => new Response('{}', { status: 503 })));
  await request('https://a.test/api/peers/remote-a/token', { headers: { Authorization: 'Bearer secret' } });
  await request('https://a.test/api/peers/remote-b/token', { headers: { Authorization: 'Bearer secret' } });
  expect(record).toHaveBeenCalledWith(expect.objectContaining({ detalhe: 'GET /api/peers/remote-a/token' }), 'https://a.test');
  expect(record).toHaveBeenCalledWith(expect.objectContaining({ detalhe: 'GET /api/peers/remote-b/token' }), 'https://a.test');
  expect(JSON.stringify(record.mock.calls)).not.toContain('secret');
});

it('diagnóstico usa a rota LAN já confirmada', async () => {
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized: () => {}, origin: null, createEventSource: vi.fn() });
  const original = estado.servidores;
  const source = { ...original[0], lan: { id: 'home', url: 'http://192.168.0.5:8765' } };
  let resolve!: (response: Response) => void;
  const principal = new Promise<Response>((done) => { resolve = done; });
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input);
    if (url.startsWith(source.baseUrl)) return principal;
    const challenge = new URL(url).searchParams.get('desafio');
    return new Response(JSON.stringify({ identificador: 'home', prova: hmacSha256Hex(source.token, `${challenge}|home`) }));
  });
  try {
    await decidirRota(source);
    expect(baseOf(source)).toBe(source.lan.url);
    resolve(new Response(JSON.stringify({ identificador: 'home', lan_url: source.lan.url })));
    estado.servidores = [source];
    const record = vi.fn();
    configureDiag({ registrar: record, novoReq: () => '' });
    const request = withMachineRequestDiagnostics(vi.fn(async () => new Response('{}', { status: 503 })));
    await request(`${source.lan.url}/api/peers/app-list`);
    expect(record).toHaveBeenCalledWith(expect.objectContaining({ codigo: '503' }), source.lan.url);
  } finally {
    resolve(new Response('{}'));
    estado.servidores = original;
    _resetRotasForTests();
  }
});
