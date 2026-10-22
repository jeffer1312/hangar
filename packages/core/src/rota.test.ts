import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { createHmac } from 'node:crypto';
import { apiEnv, configureApi, _resetApiEnvForTests } from './apiEnv';
import { fetchCostsForServer, openEventStream, openEventStreamForServer, openSessionsStream, probeServerResponse } from './api';
import { hmacSha256Hex } from './hmac';
import { baseOf, decidirRota, discoverRemoteRoute, esquecerRota, verifyServerIdentity, _resetRotasForTests } from './rota';
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
type Maquina = { identificador: string; token: string; lan_url?: string; atraso?: number } | null;
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
      const prova = createHmac('sha256', m.token).update(`${d}|${m.identificador}`).digest('hex');
      return new Response(JSON.stringify({ identificador: m.identificador, prova }));
    }
    if (auth !== `Bearer ${m.token}`) return new Response('{}', { status: 401 });
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
});
