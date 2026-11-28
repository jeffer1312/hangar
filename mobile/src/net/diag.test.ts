import { afterEach, expect, it, vi } from 'vitest';
import { configureApi, configureDiag, registrarDiag, novoReqDiag, decidirRota, baseOf, _resetRotasForTests, type Server } from '@hangar/core';
import { hmacSha256Hex } from '../../../packages/core/src/hmac';

const estado = vi.hoisted(() => ({
  base: 'https://a.test',
  servidores: [{ id: 'a', label: 'fixture-a', baseUrl: 'https://a.test', token: 'token-a' },
    { id: 'b', label: 'fixture-b', baseUrl: 'https://b.test', token: 'token-b' }] as Server[],
  appState: vi.fn(),
  changed: undefined as (() => void) | undefined,
}));
vi.mock('react-native', () => ({ Platform: { OS: 'android' }, AppState: { addEventListener: estado.appState } }));
vi.mock('expo-updates', () => ({ updateId: 'update-fixture' }));
vi.mock('expo-constants', () => ({ default: { expoConfig: { version: '0.1.0' } } }));
vi.mock('../stores/prefs', () => ({ prefs: { getString: () => undefined } }));
vi.mock('../stores/servers', () => ({ useServers: {
  getState: () => ({ servers: estado.servidores, active: () => estado.servidores.find((s) => s.baseUrl === estado.base) }),
  subscribe: (fn: () => void) => { estado.changed = fn; },
} }));
import { iniciarDiag, onTelemetrySaved, withMachineRequestDiagnostics } from './diag';

// O servidor ativo é estado do módulo: um teste que o troca não pode valer para o seguinte.
afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.restoreAllMocks(); estado.base = 'https://a.test'; });

it('separa retrato do diário, repete após 503 e descarta leitura de máquina recadastrada', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-27T23:45:00Z'));
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized: () => {}, origin: null, createEventSource: vi.fn() });
  const [first, second] = estado.servidores;
  const updated = { ...second, id: 'new-b', token: 'new-token-b' };
  const accepted = { consent: true, consent_scope: 2, enabled: true, disabled_by: null, snapshot_period: 'a'.repeat(32) } as const;
  const newConsent = { ...accepted, snapshot_period: 'b'.repeat(32) };
  let legacy = true;
  let failOnce = true;
  let oldB!: (response: Response) => void;
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async (url, init) => {
    if (String(url).endsWith('/api/telemetry')) {
      if (init?.method === 'POST') {
        if (failOnce) { failOnce = false; return new Response('{}', { status: 503 }); }
        return new Response(JSON.stringify({ snapshot_period: JSON.parse(init.body as string).snapshot_period }));
      }
      const auth = new Headers(init?.headers).get('Authorization');
      if (auth === `Bearer ${second.token}`) return new Promise((resolve) => { oldB = resolve; });
      if (auth === `Bearer ${updated.token}`) return new Response(JSON.stringify(newConsent));
      return new Response(JSON.stringify(legacy ? { consent: true, disabled_by: null } : accepted));
    }
    return new Response(JSON.stringify({ gravadas: JSON.parse(init!.body as string).eventos.length }));
  });
  const settings = vi.fn(() => ({ values: { theme: 'dark' }, device: 'dev-1' }));
  const posts = (base: string, path: string) => fetchMock.mock.calls.filter(([url, init]) =>
    url === `${base}/api/${path}` && init?.method === 'POST');
  const snapshots = (base: string) => posts(base, 'telemetry').map(([, init]) => JSON.parse(init!.body as string));

  iniciarDiag(settings); iniciarDiag();
  expect(estado.appState).toHaveBeenCalledTimes(1);
  const req = novoReqDiag();
  registrarDiag({ evento: 'acao', req });
  estado.base = second.baseUrl;
  await vi.advanceTimersByTimeAsync(4000);
  const common = JSON.parse(posts(first.baseUrl, 'diag')[0][1]!.body as string).eventos;
  expect(common).toEqual([
    expect.objectContaining({ evento: 'app.abriu', so: 'android', versao: '0.1.0' }),
    expect.objectContaining({ evento: 'acao', req, cliente: 'android' }),
  ]);
  expect(JSON.stringify(common)).not.toMatch(/token-a|https:/);

  estado.base = first.baseUrl;
  await vi.advanceTimersByTimeAsync(296_000);
  expect(settings).not.toHaveBeenCalled();
  legacy = false;
  onTelemetrySaved(first, accepted);
  await vi.advanceTimersByTimeAsync(0);
  expect(snapshots(first.baseUrl)).toEqual([expect.objectContaining({ snapshot_period: accepted.snapshot_period,
    values: { theme: 'dark' }, device: 'dev-1', ts: '2026-11-27T23:50:00.000Z' })]);
  await vi.advanceTimersByTimeAsync(599_000);
  expect(snapshots(first.baseUrl).map((body) => body.ts)).toEqual(['2026-11-27T23:50:00.000Z', '2026-11-27T23:55:00.000Z']);
  expect(JSON.stringify(posts(first.baseUrl, 'diag'))).not.toMatch(/settings\.snapshot|snapshot_period|dev-1/);

  const original = estado.servidores;
  try {
    estado.base = second.baseUrl;
    estado.changed?.();
    expect(oldB).toBeDefined();
    estado.servidores = [first, updated];
    estado.changed?.();
    oldB?.(new Response(JSON.stringify(accepted)));
    await vi.advanceTimersByTimeAsync(0);
    expect(snapshots(second.baseUrl)).toEqual([expect.objectContaining({ snapshot_period: newConsent.snapshot_period,
      ts: '2026-11-27T23:59:59.000Z' })]);
    await vi.advanceTimersByTimeAsync(1000);
    expect(snapshots(second.baseUrl).map((body) => body.ts)).toEqual(['2026-11-27T23:59:59.000Z', '2026-11-28T00:00:00.000Z']);
  } finally { estado.servidores = original; }
});

it('não relê o aceite no mesmo dia depois do retrato confirmado (aceite do escopo 3)', async () => {
  vi.resetModules();
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-27T10:00:00Z'));
  const core = await import('@hangar/core');
  core.configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized: () => {}, origin: null, createEventSource: vi.fn() });
  const fresh = await import('./diag');
  const accepted = { consent: true, consent_scope: 3, enabled: true, disabled_by: null, snapshot_period: 'a'.repeat(32) };
  const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async (url, init) => {
    if (String(url).endsWith('/api/telemetry')) {
      return new Response(JSON.stringify(init?.method === 'POST' ? { snapshot_period: accepted.snapshot_period } : accepted));
    }
    return new Response(JSON.stringify({ gravadas: JSON.parse(init!.body as string).eventos.length }));
  });
  const calls = (post: boolean) => fetchMock.mock.calls.filter(([url, init]) =>
    String(url).endsWith('/api/telemetry') && (init?.method === 'POST') === post).length;
  fresh.iniciarDiag(() => ({ values: { theme: 'dark' }, device: 'dev-1' }));
  await vi.advanceTimersByTimeAsync(0);
  expect([calls(false), calls(true)]).toEqual([1, 1]);
  await vi.advanceTimersByTimeAsync(60 * 60 * 1000);
  expect([calls(false), calls(true)]).toEqual([1, 1]);
  await vi.advanceTimersByTimeAsync(14 * 60 * 60 * 1000);
  expect([calls(false), calls(true)]).toEqual([2, 2]);
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
  estado.base = 'https://no-active.test';
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
    expect(record).toHaveBeenCalledWith(expect.objectContaining({ codigo: '503' }), source.baseUrl);
  } finally {
    resolve(new Response('{}'));
    estado.servidores = original;
    _resetRotasForTests();
  }
});
