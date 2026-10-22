import { createHmac } from 'node:crypto';
import { describe, test, expect, vi, beforeEach, afterEach } from 'vitest';
import { configureApi, configureDiag, definirArmazem, _resetRotasForTests, estaDesligado, registrarFalha, registrarSucesso, retryAfterMs, retentarAgora } from '@hangar/core';
import { prefs } from './prefs';
import * as m from '../paraglide/messages';

const { memory } = vi.hoisted(() => ({ memory: new Map<string, string>() }));
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (key: string) => memory.get(key),
    set: (key: string, value: string) => { memory.set(key, value); },
    remove: (key: string) => { memory.delete(key); },
  }),
}));
import { useServers } from './servers';
import { useSessions, _resetSessionsForTests, markLeavingPair, setPairNotice, setMachineDiscovery } from './sessions';

const avisos: string[] = [];
const cooldownStorage = {
  getItem: (key: string) => prefs.getString(key) ?? null,
  setItem: (key: string, value: string) => prefs.set(key, value),
  removeItem: (key: string) => { prefs.remove(key); },
};

// mock SecureStore para useServers não quebrar se load for chamado
vi.mock('expo-secure-store', () => {
  const m = new Map<string, string>();
  return {
    getItemAsync: async (k: string) => m.get(k) ?? null,
    setItemAsync: async (k: string, v: string) => { m.set(k, v); },
    deleteItemAsync: async (k: string) => { m.delete(k); },
  };
});

// fábrica de EventSource falso injetada via configureApi
type FakeES = {
  url: string;
  opts: Record<string, unknown>;
  listeners: Record<string, ((e: { data: string }) => void)[]>;
  close: ReturnType<typeof vi.fn>;
  trigger: (type: string, data: string) => void;
  onerror: ((e: unknown) => void) | null;
};

let created: FakeES[] = [];
let fetchMock: ReturnType<typeof vi.fn>;

async function settle() {
  for (let i = 0; i < 30; i++) await Promise.resolve();
}

function response(data: unknown, status = 200): Response {
  return { ok: status >= 200 && status < 300, status, json: async () => data } as Response;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

function fakeCreateEventSource(url: string, opts: { withCredentials: boolean; headers?: Record<string, string> }): FakeES & { addEventListener: any; removeEventListener: any; close: any; onerror: any; onopen: any; readyState: number } {
  const fake: FakeES = {
    url,
    opts,
    listeners: {},
    close: vi.fn(),
    onerror: null,
    trigger(type, data) {
      (this.listeners[type] ?? []).forEach((fn) => fn({ data }));
    },
  };
  const wrapper = {
    get url() { return fake.url; },
    addEventListener(type: string, fn: (e: { data: string }) => void) {
      (fake.listeners[type] ??= []).push(fn);
    },
    removeEventListener() {},
    close: fake.close,
    get onerror() { return fake.onerror; },
    set onerror(fn: ((e: unknown) => void) | null) { fake.onerror = fn; },
    get onopen() { return null; },
    set onopen(_: unknown) {},
    get readyState() { return 1; },
    trigger: fake.trigger.bind(fake),
    listeners: fake.listeners,
    opts: fake.opts,
  } as unknown as FakeES & { addEventListener: any; removeEventListener: any; close: any; onerror: any; onopen: any; readyState: number };
  created.push(wrapper as unknown as FakeES);
  return wrapper as any;
}

beforeEach(() => {
  created = [];
  _resetSessionsForTests();
  _resetRotasForTests();
  memory.clear();
  definirArmazem(cooldownStorage);
  fetchMock = vi.fn(async () => response([]));
  vi.stubGlobal('fetch', fetchMock);
  useServers.setState({ servers: [], activeId: null, ready: true, vaultReadable: true, aviso: null });
  configureApi({
    getBaseUrl: () => useServers.getState().active()?.baseUrl ?? '',
    getToken: () => useServers.getState().active()?.token ?? null,
    onUnauthorized: () => {},
    origin: null,
    createEventSource: fakeCreateEventSource as any,
  });
  // um servidor ativo
  const s = { id: 'srv1', label: 'srv1', baseUrl: 'http://localhost:8765', token: 'tok' };
  useServers.setState({ servers: [s], activeId: s.id, ready: true });
});

afterEach(() => {
  setMachineDiscovery(() => {});
  configureDiag({ registrar: () => {}, novoReq: () => '' });
  _resetSessionsForTests();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

test('descoberta espera lista válida e roda uma vez por conexão, inclusive após retomar', async () => {
  const discover = vi.fn();
  setMachineDiscovery(discover);
  const release = useSessions.getState().retain();
  await settle();
  expect(discover).not.toHaveBeenCalled();
  created[0].trigger('sessions', 'invalid');
  expect(discover).not.toHaveBeenCalled();
  created[0].trigger('sessions', '[]');
  created[0].trigger('sessions', '[]');
  expect(discover).toHaveBeenCalledExactlyOnceWith(useServers.getState().servers[0]);
  useSessions.getState().setForeground(false);
  created[0].trigger('sessions', '[]');
  expect(discover).toHaveBeenCalledTimes(1);
  useSessions.getState().setForeground(true);
  await settle();
  expect(discover).toHaveBeenCalledTimes(1);
  created.at(-1)!.trigger('sessions', '[]');
  expect(discover).toHaveBeenCalledTimes(2);
  release();
});

test('descobre Tailscale que ficou disponível depois da primeira lista sem reconectar o SSE', async () => {
  vi.useFakeTimers({ toFake: ['Date'] });
  let available = false;
  const server = { ...useServers.getState().servers[0], lan: { url: 'http://192.168.1.4:8765', id: 'owner' } };
  useServers.setState({ servers: [server] });
  fetchMock.mockImplementation(async (url: string) => {
    const parsed = new URL(url);
    if (parsed.pathname === '/api/alcance') return response({ enderecos: available ? [{ tipo: 'tailscale', url: 'http://100.64.0.2:8765' }] : [] });
    if (parsed.pathname === '/api/peers/prova') {
      const challenge = parsed.searchParams.get('desafio');
      return response({ identificador: 'owner', prova: createHmac('sha256', server.token).update(`${challenge}|owner`).digest('hex') });
    }
    return response([]);
  });
  const release = useSessions.getState().retain();
  await settle();
  created[0].trigger('sessions', '[]');
  await settle();
  expect(useServers.getState().servers[0].remote).toBeUndefined();
  available = true;
  created[0].trigger('ping', '');
  await settle();
  expect(fetchMock.mock.calls.filter(([url]) => String(url).endsWith('/api/alcance'))).toHaveLength(1);
  vi.setSystemTime(Date.now() + 31_000);
  created[0].trigger('ping', '');
  await settle();
  expect(useServers.getState().servers[0].remote).toEqual({ url: 'http://100.64.0.2:8765', id: 'owner' });
  expect(created).toHaveLength(1);
  release();
});

test('dois retain() = um createEventSource; release dos dois = close() uma vez', async () => {
  const rel1 = useSessions.getState().retain();
  await settle();
  expect(created.length).toBe(1);
  const rel2 = useSessions.getState().retain();
  expect(created.length).toBe(1); // segundo retain não abre novo stream
  // ainda aberto
  expect(created[0].close).not.toHaveBeenCalled();
  rel1();
  expect(created[0].close).not.toHaveBeenCalled(); // ainda tem 1 consumidor
  rel2();
  expect(created[0].close).toHaveBeenCalledTimes(1);
});

test('um message com 2 sessões → order() com 2 slots ordenados como o front (aggregateSessions + sortSessions)', async () => {
  useSessions.getState().retain();
  await settle();
  const es = created[0] as unknown as FakeES & { trigger: (t: string, d: string) => void };
  const payload = JSON.stringify([
    { name: 'beta', state: 'idle', jsonl: '/tmp/beta.jsonl' },
    { name: 'alpha', state: 'awaiting_input', jsonl: '/tmp/alpha.jsonl' },
  ]);
  es.trigger('sessions', payload);
  const ordered = useSessions.getState().order();
  expect(ordered).toHaveLength(2);
  // sortSessions: awaiting_input primeiro, depois alfabético
  expect(ordered.map((s) => s.name)).toEqual(['alpha', 'beta']);
  // rows também deve ter 2
  expect(useSessions.getState().rows).toHaveLength(2);
});

test('linha que não mudou mantém o mesmo objeto quando outra sessão do servidor muda', async () => {
  useSessions.getState().retain();
  await settle();
  const es = created[0] as unknown as FakeES & { trigger: (t: string, d: string) => void };
  const beta = { name: 'beta', state: 'idle', jsonl: '/tmp/beta.jsonl', pair_peers: ['x'] };
  es.trigger('sessions', JSON.stringify([beta, { name: 'alpha', state: 'idle', jsonl: '/tmp/alpha.jsonl' }]));
  const before = useSessions.getState().rows;
  es.trigger('sessions', JSON.stringify([beta, { name: 'alpha', state: 'working', jsonl: '/tmp/alpha.jsonl' }]));
  const after = useSessions.getState().rows;
  const pick = (rows: typeof after, name: string) => rows.find((r) => r.name === name);
  expect(pick(after, 'beta')).toBe(pick(before, 'beta'));
  expect(pick(after, 'alpha')).not.toBe(pick(before, 'alpha'));
  expect(pick(after, 'alpha')?.state).toBe('working');
  expect(useSessions.getState().byServer[0].sessions).toContain(pick(before, 'beta'));
});

test('mensagem malformada mantém última lista e mostra erro de dados, não queda de rede', async () => {
  useSessions.getState().retain();
  await settle();
  const es = created[0] as unknown as FakeES & { trigger: (t: string, d: string) => void };
  const good = JSON.stringify([{ name: 'x', state: 'idle', jsonl: '/j/x.jsonl' }]);
  es.trigger('sessions', good);
  expect(useSessions.getState().rows).toHaveLength(1);
  es.trigger('sessions', 'not-json');
  expect(useSessions.getState().byServer[0].loaded).toBe(true);
  expect(useSessions.getState().rows).toHaveLength(1);
  expect(useSessions.getState().byServer[0].error).toBe(m.sessao_erro_servidor());
  expect(estaDesligado('srv1')).toBe(false);
});

test('troca para outro activeId preserva retry; servidor novo abre só a própria conexão', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-10-15T12:00:00Z'));
  const registrar = vi.fn();
  configureDiag({ registrar, novoReq: () => 'lista-teste' });
  const rel = useSessions.getState().retain();
  await settle();
  expect(created.length).toBe(1);
  const es = created[0] as unknown as FakeES & { trigger: (t: string, d: string) => void; onerror: ((e: unknown) => void) | null };
  // O recuo do stream é local, sem prazo persistido para o ativo.
  (es as unknown as { onerror: (e: unknown) => void }).onerror?.({});
  expect(registrar).toHaveBeenCalledWith(expect.objectContaining({ evento: 'lista.retentativa', espera_ms: 2000 }), 'http://localhost:8765');
  expect(created[0].close).toHaveBeenCalled();
  // em backoff, streams foi removido, mas retryTimer pendente
  expect(created.length).toBe(1);
  // mudança irrelevante (activeId) não deve reabrir
  useServers.setState({ activeId: 'outro' });
  expect(created.length).toBe(1);
  // mudança real: adiciona servidor novo → deve abrir só para o novo
  const s2 = { id: 'srv2', label: 'srv2', baseUrl: 'http://localhost:8766', token: 'tok2' };
  const cur = useServers.getState().servers;
  useServers.setState({ servers: [...cur, s2] });
  await settle();
  expect(created.length).toBe(2);
  expect((created[1] as unknown as { url: string }).url).toContain('localhost:8766');
  rel();
});

test('fundo preserva lista e hidden; volta sincroniza uma vez com refs compartilhadas', async () => {
  const first = [
    { name: 'visible', state: 'idle', jsonl: '/j/visible.jsonl' },
    { name: 'hidden', state: 'idle', jsonl: '/j/hidden.jsonl' },
  ];
  fetchMock.mockResolvedValue(response(first));
  const rel1 = useSessions.getState().retain();
  const rel2 = useSessions.getState().retain();
  await settle();
  useSessions.getState().markDeleting('srv1', 'hidden');
  useSessions.getState().setForeground(false);
  expect(created[0].close).toHaveBeenCalledTimes(1);
  expect(useSessions.getState().rows.map((s) => s.name)).toEqual(['visible']);
  expect(useSessions.getState().byServer[0].error).toBeNull();
  rel1();
  useSessions.getState().reconnect();
  useSessions.getState().refreshServers();
  await settle();
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(created).toHaveLength(1);
  fetchMock.mockResolvedValue(response([...first, { name: 'new', state: 'working', jsonl: '/j/new.jsonl' }]));
  useSessions.getState().setForeground(true);
  useSessions.getState().setForeground(true);
  await settle();
  expect(fetchMock).toHaveBeenCalledTimes(2);
  expect(created).toHaveLength(2);
  expect(useSessions.getState().rows.map((s) => s.name)).toEqual(['visible', 'new']);
  rel2();
  expect(created[1].close).toHaveBeenCalledTimes(1);
});

test('retain no fundo e retorno sem consumidores não conectam', async () => {
  useSessions.getState().setForeground(false);
  const rel = useSessions.getState().retain();
  useSessions.getState().refreshServers();
  await settle();
  expect(fetchMock).not.toHaveBeenCalled();
  expect(created).toHaveLength(0);
  rel();
  useSessions.getState().setForeground(true);
  await settle();
  expect(fetchMock).not.toHaveBeenCalled();
  const next = useSessions.getState().retain();
  await settle();
  expect(created).toHaveLength(1);
  next();
});

test('pausa cancela retry; erro tardio do stream fechado não agenda outro', async () => {
  vi.useFakeTimers();
  const rel = useSessions.getState().retain();
  await settle();
  const old = created[0];
  old.onerror?.({});
  useSessions.getState().setForeground(false);
  old.onerror?.({});
  vi.advanceTimersByTime(60_000);
  await settle();
  expect(created).toHaveLength(1);
  expect(fetchMock).toHaveBeenCalledTimes(1);
  useSessions.getState().setForeground(true);
  await settle();
  expect(created).toHaveLength(2);
  old.trigger('sessions', JSON.stringify([{ name: 'late', state: 'idle' }]));
  expect(useSessions.getState().rows).toHaveLength(0);
  rel();
});

test('GET antigo não é abortado na pausa e não publica lista nem stream após a volta', async () => {
  const old = deferred<Response>();
  fetchMock.mockReturnValueOnce(old.promise);
  const rel = useSessions.getState().retain();
  await settle();
  const signal = fetchMock.mock.calls[0][1].signal as AbortSignal;
  useSessions.getState().setForeground(false);
  expect(signal.aborted).toBe(false);
  useSessions.getState().setForeground(true);
  await settle();
  expect(created).toHaveLength(1);
  expect(fetchMock).toHaveBeenCalledTimes(2);
  old.resolve(response([{ name: 'late', state: 'idle' }]));
  await settle();
  expect(useSessions.getState().rows).toHaveLength(0);
  expect(created).toHaveLength(1);
  expect(fetchMock).toHaveBeenCalledTimes(2);
  rel();
});

test.each([false, true])('rota antiga é descartada mesmo com retorno antes de resolver: %s', async (resumeEarly) => {
  const route = deferred<Response>();
  const target = { id: 'srv1', label: 'srv1', baseUrl: 'http://10.0.0.1:8765', token: 'tok' };
  useServers.setState({ servers: [target], activeId: target.id });
  fetchMock.mockReturnValueOnce(route.promise);
  const rel = useSessions.getState().retain();
  useSessions.getState().setForeground(false);
  if (resumeEarly) useSessions.getState().setForeground(true);
  route.resolve(response({}));
  await settle();
  if (!resumeEarly) {
    expect(created).toHaveLength(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    useSessions.getState().setForeground(true);
  }
  await settle();
  expect(created).toHaveLength(1);
  expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
    'http://10.0.0.1:8765/api/peers/identificador',
    'http://10.0.0.1:8765/api/peers/identificador',
    'http://10.0.0.1:8765/api/sessions',
  ]);
  rel();
});

test.each([
  { pending: 'identificador', lan: undefined, abortAt: 4000, resumeEarly: false },
  { pending: 'prova', lan: { url: 'http://192.168.1.2:8765', id: 'home' }, abortAt: 800, resumeEarly: false },
  { pending: 'identificador', lan: undefined, abortAt: 4000, resumeEarly: true },
])('rota pendurada não bloqueia lista nem reconexão após 6s: $pending, retorno=$resumeEarly', async ({ pending, lan, abortAt, resumeEarly }) => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-10-17T12:00:00Z'));
  const controllers: AbortController[] = [];
  const timers: ReturnType<typeof setTimeout>[] = [];
  const timeout = vi.spyOn(AbortSignal, 'timeout').mockImplementation((ms) => {
    const controller = new AbortController();
    controllers.push(controller);
    timers.push(setTimeout(() => controller.abort(new DOMException('timeout', 'TimeoutError')), ms));
    return controller.signal;
  });
  const registrar = vi.fn();
  configureDiag({ registrar, novoReq: () => 'route-test' });
  const target = { id: 'srv1', label: 'srv1', baseUrl: 'http://10.0.0.1:8765', token: 'tok', lan };
  useServers.setState({ servers: [target], activeId: target.id });
  const hung = deferred<Response>();
  let hungSignal: AbortSignal | undefined;
  const unexpected: string[] = [];
  fetchMock.mockImplementation((url: string, init?: RequestInit) => {
    const path = new URL(url).pathname;
    if (path === `/api/peers/${pending}`) {
      hungSignal = init?.signal ?? undefined;
      // O sinal aborta de verdade, mas este transporte continua sem concluir.
      return hung.promise;
    }
    if (url === `${target.baseUrl}/api/peers/identificador`) {
      return Promise.resolve(response({ identificador: 'home', lan_url: lan?.url }));
    }
    if (url === `${target.baseUrl}/api/sessions`) {
      return Promise.resolve(response([{ name: 'principal', state: 'working', jsonl: '/j/principal.jsonl' }]));
    }
    unexpected.push(url);
    return Promise.reject(new Error(`unexpected_request: ${url}`));
  });
  const listRequests = () => fetchMock.mock.calls.filter(([url]) => url === `${target.baseUrl}/api/sessions`);
  const release = useSessions.getState().retain();
  try {
    await settle();
    expect(hungSignal?.aborted).toBe(false);
    if (resumeEarly) {
      await vi.advanceTimersByTimeAsync(1000);
      useSessions.getState().setForeground(false);
      useSessions.getState().setForeground(true);
    }
    await vi.advanceTimersByTimeAsync(abortAt - (resumeEarly ? 1000 : 0));
    expect(hungSignal?.aborted).toBe(true);
    expect(listRequests()).toHaveLength(0);
    expect(created).toHaveLength(0);
    expect(useSessions.getState().loading).toBe(true);
    await vi.advanceTimersByTimeAsync(5999 - abortAt);
    expect(listRequests()).toHaveLength(0);
    expect(created).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    if (resumeEarly) await vi.advanceTimersByTimeAsync(1);
    if (lan) {
      expect(useSessions.getState().rows).toEqual([]);
      expect(useSessions.getState().loading).toBe(false);
      expect(listRequests()).toHaveLength(0);
      expect(created).toHaveLength(0);
      expect(fetchMock.mock.calls.every(([, init]) => !new Headers(init?.headers).has('Authorization'))).toBe(true);
      return;
    }
    if (resumeEarly) await vi.advanceTimersByTimeAsync(6000);
    expect(useSessions.getState().rows.map((row) => row.name)).toEqual(['principal']);
    expect(useSessions.getState().loading).toBe(false);
    expect(listRequests()).toHaveLength(1);
    expect(listRequests()[0][1].headers.Authorization).toBe('Bearer tok');
    expect(created).toHaveLength(1);
    expect(new URL(created[0].url).origin).toBe(target.baseUrl);
    expect(new URL(created[0].url).pathname).toBe('/api/sessions/events');
    expect(new URL(created[0].url).searchParams.get('token')).toBe('tok');
    expect(registrar).toHaveBeenCalledWith(expect.objectContaining({
      evento: 'rota.falhou', codigo: 'route_timeout', nivel: 'aviso',
    }), target.baseUrl);

    created[0].onerror?.({});
    expect(created[0].close).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1999);
    expect(created).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    // A conexão nova não reaproveita a decisão que expirou.
    await vi.advanceTimersByTimeAsync(6001);
    expect(listRequests()).toHaveLength(2);
    expect(created).toHaveLength(2);
    expect(new URL(created[1].url).origin).toBe(target.baseUrl);
    expect(useSessions.getState().loading).toBe(false);
    expect(useSessions.getState().byServer[0].error).toBeNull();
    created[1].trigger('sessions', '[{"name":"reconnected","state":"idle"}]');
    expect(useSessions.getState().rows.map((row) => row.name)).toEqual(['reconnected']);
    expect(fetchMock.mock.calls.filter(([url]) => new URL(url).pathname === `/api/peers/${pending}`)).toHaveLength(resumeEarly ? 3 : 2);
    const proofs = fetchMock.mock.calls.filter(([url]) => new URL(url).pathname === '/api/peers/prova');
    expect(proofs).toHaveLength(lan ? 1 : 0);
    for (const [, init] of proofs) {
      expect(new Headers(init?.headers).has('Authorization')).toBe(false);
      expect(init?.credentials).toBe('omit');
    }
    expect(unexpected).toEqual([]);
  } finally {
    release();
    hung.resolve(response({}, 503));
    await settle();
    for (const timer of timers) clearTimeout(timer);
    for (const controller of controllers) controller.abort();
    timeout.mockRestore();
    _resetRotasForTests();
  }
});

test('prova LAN tardia do token antigo não autoriza rota do token novo', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-10-17T12:00:00Z'));
  const controllers: AbortController[] = [];
  const timers: ReturnType<typeof setTimeout>[] = [];
  const timeout = vi.spyOn(AbortSignal, 'timeout').mockImplementation((ms) => {
    const controller = new AbortController();
    controllers.push(controller);
    timers.push(setTimeout(() => controller.abort(new DOMException('timeout', 'TimeoutError')), ms));
    return controller.signal;
  });
  const target = {
    id: 'srv1', label: 'srv1', baseUrl: 'https://home.tail.ts.net', token: 'old-token',
    lan: { url: 'http://192.168.1.2:8765', id: 'home' },
  };
  useServers.setState({ servers: [target], activeId: target.id });
  const proof = deferred<Response>();
  const currentList = deferred<Response>();
  let challenge = '';
  let proofSignal: AbortSignal | undefined;
  const unexpected: string[] = [];
  fetchMock.mockImplementation((url: string, init?: RequestInit) => {
    const parsed = new URL(url);
    if (url === `${target.baseUrl}/api/peers/identificador`) return Promise.resolve(response({}));
    if (parsed.origin === target.lan.url && parsed.pathname === '/api/peers/prova') {
      challenge ||= parsed.searchParams.get('desafio') ?? '';
      proofSignal ??= init?.signal ?? undefined;
      return proof.promise;
    }
    if ([target.baseUrl, target.lan.url].includes(parsed.origin) && parsed.pathname === '/api/sessions') {
      return currentList.promise;
    }
    unexpected.push(url);
    return Promise.reject(new Error(`unexpected_request: ${url}`));
  });
  const listRequests = () => fetchMock.mock.calls.filter(([url]) => new URL(url).pathname === '/api/sessions');
  const release = useSessions.getState().retain();
  try {
    await settle();
    expect(challenge).toMatch(/^[a-f0-9]{32}$/);
    expect(proofSignal?.aborted).toBe(false);
    useServers.setState({ servers: [{ ...target, token: 'new-token' }] });
    await vi.advanceTimersByTimeAsync(6000);
    expect(proofSignal?.aborted).toBe(true);
    expect(created).toHaveLength(0);
    // Credencial nova exige sondagem nova; o resultado antigo não a autoriza.
    expect(fetchMock.mock.calls.map(([url]) => new URL(url).pathname).filter((path) => path.startsWith('/api/peers/'))).toEqual([
      '/api/peers/identificador', '/api/peers/prova', '/api/peers/identificador', '/api/peers/prova',
    ]);
    await vi.advanceTimersByTimeAsync(1000);
    proof.resolve(response({
      identificador: 'home',
      prova: createHmac('sha256', 'old-token').update(`${challenge}|home`).digest('hex'),
    }));
    await settle();
    // A prova pode chegar entre o GET principal e a abertura do stream.
    currentList.resolve(response([{ name: 'new-token', state: 'working', jsonl: '/j/new-token.jsonl' }]));
    await settle();
    expect(listRequests().map(([url]) => url)).toEqual([`${target.baseUrl}/api/sessions`]);
    expect(new Headers(listRequests()[0][1].headers).get('Authorization')).toBe('Bearer new-token');
    expect(created).toHaveLength(1);
    expect(new URL(created[0].url).origin).toBe(target.baseUrl);
    expect(new URL(created[0].url).pathname).toBe('/api/sessions/events');
    expect(new URL(created[0].url).searchParams.get('token')).toBe('new-token');
    expect(fetchMock.mock.calls.filter(([url, init]) => new URL(url).origin === target.lan.url
      && new Headers(init?.headers).get('Authorization') === 'Bearer new-token')).toEqual([]);
    expect(useSessions.getState().rows.map((row) => row.name)).toEqual(['new-token']);
    expect(useSessions.getState().loading).toBe(false);
    expect(unexpected).toEqual([]);
  } finally {
    release();
    proof.resolve(response({}, 503));
    currentList.resolve(response([]));
    await settle();
    for (const timer of timers) clearTimeout(timer);
    for (const controller of controllers) controller.abort();
    timeout.mockRestore();
    _resetRotasForTests();
  }
});

test('troca de token durante GET descarta resposta antiga e usa destino capturado', async () => {
  const old = deferred<Response>();
  fetchMock.mockReturnValueOnce(old.promise);
  const rel = useSessions.getState().retain();
  await settle();
  const target = { ...useServers.getState().servers[0], token: 'new-token' };
  useServers.setState({ servers: [target], activeId: 'other' });
  old.resolve(response([{ name: 'late', state: 'idle' }]));
  await settle();
  expect(useSessions.getState().rows).toHaveLength(0);
  expect(created).toHaveLength(1);
  expect(created[0].url).toContain('token=new-token');
  expect(fetchMock.mock.calls[1][1].headers.Authorization).toBe('Bearer new-token');
  rel();
});

test('troca de endereço inativo cancela GET antigo sem registrar queda falsa e conecta o novo', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-10-16T12:00:00Z'));
  const first = useServers.getState().servers[0];
  const second = { ...first, id: 'srv2', baseUrl: 'http://localhost:8766', token: 'tok2' };
  const nextBase = 'http://localhost:8767';
  useServers.setState({ servers: [first, second] });
  const old = deferred<Response>();
  const current = deferred<Response>();
  let oldSignal: AbortSignal | undefined;
  const unexpected: string[] = [];
  const diagnostics: unknown[] = [];
  configureDiag({ registrar: (entry) => { if (entry.nivel === 'erro') diagnostics.push(entry); }, novoReq: () => 'lista-teste' });
  fetchMock.mockImplementation((url: string, init?: RequestInit) => {
    if (url === `${second.baseUrl}/api/sessions`) {
      oldSignal = init?.signal ?? undefined;
      oldSignal?.addEventListener('abort', () => old.reject(new TypeError('cancelled_fetch')), { once: true });
      return old.promise;
    }
    if (url === `${nextBase}/api/sessions`) return current.promise;
    if (url === `${first.baseUrl}/api/sessions`) return Promise.resolve(response([{ name: 'healthy', state: 'working' }]));
    if (new URL(url).pathname === '/api/external-pairs') return Promise.resolve(response([]));
    unexpected.push(url);
    return Promise.reject(new Error(`unexpected_request: ${url}`));
  });
  const release = useSessions.getState().retain();
  try {
    await settle();
    expect(oldSignal?.aborted).toBe(false);
    const healthyStream = created.find((es) => es.url.startsWith(first.baseUrl));
    expect(healthyStream).toBeDefined();
    useServers.getState().update(second.id, { baseUrl: nextBase });
    await settle();
    expect(oldSignal?.aborted).toBe(true);
    expect(useServers.getState().activeId).toBe(first.id);
    expect.soft(retryAfterMs(second.id)).toBe(0);
    expect.soft(estaDesligado(second.id)).toBe(false);
    expect(fetchMock.mock.calls.filter(([url]) => url === `${nextBase}/api/sessions`)).toHaveLength(1);
    expect(healthyStream?.close).not.toHaveBeenCalled();
    current.resolve(response([{ name: 'new-address', state: 'working' }]));
    await settle();
    expect(useSessions.getState().rows.map((row) => row.name).sort()).toEqual(['healthy', 'new-address']);
    expect(useSessions.getState().byServer.find((bucket) => bucket.server.id === second.id)?.error).toBeNull();
    expect(created.filter((es) => es.url.startsWith(nextBase))).toHaveLength(1);
    expect(created.some((es) => es.url.startsWith(second.baseUrl))).toBe(false);
    expect(retryAfterMs(second.id)).toBe(0);
    expect(estaDesligado(second.id)).toBe(false);
    expect(unexpected).toEqual([]);
    for (const entry of diagnostics) {
      expect(entry).toMatchObject({ evento: 'api.sem_rede', codigo: 'rede', detalhe: 'GET /api/sessions' });
    }
  } finally {
    release();
    old.resolve(response([]));
    current.resolve(response([]));
    await settle();
  }
});

test('remoção no fundo conserva outro servidor e retoma um stream por servidor restante', async () => {
  const first = useServers.getState().servers[0];
  const second = { ...first, id: 'srv2', baseUrl: 'http://localhost:8766', token: 'tok2' };
  useServers.setState({ servers: [first, second] });
  fetchMock.mockImplementation(async (url: string) => response([
    { name: url.includes('8766') ? 'second' : 'first', state: 'idle', jsonl: url },
  ]));
  const rel = useSessions.getState().retain();
  await settle();
  expect(created).toHaveLength(2);
  useSessions.getState().setForeground(false);
  useServers.setState({ servers: [second], activeId: second.id });
  expect(useSessions.getState().rows.map((s) => s.name)).toEqual(['second']);
  useSessions.getState().setForeground(true);
  await settle();
  expect(created).toHaveLength(3);
  expect(created[2].url).toContain('localhost:8766');
  expect(fetchMock).toHaveBeenCalledTimes(3);
  rel();
});

test('REST sem rede permite SSE recuperar e erro repetido agenda somente um retry', async () => {
  vi.useFakeTimers();
  fetchMock.mockRejectedValueOnce(new TypeError('offline'));
  const rel = useSessions.getState().retain();
  await settle();
  expect(useSessions.getState().byServer[0].error).toBe('offline');
  expect(created).toHaveLength(1);
  created[0].onerror?.({});
  created[0].onerror?.({});
  vi.advanceTimersByTime(1999);
  await settle();
  expect(created).toHaveLength(1);
  vi.advanceTimersByTime(1);
  await settle();
  expect(created).toHaveLength(2);
  expect(fetchMock).toHaveBeenCalledTimes(2);
  created[1].trigger('sessions', JSON.stringify([{ name: 'back', state: 'working' }]));
  expect(useSessions.getState().rows.map((s) => s.name)).toEqual(['back']);
  expect(useSessions.getState().byServer[0].error).toBeNull();
  rel();
});

test('list_error com linhas na tela vai ao diário e não desenha erro do servidor', async () => {
  const diagnostics: { evento?: string; codigo?: string }[] = [];
  configureDiag({ registrar: (entry) => { diagnostics.push(entry); }, novoReq: () => 'lista-teste' });
  const rel = useSessions.getState().retain();
  await settle();
  created[0].trigger('sessions', JSON.stringify([{ name: 'viva', state: 'working' }]));
  created[0].trigger('list_error', '{}');
  expect(useSessions.getState().rows.map((s) => s.name)).toEqual(['viva']);
  expect(useSessions.getState().byServer[0].error).toBeNull();
  expect(diagnostics).toContainEqual(expect.objectContaining({ evento: 'lista.falhou', codigo: 'produtor_falhou' }));
  rel();
});

test('list_error com linhas na tela tira o erro anterior do servidor, como no PWA', async () => {
  const rel = useSessions.getState().retain();
  await settle();
  created[0].trigger('sessions', JSON.stringify([{ name: 'viva', state: 'working' }]));
  created[0].trigger('sessions', '{torto');
  expect(useSessions.getState().byServer[0].error).toBe(m.sessao_erro_servidor());
  created[0].trigger('list_error', '{}');
  expect(useSessions.getState().rows.map((s) => s.name)).toEqual(['viva']);
  expect(useSessions.getState().byServer[0].error).toBeNull();
  rel();
});

test('list_error sem lista na tela mantém o aviso de bloqueio do servidor', async () => {
  const rel = useSessions.getState().retain();
  await settle();
  created[0].trigger('list_error', '{}');
  expect(useSessions.getState().rows).toEqual([]);
  expect(useSessions.getState().byServer[0].error).toBe(m.sessao_erro_servidor());
  rel();
});

describe('recusas e recuperação da lista', () => {
  const outageKey = 'hangar_servidores_desligados';
  const repliesKey = 'hangar_servidores_responderam';
  const second = { id: 'srv2', label: 'srv2', baseUrl: 'http://localhost:8766', token: 'tok2' };

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-10-15T12:00:00Z'));
  });

  async function advance(ms: number) {
    vi.advanceTimersByTime(ms);
    await settle();
  }

  test.each(['REST', 'SSE'])('401 %s remove só o destino não ativo e deixa aviso de token, sem retry', async (transport) => {
    const first = useServers.getState().servers[0];
    useServers.setState({ servers: [first, second] });
    if (transport === 'REST') fetchMock.mockImplementation(async (url: string) => response([], url.includes('8766') ? 401 : 200));
    const rel = useSessions.getState().retain();
    await settle();
    if (transport === 'SSE') created.find((es) => es.url.includes('8766'))!.onerror?.({ xhrStatus: 401 });
    await settle();
    expect(useServers.getState().servers).toEqual([first]);
    expect(useServers.getState().aviso).toEqual({ tipo: 'token', label: second.label });
    expect(created[0].close).not.toHaveBeenCalled();
    const calls = fetchMock.mock.calls.length;
    await advance(1_800_000);
    expect(fetchMock).toHaveBeenCalledTimes(calls);
    rel();
  });

  test('401 antigo não remove token substituído enquanto o GET estava em voo', async () => {
    const old = deferred<Response>();
    fetchMock.mockReturnValueOnce(old.promise);
    const rel = useSessions.getState().retain();
    await settle();
    const server = { ...useServers.getState().servers[0], token: 'fresh' };
    useServers.setState({ servers: [server] });
    old.resolve(response({}, 401));
    await settle();
    expect(useServers.getState().servers).toEqual([server]);
    expect(useServers.getState().aviso).toBeNull();
    expect(created).toHaveLength(1);
    expect(created[0].url).toContain('token=fresh');
    rel();
  });

  test.each(['REST', 'SSE'])('403 %s conserva tokens e voltaforeground libera uma tentativa, sem loop', async (transport) => {
    const first = useServers.getState().servers[0];
    useServers.setState({ servers: [first, second] });
    fetchMock.mockImplementation(async (url: string) => response([{ name: url.includes('8766') ? 'stale-second' : 'first', state: 'idle' }], transport === 'REST' && url.includes('8766') ? 403 : 200));
    let rel = useSessions.getState().retain();
    await settle();
    if (transport === 'SSE') created.find((es) => es.url.includes('8766'))!.onerror?.({ xhrStatus: 403 });
    expect(useServers.getState().servers).toEqual([first, second]);
    expect(useServers.getState().aviso).toBeNull();
    expect(useSessions.getState().byServer.find((b) => b.server.id === second.id)?.error).toBe(m.lista_acesso_recusado());
    expect(useSessions.getState().rows.some((r) => r.serverId === second.id)).toBe(false);
    expect(estaDesligado(second.id)).toBe(false);
    const callsToSecond = () => fetchMock.mock.calls.filter(([url]) => String(url).includes('8766')).length;
    const calls = callsToSecond();
    fetchMock.mockImplementation(async (url: string) => response([], url.includes('8766') ? 403 : 200));
    useSessions.getState().setForeground(false);
    useSessions.getState().setForeground(true);
    await settle();
    expect(callsToSecond()).toBe(calls + 1);
    rel();
    rel = useSessions.getState().retain();
    await advance(1_800_000);
    expect(callsToSecond()).toBe(calls + 1);
    fetchMock.mockResolvedValue(response([]));
    const healthy = created.at(-1)!;
    useSessions.getState().reconnect();
    await settle();
    expect(callsToSecond()).toBe(calls + 2);
    expect(healthy.close).not.toHaveBeenCalled();
    rel();
  });

  test.each(['refresh', 'recovered'])('403 é liberado por %s sem reabrir saudável nem antecipar outro prazo', async (action) => {
    const first = useServers.getState().servers[0];
    const third = { ...second, id: 'srv3', baseUrl: 'http://localhost:8767' };
    useServers.setState({ servers: [first, second, third] });
    prefs.set(outageKey, JSON.stringify({ srv3: { failures: 4, retryAt: Date.now() + 60_000 } }));
    fetchMock.mockImplementation(async (url: string) => response([], url.includes('8766') ? 403 : 200));
    const rel = useSessions.getState().retain();
    await settle();
    if (action === 'recovered') {
      registrarFalha(second.id);
      registrarSucesso(second.id);
    } else {
      retentarAgora(second.id);
      useSessions.getState().refreshServers();
    }
    await settle();
    expect(fetchMock.mock.calls.filter(([url]) => String(url).includes('8766'))).toHaveLength(2);
    expect(created[0].close).not.toHaveBeenCalled();
    expect(retryAfterMs(third.id)).toBe(60_000);
    await advance(5000);
    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(useServers.getState().servers).toEqual([first, second, third]);
    rel();
  });

  test.each(['REST', 'SSE'])('401/410 de convite %s encerra sem apagar entrada nem aviso de token', async (transport) => {
    const invite = { ...second, invite: true };
    useServers.setState({ servers: [invite], activeId: null });
    if (transport === 'REST') fetchMock.mockResolvedValue(response({}, 401));
    const rel = useSessions.getState().retain();
    await settle();
    if (transport === 'SSE') created[0].onerror?.({ xhrStatus: 410 });
    await advance(1_800_000);
    expect(useServers.getState().servers).toEqual([{ ...invite, inviteEnded: true }]);
    expect(useServers.getState().aviso).toBeNull();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    rel();
  });

  test('ativo escala recuo local sem persistir nem dobrar falha REST + SSE; ping não zera', async () => {
    fetchMock.mockRejectedValue(new TypeError('offline'));
    let rel = useSessions.getState().retain();
    await settle();
    for (const [index, delay] of [2000, 5000, 30000, 60000, 120000, 240000, 300000].entries()) {
      created[index].onerror?.({});
      if (index === 1) {
        rel();
        rel = useSessions.getState().retain();
        await settle();
      }
      await advance(delay - 1);
      expect(created).toHaveLength(index + 1);
      await advance(1);
      expect(created).toHaveLength(index + 2);
      expect(estaDesligado('srv1')).toBe(false);
      expect(prefs.getString(outageKey)).toBeUndefined();
    }
    created.at(-1)!.trigger('ping', '');
    created.at(-1)!.onerror?.({});
    await advance(599999);
    expect(created).toHaveLength(8);
    await advance(1);
    expect(created).toHaveLength(9);
    rel();
  });

  test('inativo sem resposta respeita sequência persistida de 2s até teto de 30min, uma queda por tentativa', async () => {
    useServers.setState({ activeId: null });
    fetchMock.mockRejectedValue(new TypeError('offline'));
    const rel = useSessions.getState().retain();
    await settle();
    const delays = [2000, 5000, 30000, 60000, 120000, 240000, 300000, 600000, 1800000, 1800000];
    for (const [index, delay] of delays.entries()) {
      created[index].onerror?.({});
      created[index].onerror?.({});
      expect(retryAfterMs('srv1')).toBe(delay);
      expect(JSON.parse(prefs.getString(outageKey)!).srv1.failures).toBe(Math.min(index + 1, 9));
      await advance(delay - 1);
      expect(created).toHaveLength(index + 1);
      await advance(1);
      expect(created).toHaveLength(index + 2);
    }
    rel();
  });

  test('falha REST já contada não sobe de novo se SSE falhar depois de expirar o prazo', async () => {
    useServers.setState({ activeId: null });
    fetchMock.mockRejectedValue(new TypeError('offline'));
    const rel = useSessions.getState().retain();
    await settle();
    await advance(3000);
    created[0].onerror?.({});
    expect(JSON.parse(prefs.getString(outageKey)!).srv1.failures).toBe(1);
    rel();
  });

  test('prazo guardado sobrevive à recarga, pausa e retain; expirar não comprova recuperação', async () => {
    useServers.setState({ activeId: null });
    prefs.set(outageKey, JSON.stringify({ srv1: { failures: 4, retryAt: Date.now() + 60_000 } }));
    let rel = useSessions.getState().retain();
    await settle();
    expect(fetchMock).not.toHaveBeenCalled();
    useSessions.getState().setForeground(false);
    await advance(10_000);
    useSessions.getState().setForeground(true);
    rel();
    rel = useSessions.getState().retain();
    await advance(49_999);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(estaDesligado('srv1')).toBe(true);
    fetchMock.mockRejectedValue(new TypeError('offline'));
    await advance(1);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(JSON.parse(prefs.getString(outageKey)!).srv1.failures).toBe(5);
    rel();
  });

  test('primeira abertura após carregar servidores libera recente, mas retain seguinte conserva nova espera', async () => {
    const first = useServers.getState().servers[0];
    useServers.setState({ servers: [], activeId: null, ready: false });
    prefs.set(outageKey, JSON.stringify(Object.fromEntries([first, second].map((s) => [s.id, { failures: 4, retryAt: Date.now() + 60_000 }]))));
    prefs.set(repliesKey, JSON.stringify({ srv1: Date.now() - 1000 }));
    fetchMock.mockRejectedValue(new TypeError('offline'));
    let rel = useSessions.getState().retain();
    await settle();
    expect(fetchMock).not.toHaveBeenCalled();
    useServers.setState({ servers: [first, second], ready: true });
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0][0]).toContain('8765/api/sessions');
    expect(retryAfterMs('srv1')).toBe(2000);
    expect(retryAfterMs('srv2')).toBe(60_000);
    created[0].onerror?.({});
    rel();
    rel = useSessions.getState().retain();
    await advance(1999);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(retryAfterMs('srv1')).toBe(5000);
    rel();
  });

  test('volta tenta ativo/recente na hora, antigo espera; fundo ignora falhas e não grava', async () => {
    const first = useServers.getState().servers[0];
    const third = { ...second, id: 'srv3', baseUrl: 'http://localhost:8767' };
    useServers.setState({ servers: [first, second, third] });
    prefs.set(outageKey, JSON.stringify(Object.fromEntries([first, second, third].map((s) => [s.id, { failures: 4, retryAt: Date.now() + 60_000 }]))));
    prefs.set(repliesKey, JSON.stringify({ srv2: Date.now() - 1000, srv3: Date.now() - 25 * 60 * 60_000 }));
    const late = deferred<Response>();
    fetchMock.mockReturnValueOnce(late.promise);
    const rel = useSessions.getState().retain();
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(2);
    useSessions.getState().setForeground(false);
    const stored = prefs.getString(outageKey);
    late.reject(new TypeError('late network failure'));
    await settle();
    registrarFalha('srv2');
    expect(prefs.getString(outageKey)).toBe(stored);
    await advance(5000);
    useSessions.getState().setForeground(true);
    await settle();
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      'http://localhost:8765/api/sessions',
      'http://localhost:8766/api/sessions',
      'http://localhost:8765/api/sessions',
      'http://localhost:8766/api/sessions',
    ]);
    expect(retryAfterMs('srv3')).toBe(55_000);
    rel();
  });

  test.each([false, true])('GET inativo cancelado não grava queda, inclusive se rejeitar após voltar: %s', async (resumeEarly) => {
    useServers.setState({ activeId: null });
    const late = deferred<Response>();
    fetchMock.mockReturnValueOnce(late.promise);
    const rel = useSessions.getState().retain();
    await settle();
    useSessions.getState().setForeground(false);
    if (resumeEarly) useSessions.getState().setForeground(true);
    late.reject(new TypeError('late offline'));
    await settle();
    expect(estaDesligado('srv1')).toBe(false);
    expect(prefs.getString(outageKey)).toBeUndefined();
    await advance(60_000);
    expect(fetchMock).toHaveBeenCalledTimes(resumeEarly ? 2 : 1);
    fetchMock.mockRejectedValue(new TypeError('offline'));
    if (resumeEarly) {
      created.at(-1)!.onerror?.({});
      useSessions.getState().reconnect();
    } else useSessions.getState().setForeground(true);
    await settle();
    expect(retryAfterMs('srv1')).toBe(2000);
    expect(JSON.parse(prefs.getString(outageKey)!).srv1.failures).toBe(1);
    rel();
  });

  test.each([false, true])('503 de GET cancelado não cria recuo HTTP na volta: %s', async (resumeEarly) => {
    useServers.setState({ activeId: null });
    const late = deferred<Response>();
    fetchMock.mockReturnValueOnce(late.promise).mockResolvedValue(response({}, 503));
    const rel = useSessions.getState().retain();
    await settle();
    useSessions.getState().setForeground(false);
    if (resumeEarly) useSessions.getState().setForeground(true);
    late.resolve(response({}, 503));
    await settle();
    if (!resumeEarly) useSessions.getState().setForeground(true);
    await settle();
    expect(created).toHaveLength(0);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    await advance(1999);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    await advance(1);
    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(created).toHaveLength(0);
    rel();
  });

  test('reiniciar o estado da lista restaura o prazo MMKV em vez de iniciar outra tentativa', async () => {
    const server = useServers.getState().servers[0];
    useServers.setState({ activeId: null });
    fetchMock.mockRejectedValue(new TypeError('offline'));
    const release = useSessions.getState().retain();
    await settle();
    created[0].onerror?.({});
    release();
    const stored = prefs.getString(outageKey);
    expect(stored).toBeDefined();
    // Desliga só o armazenamento antes de zerar a memória: o reset não apaga o prazo guardado.
    definirArmazem(null);
    _resetSessionsForTests();
    definirArmazem(cooldownStorage);
    useServers.setState({ servers: [server], activeId: null, ready: true });
    const rel = useSessions.getState().retain();
    await settle();
    expect(prefs.getString(outageKey)).toBe(stored);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await advance(1999);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await advance(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(retryAfterMs(server.id)).toBe(5000);
    rel();
  });

  test('máquina recente limita recuo de rede a 30s; ping confirma vida sem zerar recuo local nem reabrir stream', async () => {
    useServers.setState({ activeId: null });
    registrarSucesso('srv1');
    fetchMock.mockRejectedValue(new TypeError('offline'));
    const rel = useSessions.getState().retain();
    await settle();
    for (const delay of [2000, 5000, 30000, 60000]) {
      created.at(-1)!.onerror?.({});
      expect(retryAfterMs('srv1')).toBe(Math.min(delay, 30000));
      await advance(delay);
    }
    const count = created.length;
    expect(estaDesligado('srv1')).toBe(true);
    created.at(-1)!.trigger('ping', '');
    await settle();
    expect(estaDesligado('srv1')).toBe(false);
    expect(created).toHaveLength(count);
    expect(created.at(-1)!.close).not.toHaveBeenCalled();
    created.at(-1)!.onerror?.({});
    expect(retryAfterMs('srv1')).toBe(2000);
    rel();
  });

  test.each([400, 404, 405, 410, 408, 429, 500, 502, 503, 530].flatMap((status) =>
    ['REST', 'SSE'].map((transport) => ({ status, transport }))))('HTTP $status $transport repete com recuo local, sem virar offline ou contar REST + SSE duas vezes', async ({ status, transport }) => {
    useServers.setState({ activeId: null });
    fetchMock.mockResolvedValue(response(transport === 'REST' ? {} : [], transport === 'REST' ? status : 200));
    const rel = useSessions.getState().retain();
    await settle();
    for (const [index, delay] of [2000, 5000, 30000].entries()) {
      if (transport === 'SSE') {
        created[index].onerror?.({ xhrStatus: status });
        created[index].onerror?.({ xhrStatus: status });
      } else expect(created).toHaveLength(0);
      expect(estaDesligado('srv1')).toBe(false);
      expect(retryAfterMs('srv1')).toBe(0);
      expect(useSessions.getState().byServer[0].error).toBe(m.sessao_erro_servidor());
      await advance(delay - 1);
      expect(fetchMock).toHaveBeenCalledTimes(index + 1);
      expect(useSessions.getState().byServer[0].error).not.toBe('offline');
      await advance(1);
      expect(fetchMock).toHaveBeenCalledTimes(index + 2);
    }
    rel();
  });

  test.each([
    ['REST', false], ['SSE', false], ['REST', true], ['SSE', true],
  ] as const)('503 %s contínuo, ativo=%s, progride até 30min sem ping/recuperação core zerarem recuo HTTP', async (transport, active) => {
    useServers.setState({ activeId: active ? 'srv1' : null });
    fetchMock.mockResolvedValue(response(transport === 'REST' ? {} : [], transport === 'REST' ? 503 : 200));
    const rel = useSessions.getState().retain();
    await settle();
    const delays = [2000, 5000, 30000, 60000, 120000, 240000, 300000, 600000, 1800000, 1800000];
    for (const [index, delay] of delays.entries()) {
      if (transport === 'SSE') {
        created[index].trigger('ping', '');
        created[index].trigger('list_error', '{}');
        created[index].trigger('sessions', 'broken-json');
        created[index].onerror?.({ xhrStatus: 503 });
      } else expect(created).toHaveLength(0);
      registrarFalha('srv1');
      registrarSucesso('srv1');
      await settle();
      expect(estaDesligado('srv1')).toBe(false);
      expect(useSessions.getState().byServer[0].error).toBe(m.sessao_erro_servidor());
      await advance(delay - 1);
      expect(fetchMock).toHaveBeenCalledTimes(index + 1);
      await advance(1);
      expect(fetchMock).toHaveBeenCalledTimes(index + 2);
    }
    if (transport === 'REST') {
      fetchMock.mockResolvedValue(response([]));
      await advance(1800000);
    }
    const calls = fetchMock.mock.calls.length;
    created.at(-1)!.trigger('sessions', '[]');
    created.at(-1)!.onerror?.({ xhrStatus: 503 });
    await advance(1999);
    expect(fetchMock).toHaveBeenCalledTimes(calls);
    await advance(1);
    expect(fetchMock).toHaveBeenCalledTimes(calls + 1);
    rel();
  });

  test.each([
    ['7', 7000], ['Thu, 15 Oct 2026 12:00:07 GMT', 7000], ['invalid', 2000],
    ['Thu, 15 Oct 2026 11:59:00 GMT', 2000], ['999999999999', 1800000],
    ['9'.repeat(400), 1800000], ['Thu, 15 Oct 2099 12:00:00 GMT', 1800000],
  ] as const)('429 REST respeita Retry-After %s sem abrir SSE nem ultrapassar 30min', async (retryAfter, delay) => {
    useServers.setState({ activeId: null });
    fetchMock.mockResolvedValue({ ...response({}, 429), headers: new Headers({ 'Retry-After': retryAfter }) });
    const rel = useSessions.getState().retain();
    await settle();
    expect(created).toHaveLength(0);
    fetchMock.mockResolvedValue(response({}, 503));
    await advance(delay - 1);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(created).toHaveLength(0);
    await advance(1);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(created).toHaveLength(0);
    fetchMock.mockResolvedValue(response([]));
    await advance(4999);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    await advance(1);
    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(created).toHaveLength(1);
    rel();
  });

  test('voltaforeground ignora prazo HTTP uma vez sem zerar a contagem nem antecipar o core de outra máquina', async () => {
    const first = useServers.getState().servers[0];
    useServers.setState({ servers: [first, second], activeId: null });
    prefs.set(outageKey, JSON.stringify({ srv2: { failures: 4, retryAt: Date.now() + 60_000 } }));
    fetchMock.mockResolvedValue(response({}, 503));
    const rel = useSessions.getState().retain();
    await settle();
    expect(created).toHaveLength(0);
    await advance(500);
    useSessions.getState().setForeground(false);
    useSessions.getState().setForeground(true);
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(created).toHaveLength(0);
    expect(retryAfterMs('srv2')).toBe(59_500);
    await advance(4999);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    await advance(1);
    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(created).toHaveLength(0);
    rel();
  });

  test.each([
    { event: {}, error: 'offline', offline: true },
    { event: { xhrStatus: 503 }, error: m.sessao_erro_servidor(), offline: false },
  ])('erro SSE após REST 200 distingue rede de HTTP sem zerar recuo: $error', async ({ event, error, offline }) => {
    useServers.setState({ activeId: null });
    fetchMock.mockResolvedValue(response([]));
    const rel = useSessions.getState().retain();
    await settle();
    for (const [index, delay] of [2000, 5000, 30000].entries()) {
      expect(useSessions.getState().byServer[0].error).toBeNull();
      created[index].onerror?.(event);
      expect(useSessions.getState().byServer[0].error).toBe(error);
      expect(estaDesligado('srv1')).toBe(offline);
      expect(useSessions.getState().rows).toEqual([]);
      expect(created[index].close).toHaveBeenCalledTimes(1);
      await advance(delay - 1);
      expect(created).toHaveLength(index + 1);
      expect(fetchMock).toHaveBeenCalledTimes(index + 1);
      await advance(1);
      expect(created).toHaveLength(index + 2);
      expect(fetchMock).toHaveBeenCalledTimes(index + 2);
    }
    rel();
  });

  test.each([
    { failure: 'rede', error: 'offline', offline: true },
    { failure: 'HTTP 503', error: m.sessao_erro_servidor(), offline: false },
    { failure: 'constructor', error: 'offline', offline: true },
  ].flatMap((failure) => ['foreground', 'retain'].map((resume) => ({ ...failure, resume }))))(
    'classificação da falha sobrevive a $resume durante recuo: $failure', async ({ failure, error, offline, resume }) => {
      useServers.setState({ activeId: null });
      if (offline) fetchMock.mockRejectedValue(new TypeError('offline'));
      if (failure === 'constructor') configureApi({
        getBaseUrl: () => '', getToken: () => null, onUnauthorized: () => {}, origin: null,
        createEventSource: () => { throw new Error('stream_unavailable'); },
      });
      let rel = useSessions.getState().retain();
      await settle();
      if (failure !== 'constructor') created[0].onerror?.(offline ? {} : { xhrStatus: 503 });
      expect(useSessions.getState().byServer[0].error).toBe(error);
      expect(estaDesligado('srv1')).toBe(offline);
      expect(retryAfterMs('srv1')).toBe(offline ? 2000 : 0);
      await advance(500);
      if (!offline) fetchMock.mockResolvedValue(response({}, 503));
      if (resume === 'foreground') {
        useSessions.getState().setForeground(false);
        useSessions.getState().setForeground(true);
      } else {
        rel();
        rel = useSessions.getState().retain();
      }
      await settle();
      expect(fetchMock).toHaveBeenCalledTimes(!offline && resume === 'foreground' ? 2 : 1);
      expect(created).toHaveLength(failure === 'constructor' ? 0 : 1);
      expect(useSessions.getState().byServer[0].error).toBe(error);
      expect(estaDesligado('srv1')).toBe(offline);
      expect(retryAfterMs('srv1')).toBe(offline ? 1500 : 0);
      rel();
    },
  );

  test.each([
    { xhrStatus: 503 }, { xhrStatus: 200, type: 'close' }, {}, { type: 'close' }, { type: 'timeout' },
  ])('REST 200 e ping não zeram recuo após erro SSE $xhrStatus/$type; só lista SSE válida zera', async (error) => {
    useServers.setState({ activeId: null });
    fetchMock.mockResolvedValue(response([{ name: 'rest', state: 'idle' }]));
    const rel = useSessions.getState().retain();
    await settle();
    for (const [index, delay] of [2000, 5000, 30000].entries()) {
      expect(useSessions.getState().rows.map((row) => row.name)).toEqual(['rest']);
      expect(useSessions.getState().byServer[0].error).toBeNull();
      created[index].trigger('ping', '');
      created[index].onerror?.(error);
      created[index].onerror?.(error);
      expect(created[index].close).toHaveBeenCalledTimes(1);
      expect(useSessions.getState().byServer[0].error).not.toBeNull();
      if (error.xhrStatus) {
        expect(useSessions.getState().byServer[0].error).toBe(m.sessao_erro_servidor());
        expect(estaDesligado('srv1')).toBe(false);
      }
      await advance(delay - 1);
      expect(created).toHaveLength(index + 1);
      expect(fetchMock).toHaveBeenCalledTimes(index + 1);
      await advance(1);
      expect(created).toHaveLength(index + 2);
      expect(fetchMock).toHaveBeenCalledTimes(index + 2);
    }
    created[3].trigger('sessions', '[{"name":"live","state":"working"}]');
    expect(useSessions.getState().rows.map((row) => row.name)).toEqual(['live']);
    expect(useSessions.getState().byServer[0].error).toBeNull();
    created[3].onerror?.(error);
    await advance(1999);
    expect(created).toHaveLength(4);
    await advance(1);
    expect(created).toHaveLength(5);
    rel();
  });

  test('falha ao abrir EventSource escala mesmo após lista REST válida', async () => {
    useServers.setState({ activeId: null });
    const open = vi.fn(() => { throw new Error('stream_unavailable'); });
    configureApi({
      getBaseUrl: () => useServers.getState().active()?.baseUrl ?? '',
      getToken: () => useServers.getState().active()?.token ?? null,
      onUnauthorized: () => {},
      origin: null,
      createEventSource: open,
    });
    const rel = useSessions.getState().retain();
    await settle();
    for (const [index, delay] of [2000, 5000, 30000].entries()) {
      expect(useSessions.getState().byServer[0].error).toBe('offline');
      expect(estaDesligado('srv1')).toBe(true);
      await advance(delay - 1);
      expect(open).toHaveBeenCalledTimes(index + 1);
      expect(fetchMock).toHaveBeenCalledTimes(index + 1);
      await advance(1);
      expect(open).toHaveBeenCalledTimes(index + 2);
      expect(fetchMock).toHaveBeenCalledTimes(index + 2);
    }
    rel();
  });

  test.each(['REST', 'SSE'])('reconnect manual após faixa alta %s volta ao primeiro recuo sem reabrir saudável', async (transport) => {
    const first = useServers.getState().servers[0];
    useServers.setState({ servers: [first, second] });
    fetchMock.mockImplementation(async (url: string) => response([], transport === 'REST' && url.includes('8766') ? 503 : 200));
    const rel = useSessions.getState().retain();
    await settle();
    const healthy = created[0];
    for (const delay of [2000, 5000, 30000, 60000]) {
      if (transport === 'SSE') created.at(-1)!.onerror?.({ xhrStatus: 503 });
      await advance(delay);
    }
    if (transport === 'SSE') created.at(-1)!.onerror?.({ xhrStatus: 503 });
    const calls = fetchMock.mock.calls.length;
    useSessions.getState().reconnect();
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(calls + 1);
    expect(healthy.close).not.toHaveBeenCalled();
    if (transport === 'SSE') created.at(-1)!.onerror?.({ xhrStatus: 503 });
    await advance(1999);
    expect(fetchMock).toHaveBeenCalledTimes(calls + 1);
    await advance(1);
    expect(fetchMock).toHaveBeenCalledTimes(calls + 2);
    rel();
  });

  test('novo ativo sai do prazo antigo sem reabrir o stream saudável do outro', async () => {
    const first = useServers.getState().servers[0];
    useServers.setState({ servers: [first, second] });
    prefs.set(outageKey, JSON.stringify({ srv2: { failures: 9, retryAt: Date.now() + 1_800_000 } }));
    const rel = useSessions.getState().retain();
    await settle();
    expect(created).toHaveLength(1);
    useServers.getState().setActive(second.id);
    await settle();
    expect(created).toHaveLength(2);
    expect(created[0].close).not.toHaveBeenCalled();
    rel();
  });

  test('recuperação externa libera só o timer do destino e remoção/token novo esquecem só sua marca', async () => {
    const first = useServers.getState().servers[0];
    useServers.setState({ servers: [first, second], activeId: null });
    prefs.set(outageKey, JSON.stringify(Object.fromEntries([first, second].map((s) => [s.id, { failures: 4, retryAt: Date.now() + 60_000 }]))));
    const rel = useSessions.getState().retain();
    await settle();
    registrarSucesso('srv1');
    await settle();
    expect(created).toHaveLength(1);
    expect(retryAfterMs('srv2')).toBe(60_000);
    useServers.setState({ servers: [first, { ...second, token: 'new' }] });
    await settle();
    expect(created).toHaveLength(2);
    expect(created[0].close).not.toHaveBeenCalled();
    useServers.getState().remove('srv2');
    expect(estaDesligado('srv2')).toBe(false);
    rel();
  });
});

describe('pares de fora', () => {
  const CASA = 'https://casa.ts.net:8443';
  const pair = (token = 'tp', session = 'web') => ({ local_session: 'api', alias: 'casa', owner: 'Ana', session, address: CASA, token });
  const minha = (externals: unknown[]) => [{ name: 'api', state: 'idle', jsonl: '/j/api.jsonl', pair_gid: 'g1',
    pair_externals: externals }];
  let pares: unknown[];
  let deFora: unknown;
  let encerrado = false;

  beforeEach(() => {
    avisos.length = 0;
    setPairNotice((t) => avisos.push(t));
    encerrado = false;
    pares = [pair()];
    deFora = [{ name: 'web', state: 'working', jsonl: '/c/web.jsonl', guest_kind: 'pair' }];
    fetchMock.mockImplementation(async (url: string) => {
      if (url.includes('/api/external-pairs')) return response(pares);
      if (url.includes('/api/guest/attach')) return response({ ok: true });
      if (url.startsWith(CASA)) return encerrado ? ({ ok: false, status: 410, json: async () => ({}) } as Response) : response(deFora);
      return response(minha([{ alias: 'casa', owner: 'Ana', session: 'web', address: CASA, level: 'view', pending: false }]));
    });
  });

  test('a sessão de fora entra no grupo do servidor próprio, sem seção da entrada temporária', async () => {
    const rel = useSessions.getState().retain();
    for (let i = 0; i < 4; i++) await settle();
    const { rows, byServer } = useSessions.getState();
    expect(byServer.map((b) => b.server.id)).toEqual(['srv1']);
    const web = rows.find((r) => r.name === 'web');
    expect(web?.outside).toMatchObject({ owner: 'Ana', localServerId: 'srv1', localSession: 'api' });
    expect(web?.pair_gid).toBe('g1');
    expect(useServers.getState().servers.find((s) => s.ephemeral)?.token).toBe('tp');
    rel();
  });

  test('segundo par na mesma máquina liga o token dele à entrada que já existe', async () => {
    pares = [pair('t1'), pair('t2', 'docs')];
    const rel = useSessions.getState().retain();
    for (let i = 0; i < 4; i++) await settle();
    const attach = fetchMock.mock.calls.find(([url]) => String(url).includes('/api/guest/attach'));
    expect(attach?.[0]).toContain(CASA);
    expect(JSON.parse(String((attach?.[1] as RequestInit).body))).toEqual({ token: 't2' });
    rel();
  });

  test('par que some entre duas leituras avisa que o dono saiu', async () => {
    const rel = useSessions.getState().retain();
    for (let i = 0; i < 4; i++) await settle();
    pares = [];
    created[0].trigger('sessions', JSON.stringify(minha([])));
    for (let i = 0; i < 4; i++) await settle();
    expect(avisos.some((t) => t.includes('Ana'))).toBe(true);
    expect(useServers.getState().servers.some((s) => s.ephemeral)).toBe(false);
    rel();
  });

  test('quem saiu pelo próprio menu não recebe o aviso', async () => {
    const rel = useSessions.getState().retain();
    for (let i = 0; i < 4; i++) await settle();
    markLeavingPair('casa', 'web');
    pares = [];
    created[0].trigger('sessions', JSON.stringify(minha([])));
    for (let i = 0; i < 4; i++) await settle();
    expect(avisos).toEqual([]);
    rel();
  });

  test('410 na entrada do par marca encerrado e não fica tentando', async () => {
    encerrado = true;
    const rel = useSessions.getState().retain();
    for (let i = 0; i < 4; i++) await settle();
    expect(useServers.getState().servers.find((s) => s.ephemeral)?.inviteEnded).toBe(true);
    expect(created.filter((e) => e.url.startsWith(CASA))).toHaveLength(0);
    rel();
  });
});
