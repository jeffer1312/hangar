import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createHash, createHmac } from 'node:crypto';
import { apiEnv, configureApi, _resetApiEnvForTests } from './apiEnv';
import { fetchCostsForServer, openEventStream, openEventStreamForServer, openSessionsStream, probeServerResponse } from './api';
import { hmacSha256Hex, sha256Hex } from './hmac';
import { baseOf, decidirRota, discoverConnectRoute, discoverRemoteRoute, esquecerRota, readConnectUrl, routeKind, verifyServerIdentity, _resetRotasForTests } from './rota';
import type { LanInfo, Server } from './servers';

const TS = 'https://maq.tail.ts.net';
const LAN = 'http://192.168.77.142:8765';

function env(origin: string | null, lembrados: Record<string, LanInfo> = {}) {
  configureApi({
    getBaseUrl: () => '', getToken: () => null, onUnauthorized: () => {}, origin,
    createEventSource: () => { throw new Error('sem SSE'); },
    rememberLan: (id, lan) => { lembrados[id] = lan; },
  });
  return lembrados;
}

// Cada endereço é uma máquina com nome, token e atraso em ms (ou nada no ar, com `null`).
type Maquina = { identificador: string; token: string; lan_url?: string; atraso?: number; proofKey?: string; connect?: string; receivedHost?: string } | null;
function rede(maquinas: Record<string, Maquina>) {
  const chamadas: { url: string; auth: string | null }[] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    const auth = (init?.headers as Record<string, string> | undefined)?.Authorization ?? null;
    chamadas.push({ url, auth });
    const u = new URL(url);
    const m = maquinas[u.origin];
    if (!m) throw new TypeError('rede');
    await new Promise((r) => setTimeout(r, m.atraso ?? 0));
    if (u.pathname === '/api/peers/prova') {
      const d = u.searchParams.get('desafio')!;
      const bound = u.searchParams.has('device') ? `|${m.receivedHost ?? u.host}` : '';
      const prova = createHmac('sha256', m.proofKey ?? m.token).update(`${d}|${m.identificador}${bound}`).digest('hex');
      return new Response(JSON.stringify({ identificador: m.identificador, prova }));
    }
    if (auth !== `Bearer ${m.token}`) return new Response('{}', { status: 401 });
    if (u.pathname === '/api/alcance') return new Response(JSON.stringify({ enderecos: [], connect: m.connect ?? null }));
    return new Response(JSON.stringify({ identificador: m.identificador, lan_url: m.lan_url ?? '' }));
  }));
  return chamadas;
}

const srv = (lan?: LanInfo): Server => ({ id: 's1', label: 'maq', baseUrl: TS, token: 't', lan });
// O Tailscale leva 100 ms; a rede local de verdade responde na hora.
const maq = { identificador: 'maq', token: 't', lan_url: LAN, atraso: 100 };
const maqLocal = { ...maq, atraso: 0 };

beforeEach(() => { _resetRotasForTests(); _resetApiEnvForTests(); });
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

describe('verifyServerIdentity', () => {
  it('aceita a prova HMAC do destino exato sem transmitir o token', async () => {
    env(null);
    const chamadas = rede({ [LAN]: maqLocal });
    const signal = new AbortController().signal;

    expect(await verifyServerIdentity(srv({ url: 'http://cached.test', id: 'cached' }), { url: LAN, id: 'maq' }, signal)).toBe(true);

    expect(chamadas).toHaveLength(1);
    const url = new URL(chamadas[0].url);
    expect(url.origin).toBe(LAN);
    expect(url.pathname).toBe('/api/peers/prova');
    expect([...url.searchParams.keys()]).toEqual(['desafio']);
    expect(url.searchParams.get('desafio')).toMatch(/^[a-f0-9]{32}$/);
    expect(chamadas[0].auth).toBeNull();
    const init = vi.mocked(fetch).mock.calls[0][1];
    expect(init?.signal).toBe(signal);
    expect(init?.credentials).toBe('omit');
    expect(init?.body).toBeUndefined();
  });

  it.each([
    { name: 'identificador-diferente', identificador: 'other', token: 't' },
    { name: 'mesmo-identificador-hmac-falso', identificador: 'maq', token: 'wrong' },
  ])('recusa $name sem tentar autenticação', async ({ identificador, token }) => {
    env(null);
    const chamadas = rede({ [LAN]: { identificador, token } });

    expect(await verifyServerIdentity(srv(), { url: LAN, id: 'maq' })).toBe(false);
    expect(chamadas).toHaveLength(1);
    expect(chamadas.every((call) => call.auth === null)).toBe(true);
  });

  it.each(['rede', 'http', 'json'])('recusa falha de %s sem alternativa autenticada', async (failure) => {
    env(null);
    const fetch = vi.fn(async () => {
      if (failure === 'rede') throw new TypeError('offline');
      return failure === 'http' ? new Response('{}', { status: 503 }) : new Response('invalid-json');
    });
    vi.stubGlobal('fetch', fetch);

    expect(await verifyServerIdentity(srv(), { url: LAN, id: 'maq' })).toBe(false);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(new Headers(vi.mocked(globalThis.fetch).mock.calls[0][1]?.headers).has('Authorization')).toBe(false);
  });

  it('mantém o prazo LAN padrão quando não recebe sinal', async () => {
    env(null);
    rede({ [LAN]: maqLocal });
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    try {
      expect(await verifyServerIdentity(srv(), { url: LAN, id: 'maq' })).toBe(true);
      expect(timeout).toHaveBeenCalledWith(800);
    } finally { timeout.mockRestore(); }
  });
});

describe('rota até o servidor', () => {
  it('a prova HTTP do principal impede uma LAN mais lenta de vencer durante a leitura do identificador', async () => {
    env(null);
    vi.useFakeTimers();
    const primary = 'http://100.64.0.2:8765';
    rede({ [primary]: { ...maq, atraso: 100 }, [LAN]: { ...maqLocal, atraso: 150 } });
    const saved = { ...srv({ url: LAN, id: 'maq' }), baseUrl: primary };
    const deciding = decidirRota(saved);
    await vi.advanceTimersByTimeAsync(250);
    await deciding;

    expect(baseOf(saved)).toBe(primary);
  });

  it('cancelar o pedido interrompe a espera pela prova sem enviar token', async () => {
    env(null);
    let finish!: (response: Response) => void;
    const pending = new Promise<Response>((resolve) => { finish = resolve; });
    vi.stubGlobal('fetch', vi.fn(() => pending));
    const saved = { ...srv({ url: LAN, id: 'maq' }), baseUrl: LAN };
    const controller = new AbortController();
    let rejected = false;
    const outcome = probeServerResponse(saved, '/api/costs', { signal: controller.signal }).catch(() => { rejected = true; });
    controller.abort();
    try {
      for (let i = 0; i < 10; i++) await Promise.resolve();
      expect(rejected).toBe(true);
    } finally {
      finish(new Response('{}', { status: 503 }));
      await outcome;
    }
    expect(vi.mocked(fetch).mock.calls.every(([, init]) => !new Headers(init?.headers).has('Authorization'))).toBe(true);
  });

  it('não autentica o relatório enquanto a prova LAN está pendente ou recusada', async () => {
    env(null);
    let finish!: (response: Response) => void;
    const pending = new Promise<Response>((resolve) => { finish = resolve; });
    const calls: { url: string; auth: string | null }[] = [];
    vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url, auth: new Headers(init?.headers).get('Authorization') });
      if (url.startsWith(`${LAN}/api/peers/prova?`)) return pending;
      return new Response(JSON.stringify({ totals: {}, applied: { period: '30d' } }));
    }));
    const saved = { ...srv({ url: LAN, id: 'maq' }), baseUrl: LAN };
    const outcome = fetchCostsForServer(saved, '30d', false, true).then(() => null, (error: unknown) => error);
    try {
      expect(calls.filter((call) => call.auth !== null)).toHaveLength(0);
      expect(baseOf(saved)).not.toBe(LAN);
      configureApi({ ...apiEnv(), getBaseUrl: () => baseOf(saved) });
      for (const open of [() => openSessionsStream(saved), () => openEventStreamForServer(saved, 'fixture'), () => openEventStream('fixture')]) {
        expect(open).toThrowError(expect.objectContaining({ code: 'server_identity_unconfirmed' }));
      }
    } finally {
      finish(new Response(JSON.stringify({ identificador: 'maq', prova: 'invalid-owner-proof' })));
    }

    expect(await outcome).toMatchObject({ code: 'server_identity_unconfirmed' });
    expect(calls.filter((call) => call.auth !== null)).toHaveLength(0);
  });

  it('não envia bearer ao IP principal LAN conhecido antes da prova em outra rede', async () => {
    env(null);
    const calls = rede({ [LAN]: { ...maqLocal, token: 'another-owner' }, [TS]: maqLocal });
    const saved = { ...srv({ url: LAN, id: 'maq' }), baseUrl: LAN, remote: { url: TS, id: 'maq' } };

    await decidirRota(saved);

    expect(baseOf(saved)).toBe(TS);
    expect(calls.filter((call) => call.url.startsWith(LAN)).every((call) => call.auth === null)).toBe(true);
  });

  it('não deixa uma LAN lenta vencer o principal enquanto confere um IP recém-descoberto', async () => {
    env(null);
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const renewed = 'http://192.168.77.150:8765';
    let finishMain!: (r: Response) => void;
    let finishOld!: (r: Response) => void;
    let finishNew!: (r: Response) => void;
    let sawNew!: () => void;
    const main = new Promise<Response>((resolve) => { finishMain = resolve; });
    const old = new Promise<Response>((resolve) => { finishOld = resolve; });
    const fresh = new Promise<Response>((resolve) => { finishNew = resolve; });
    const requested = new Promise<void>((resolve) => { sawNew = resolve; });
    let oldUrl = '';
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url === `${TS}/api/peers/identificador`) return main;
      if (url.startsWith(`${LAN}/api/peers/prova?`)) { oldUrl = url; return old; }
      if (url.startsWith(`${renewed}/api/peers/prova?`)) { sawNew(); return fresh; }
      throw new Error('unexpected_request');
    }));
    const choosing = decidirRota(srv({ url: LAN, id: 'maq' }));
    vi.setSystemTime(10);
    finishMain(new Response(JSON.stringify({ identificador: 'maq', lan_url: renewed })));
    await requested;
    vi.setSystemTime(20);
    const challenge = new URL(oldUrl).searchParams.get('desafio')!;
    finishOld(new Response(JSON.stringify({ identificador: 'maq', prova: createHmac('sha256', 't').update(`${challenge}|maq`).digest('hex') })));
    try {
      await choosing;
      expect(baseOf(srv())).toBe(TS);
    } finally {
      finishNew(new Response('{}', { status: 503 }));
    }
  });

  it('aprende o novo LAN pelo Tailscale quando o cadastro principal era o IP antigo', async () => {
    const remembered = env(null);
    const renewed = 'http://192.168.77.150:8765';
    rede({ [TS]: { ...maq, lan_url: renewed }, [renewed]: maqLocal });
    const saved = { ...srv({ url: LAN, id: 'maq' }), baseUrl: LAN, remote: { url: TS, id: 'maq' } };

    await decidirRota(saved);

    expect(baseOf(saved)).toBe(renewed);
    expect(remembered.s1).toEqual({ url: renewed, id: 'maq' });
  });

  it('tenta descobrir Tailscale novamente após o intervalo quando ele estava desligado', async () => {
    env(null);
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-10-22T12:00:00Z'));
    let available = false;
    let reads = 0;
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url === `${LAN}/api/alcance`) {
        reads++;
        return new Response(JSON.stringify({ enderecos: available ? [{ tipo: 'tailscale', url: TS }] : [] }));
      }
      const challenge = new URL(url).searchParams.get('desafio')!;
      return new Response(JSON.stringify({ identificador: 'maq', prova: createHmac('sha256', 't').update(`${challenge}|maq`).digest('hex') }));
    }));
    const saved = { ...srv({ url: LAN, id: 'maq' }), baseUrl: LAN };
    await decidirRota(saved);
    expect(await discoverRemoteRoute(saved)).toBeNull();
    available = true;
    expect(await discoverRemoteRoute(saved)).toBeNull();
    expect(reads).toBe(1);
    vi.setSystemTime(new Date('2026-10-22T12:00:31Z'));

    expect(await discoverRemoteRoute(saved)).toEqual({ id: 'maq', url: TS });
    expect(reads).toBe(2);
  });

  it('uma decisão antiga não volta depois que o cadastro troca de endereço', async () => {
    env(null);
    let finish!: (response: Response) => void;
    const pending = new Promise<Response>((resolve) => { finish = resolve; });
    const changed: Server = { ...srv({ url: '', id: 'maq' }), baseUrl: 'https://new.tail.ts.net', token: 'new-token' };
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      if (url.startsWith(`${LAN}/api/peers/prova?`)) return pending;
      if (url === `${changed.baseUrl}/api/peers/identificador`) return new Response(JSON.stringify({ identificador: 'maq', lan_url: '' }));
      return new Response('{}', { status: 503 });
    }));
    const original = decidirRota(srv({ url: LAN, id: 'maq' }));
    const proofUrl = String(vi.mocked(fetch).mock.calls.find(([url]) => String(url).startsWith(LAN))![0]);
    esquecerRota('s1');
    const next = decidirRota(changed);
    const challenge = new URL(proofUrl).searchParams.get('desafio')!;
    finish(new Response(JSON.stringify({ identificador: 'maq', prova: createHmac('sha256', 't').update(`${challenge}|maq`).digest('hex') })));
    await Promise.all([original, next]);

    expect(baseOf(changed)).toBe(changed.baseUrl);
  });

  it('usa o endereço LAN renovado na mesma conexão depois de uma mudança de DHCP', async () => {
    env(null);
    const renewed = 'http://192.168.77.150:8765';
    rede({ [TS]: { ...maq, lan_url: renewed }, [renewed]: maqLocal });

    await decidirRota(srv({ url: LAN, id: 'maq' }));

    expect(baseOf(srv())).toBe(renewed);
  });

  it('descobre o Tailscale pelo cadastro LAN e prova o candidato antes de autenticá-lo', async () => {
    env(null);
    const calls: { url: string; token: string | null }[] = [];
    vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url, token: new Headers(init?.headers).get('Authorization') });
      if (url === `${LAN}/api/alcance`) return new Response(JSON.stringify({
        enderecos: [{ tipo: 'tailscale', url: TS, estado: 'falhou' }],
      }));
      if (new URL(url).pathname === '/api/peers/prova') {
        const challenge = new URL(url).searchParams.get('desafio')!;
        return new Response(JSON.stringify({ identificador: 'maq', prova: createHmac('sha256', 't').update(`${challenge}|maq`).digest('hex') }));
      }
      if (url === `${LAN}/api/peers/identificador`) return new Response(JSON.stringify({ identificador: 'maq', lan_url: LAN }));
      throw new Error('unexpected_request');
    }));
    const saved = { ...srv({ url: LAN, id: 'maq' }), baseUrl: LAN };
    await decidirRota(saved);

    expect(await discoverRemoteRoute(saved)).toEqual({ url: TS, id: 'maq' });
    expect(calls.filter((call) => call.url.startsWith(TS)).every((call) => call.token === null)).toBe(true);
    expect(calls.find((call) => call.url === `${LAN}/api/alcance`)?.token).toBe('Bearer t');
    await discoverRemoteRoute(saved);
    expect(calls.filter((call) => call.url === `${LAN}/api/alcance`)).toHaveLength(1);
  });

  it('usa o Tailscale aprendido quando o cadastro principal era da LAN e ela caiu', async () => {
    env(null);
    const calls = rede({ [TS]: maqLocal });
    const saved = { ...srv({ url: LAN, id: 'maq' }), baseUrl: LAN, remote: { url: TS, id: 'maq' } };

    await decidirRota(saved);

    expect(baseOf(saved)).toBe(TS);
    const remoteCalls = calls.filter((call) => call.url.startsWith(TS));
    expect(new URL(remoteCalls[0].url).pathname).toBe('/api/peers/prova');
    expect(remoteCalls[0].auth).toBeNull();
    expect(remoteCalls.slice(1).every((call) => call.auth === 'Bearer t')).toBe(true);
  });

  it('primeira vez: aprende o endereço local pelo principal e já nasce nele', async () => {
    const lembrados = env(null);
    rede({ [TS]: maq, [LAN]: maqLocal });
    await decidirRota(srv());
    expect(baseOf(srv())).toBe(LAN);
    expect(lembrados.s1).toEqual({ url: LAN, id: 'maq' });
  });

  it('o token nunca vai pro endereço local antes da prova', async () => {
    env(null);
    const chamadas = rede({ [TS]: maq, [LAN]: maqLocal });
    await decidirRota(srv({ url: LAN, id: 'maq' }));
    expect(chamadas.filter((c) => c.url.startsWith(LAN)).every((c) => c.auth === null)).toBe(true);
  });

  it('outra máquina no mesmo IP, mesmo dizendo o nome certo, não prova o token: fica no principal', async () => {
    env(null);
    rede({ [TS]: maq, [LAN]: { identificador: 'maq', token: 'outro' } });
    await decidirRota(srv({ url: LAN, id: 'maq' }));
    expect(baseOf(srv())).toBe(TS);
  });

  it('fora da rede local cai no principal; depois da falha decide de novo', async () => {
    env(null);
    rede({ [TS]: maq });
    await decidirRota(srv({ url: LAN, id: 'maq' }));
    expect(baseOf(srv())).toBe(TS);
    rede({ [TS]: maq, [LAN]: maqLocal });
    esquecerRota('s1');
    await decidirRota(srv({ url: LAN, id: 'maq' }));
    expect(baseOf(srv())).toBe(LAN);
  });

  it('mesmo IP alcançado por VPN, mais lento que o Tailscale: fica no principal', async () => {
    env(null);
    rede({ [TS]: maq, [LAN]: { ...maq, atraso: 250 } });
    await decidirRota(srv({ url: LAN, id: 'maq' }));
    expect(baseOf(srv())).toBe(TS);
    esquecerRota('s1');
    _resetRotasForTests();
    await decidirRota(srv());
    expect(baseOf(srv())).toBe(TS);
  });

  it('página HTTPS nem tenta o http:// local (o navegador bloquearia)', async () => {
    env('https://pocket.exemplo');
    const chamadas = rede({ [TS]: maq, [LAN]: maqLocal });
    await decidirRota(srv({ url: LAN, id: 'maq' }));
    expect(baseOf(srv())).toBe(TS);
    expect(chamadas.some((c) => c.url.startsWith(LAN))).toBe(false);
  });

  it('HMAC confere com o do Node', () => {
    expect(hmacSha256Hex('k'.repeat(70), 'x|maq')).toBe(createHmac('sha256', 'k'.repeat(70)).update('x|maq').digest('hex'));
  });

  it('SHA-256 confere com o do Node', () => {
    expect(sha256Hex(new TextEncoder().encode('token-ção'))).toBe(createHash('sha256').update('token-ção').digest('hex'));
  });
});

describe('rota do Connect', () => {
  const CONNECT = 'https://pc1.acme.hangar.dev.br';
  const proofKey = createHash('sha256').update('device-token').digest('hex');
  const maqDevice = { identificador: 'maq', token: 'device-token', proofKey };
  const device = (over: Partial<Server> = {}): Server => ({
    ...srv({ url: '', id: 'maq' }), token: 'device-token', credential: 'device', deviceId: 'dev1',
    connect: { url: CONNECT, id: 'maq' }, ...over,
  });

  it('uses the hash of a device token as proof key', async () => {
    env(null);
    const chamadas = rede({ [CONNECT]: maqDevice });

    expect(await verifyServerIdentity(device(), { url: CONNECT, id: 'maq' })).toBe(true);

    expect(new URL(chamadas[0].url).searchParams.get('device')).toBe('dev1');
    expect(chamadas[0].auth).toBeNull();
    rede({ [CONNECT]: { ...maqDevice, proofKey: 'device-token' } });
    expect(await verifyServerIdentity(device(), { url: CONNECT, id: 'maq' })).toBe(false);
  });

  it('refuses a device proof relayed from the Connect address to the same LAN IP', async () => {
    env(null);
    rede({ [LAN]: { ...maqDevice, receivedHost: new URL(CONNECT).host } });
    expect(await verifyServerIdentity(device(), { url: LAN, id: 'maq' })).toBe(false);
    rede({ [LAN]: maqDevice });
    expect(await verifyServerIdentity(device(), { url: LAN, id: 'maq' })).toBe(true);
  });

  it('learns a connect route from /api/alcance', async () => {
    env(null);
    const remote = { url: 'https://outra.tail.ts.net', id: 'maq' };
    const saved = device({ connect: undefined, remote });
    const chamadas = rede({ [TS]: { ...maqDevice, connect: CONNECT }, [CONNECT]: maqDevice });

    expect(await discoverConnectRoute(saved)).toEqual({ url: CONNECT, id: 'maq' });

    expect(saved.remote).toEqual(remote);
    expect(chamadas.filter((c) => c.url.startsWith(CONNECT)).every((c) => c.auth === null)).toBe(true);
  });

  it('não aprende um endereço que não prova ser a mesma máquina', async () => {
    env(null);
    rede({ [TS]: { ...maqDevice, connect: CONNECT }, [CONNECT]: { ...maqDevice, proofKey: 'outra-chave' } });
    expect(await discoverConnectRoute(device({ connect: undefined }))).toBeNull();
  });

  it.each(['http://pc1.acme.hangar.dev.br', 'https://u:p@pc1.acme.hangar.dev.br', 'nao-e-url', 7])('ignora endereço do Connect fora do contrato: %s', async (connect) => {
    env(null);
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ enderecos: [], connect }))));
    expect(await readConnectUrl(srv())).toBeNull();
  });

  it('lê o alcance de novo depois de uma falha, sem esperar o cache', async () => {
    env(null);
    vi.stubGlobal('fetch', vi.fn(async () => { throw new TypeError('offline'); }));
    expect(await readConnectUrl(srv())).toBeNull();
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ enderecos: [], connect: CONNECT }))));
    expect(await readConnectUrl(srv())).toBe(CONNECT);
  });

  it('never tries connect with the owner password', async () => {
    env(null);
    const saved = { ...srv({ url: '', id: 'maq' }), connect: { url: CONNECT, id: 'maq' } };
    const chamadas = rede({ [TS]: { ...maq, token: 't', connect: CONNECT }, [CONNECT]: maqDevice });

    expect(await discoverConnectRoute(saved)).toBeNull();
    rede({ [CONNECT]: maqDevice });
    await decidirRota(saved);

    expect(baseOf(saved)).toBe(TS);
    expect(chamadas).toHaveLength(0);
    expect(vi.mocked(fetch).mock.calls.some(([url]) => String(url).startsWith(CONNECT))).toBe(false);
  });

  it('usa o Connect quando nenhuma outra rota responde, e só depois da prova', async () => {
    env(null);
    const chamadas = rede({ [CONNECT]: maqDevice });
    await decidirRota(device());

    expect(baseOf(device())).toBe(CONNECT);
    expect(chamadas.filter((c) => c.url.startsWith(CONNECT)).map((c) => [new URL(c.url).pathname, c.auth])).toEqual([['/api/peers/prova', null]]);
  });

  it('não entra pelo Connect que não prova a identidade', async () => {
    env(null);
    rede({ [CONNECT]: { ...maqDevice, proofKey: 'outra-chave' } });
    await decidirRota(device());
    expect(baseOf(device())).toBe(TS);
  });

  it('keeps Tailscale first when both routes answer', async () => {
    env(null);
    rede({ [TS]: { ...maqDevice, atraso: 50 }, [CONNECT]: maqDevice });
    await decidirRota(device());
    expect(baseOf(device())).toBe(TS);
  });

  it('a rede local provada continua vencendo com credencial de aparelho', async () => {
    env(null);
    rede({ [TS]: { ...maqDevice, atraso: 100, lan_url: LAN }, [LAN]: maqDevice, [CONNECT]: maqDevice });
    await decidirRota(device({ lan: { url: LAN, id: 'maq' } }));
    expect(baseOf(device({ lan: { url: LAN, id: 'maq' } }))).toBe(LAN);
  });

  describe('routeKind', () => {
    it('says connect when the Connect is the route in use', async () => {
      env(null);
      rede({ [CONNECT]: maqDevice });
      await decidirRota(device());
      expect(routeKind(device())).toBe('connect');
    });

    it('says local when the proven local address wins', async () => {
      env(null);
      const server = device({ lan: { url: LAN, id: 'maq' } });
      rede({ [TS]: { ...maqDevice, atraso: 100, lan_url: LAN }, [LAN]: maqDevice });
      await decidirRota(server);
      expect(routeKind(server)).toBe('local');
    });

    it('says tailscale when the learned Tailscale address answers for a saved non-Tailscale one', async () => {
      env(null);
      const server: Server = { id: 's1', label: 'maq', baseUrl: 'https://pc.example', token: 't', remote: { url: TS, id: 'maq' } };
      rede({ [TS]: maq });
      await decidirRota(server);
      expect(baseOf(server)).toBe(TS);
      expect(routeKind(server)).toBe('tailscale');
    });

    it('classifies the saved address by its host', () => {
      expect(routeKind({ ...srv(), baseUrl: TS })).toBe('tailscale');
      expect(routeKind({ ...srv(), baseUrl: 'https://pc.example' })).toBe('saved');
      expect(routeKind({ ...srv(), baseUrl: 'https://100.64.0.2:8443' })).toBe('tailscale');
      expect(routeKind({ ...srv(), baseUrl: 'https://100.128.0.1:8443' })).toBe('saved');
    });

    it('ignores a trailing slash when comparing addresses', () => {
      expect(routeKind({ ...srv({ url: 'https://pc.example/', id: 'maq' }), baseUrl: 'https://pc.example' })).toBe('local');
    });
  });
});
