import { createHash } from 'node:crypto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import { addMachine, listMachines, signIn, signUp, upgradeToDevice, type AccountSession, type ConnectMachine } from './connectAccount';
import { _resetRotasForTests } from './rota';
import type { Server } from './servers';

const SERVICE = 'https://connect.hangar.dev.br';
const MACHINE_URL = 'https://pc1.acme.hangar.dev.br';
const session: AccountSession = { serviceUrl: SERVICE, token: 'account-token' };
const machine: ConnectMachine = { machine_id: 'm1', name: 'pc1', host: 'pc1.acme.hangar.dev.br', online: true };
const owner: Server = { id: 's1', label: 'pc1', baseUrl: 'https://pc1.tail.ts.net', token: 'owner-password', lan: { url: '', id: 'pc1' } };

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

type Call = { url: URL; method: string; body: Record<string, unknown> | undefined; auth: string | null };
function network(route: (url: URL) => Response) {
  const calls: Call[] = [];
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const url = new URL(input);
    calls.push({ url, method: init?.method ?? 'GET', body: init?.body ? JSON.parse(String(init.body)) : undefined,
      auth: new Headers(init?.headers).get('Authorization') });
    return route(url);
  }));
  return calls;
}

function enrollment(statuses: unknown[]) {
  return network((url) => {
    if (url.origin === SERVICE) return json({ url: MACHINE_URL, ticket: '{"nonce":"n"}', signature: 'c2ln' });
    if (url.pathname === '/api/connect/redeem') return json({ device_id: 'dev1', status: 'pending' }, 202);
    if (url.pathname === '/api/connect/redeem/status') return json(statuses.shift() ?? { status: 'pending' });
    return json({ identificador: 'pc1', lan_url: '' });
  });
}

// Cada passo cede ao laço real: o pedido seguinte só é agendado depois que a resposta foi lida.
async function elapse(ms: number, step = 500) {
  for (let spent = 0; spent < ms; spent += step) await vi.advanceTimersByTimeAsync(step);
}

const onUnauthorized = vi.fn();
const env = (secureRandom?: () => string | null) => configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized, origin: null,
  createEventSource: () => { throw new Error('sem SSE'); }, secureRandom });
beforeEach(() => {
  _resetRotasForTests();
  onUnauthorized.mockReset();
  env();
});
afterEach(() => {
  _resetApiEnvForTests();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('conta do Connect', () => {
  it('cria a conta e entra pelas rotas do serviço', async () => {
    const calls = network(() => json({ token: 'account-token' }, 201));
    expect(await signUp(SERVICE, 'acme', 'senha')).toEqual(session);
    expect(await signIn(`${SERVICE}/`, 'acme', 'senha')).toEqual({ ...session, serviceUrl: `${SERVICE}/` });
    expect(calls.map((call) => `${call.method} ${call.url.href}`)).toEqual([`POST ${SERVICE}/v1/accounts`, `POST ${SERVICE}/v1/sessions`]);
    expect(calls[0].body).toEqual({ name: 'acme', password: 'senha' });
  });

  it('entrega o status da recusa sem copiar o corpo da resposta', async () => {
    network(() => json({ detail: 'eco: senha' }, 401));
    const error: unknown = await signIn(SERVICE, 'acme', 'senha').catch((e: unknown) => e);
    expect(error).toMatchObject({ status: 401 });
    expect(String(error)).not.toContain('senha');
  });

  it('não manda a senha da conta a serviço em http, exceto loopback', async () => {
    const calls = network(() => json({ token: 'account-token' }));
    await expect(signIn('http://connect.hangar.dev.br', 'acme', 'senha')).rejects.toMatchObject({ code: 'connect_service_insecure' });
    expect(calls).toHaveLength(0);
    await signIn('http://127.0.0.1:9000', 'acme', 'senha');
    expect(calls).toHaveLength(1);
  });

  it('lista as máquinas com o token da conta', async () => {
    const calls = network(() => json([machine]));
    expect(await listMachines(session)).toEqual([machine]);
    expect(calls[0].auth).toBe('Bearer account-token');
  });
});

describe('addMachine', () => {
  beforeEach(() => { vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] }); });

  it('never sends the device secret to the service', async () => {
    const calls = enrollment([{ status: 'approved', token: 'device-token' }]);
    const adding = addMachine(session, machine, 'Pixel', () => {});
    await elapse(6000);
    await adding;

    const redeem = calls.find((call) => call.url.pathname === '/api/connect/redeem')!;
    const secret = String(redeem.body?.secret);
    expect(secret).toMatch(/^[a-f0-9]{64}$/);
    expect(redeem.url.origin).toBe(MACHINE_URL);
    const toService = calls.filter((call) => call.url.origin === SERVICE);
    expect(toService).toHaveLength(1);
    expect(toService[0].url.pathname).toBe('/v1/machines/m1/devices');
    expect(toService[0].auth).toBe('Bearer account-token');
    expect(toService[0].body).toEqual({ device_name: 'Pixel', verifier: createHash('sha256').update(secret).digest('hex') });
    expect(JSON.stringify(toService.map((call) => call.body))).not.toContain(secret);
  });

  it('waits for approval before adding a machine', async () => {
    const calls = enrollment([{ status: 'pending' }, { status: 'approved', token: 'device-token' }]);
    const onPending = vi.fn();
    let added: Server | undefined;
    const adding = addMachine(session, machine, 'Pixel', onPending).then((server) => { added = server; });

    await elapse(2500);
    expect(onPending).toHaveBeenCalledOnce();
    expect(added).toBeUndefined();
    await elapse(7000);
    await adding;

    expect(onPending).toHaveBeenCalledOnce();
    expect(added).toEqual({ id: 'm1', label: 'pc1', baseUrl: MACHINE_URL, token: 'device-token', credential: 'device', deviceId: 'dev1',
      lan: { id: 'pc1', url: '' }, connect: { url: MACHINE_URL, id: 'pc1' } });
    const polls = calls.filter((call) => call.url.pathname === '/api/connect/redeem/status');
    expect(polls[0].body).toMatchObject({ device_id: 'dev1' });
    expect(polls).toHaveLength(2);
  });

  it('para quando a máquina recusa', async () => {
    enrollment([{ status: 'denied' }]);
    const outcome = addMachine(session, machine, 'Pixel', () => {}).catch((error: unknown) => error);
    await elapse(6000);
    expect(await outcome).toMatchObject({ code: 'connect_device_denied' });
  });

  it('para no fim do prazo da aprovação', async () => {
    const calls = enrollment([]);
    const outcome = addMachine(session, machine, 'Pixel', () => {}).catch((error: unknown) => error);
    await elapse(11 * 60_000, 3000);
    expect(await outcome).toMatchObject({ code: 'connect_approval_expired' });
    const polls = calls.filter((call) => call.url.pathname === '/api/connect/redeem/status').length;
    await elapse(30_000, 3000);
    expect(calls.filter((call) => call.url.pathname === '/api/connect/redeem/status')).toHaveLength(polls);
  });

  it('segue esperando quando a consulta cai no meio', async () => {
    let polls = 0;
    network((url) => {
      if (url.origin === SERVICE) return json({ url: MACHINE_URL, ticket: 't', signature: 's' });
      if (url.pathname === '/api/connect/redeem') return json({ device_id: 'dev1', status: 'pending' }, 202);
      if (url.pathname === '/api/connect/redeem/status') {
        if (++polls === 1) throw new TypeError('offline');
        return polls === 2 ? json({}, 503) : json({ status: 'approved', token: 'device-token' });
      }
      return json({ identificador: 'pc1', lan_url: '' });
    });
    const adding = addMachine(session, machine, 'Pixel', () => {});
    await elapse(12_000);
    expect((await adding).token).toBe('device-token');
  });

  it('o token entregue não se perde quando a identidade não responde', async () => {
    network((url) => {
      if (url.origin === SERVICE) return json({ url: MACHINE_URL, ticket: 't', signature: 's' });
      if (url.pathname === '/api/connect/redeem') return json({ device_id: 'dev1', status: 'pending' }, 202);
      if (url.pathname === '/api/connect/redeem/status') return json({ status: 'approved', token: 'device-token' });
      throw new TypeError('offline');
    });
    const adding = addMachine(session, machine, 'Pixel', () => {});
    await elapse(6000);
    expect(await adding).toEqual({ id: 'm1', label: 'pc1', baseUrl: MACHINE_URL, token: 'device-token', credential: 'device', deviceId: 'dev1' });
  });

  it.each(['http://pc1.acme.hangar.dev.br', 'https://outra.acme.hangar.dev.br'])('recusa %s antes de mandar o segredo', async (url) => {
    const calls = network(() => json({ url, ticket: 't', signature: 's' }));
    await expect(addMachine(session, machine, 'Pixel', () => {})).rejects.toMatchObject({ code: 'connect_response_invalid' });
    expect(calls).toHaveLength(1);
  });

  it('sem crypto usa o gerador do ApiEnv', async () => {
    vi.stubGlobal('crypto', undefined);
    env(() => 'f'.repeat(64));
    const calls = enrollment([{ status: 'approved', token: 'device-token' }]);
    const adding = addMachine(session, machine, 'Pixel', () => {});
    await elapse(6000);
    await adding;
    expect(calls.find((call) => call.url.pathname === '/api/connect/redeem')?.body?.secret).toBe('f'.repeat(64));
  });

  it.each([{ name: 'sem gerador', generator: undefined }, { name: 'gerador vazio', generator: () => null }])('sem crypto e $name falha antes de qualquer pedido', async ({ generator }) => {
    vi.stubGlobal('crypto', undefined);
    env(generator);
    const calls = enrollment([]);
    await expect(addMachine(session, machine, 'Pixel', () => {})).rejects.toMatchObject({ code: 'connect_secure_random_unavailable' });
    expect(calls).toHaveLength(0);
  });
});

describe('upgradeToDevice', () => {
  const machineWith = (connect: unknown) => network((url) => {
    if (url.pathname === '/api/alcance') return json({ enderecos: [], connect });
    return json({ device_id: 'dev1', token: 'device-token' });
  });
  const enrolls = (calls: Call[]) => calls.filter((call) => call.url.pathname === '/api/connect/devices/self');

  it('upgrades to a device credential while another route works', async () => {
    const calls = machineWith(MACHINE_URL);

    expect(await upgradeToDevice(owner)).toEqual({ ...owner, token: 'device-token', ownerToken: 'owner-password', credential: 'device', deviceId: 'dev1' });

    expect(enrolls(calls)).toHaveLength(1);
    expect(enrolls(calls)[0]).toMatchObject({ method: 'POST', auth: 'Bearer owner-password' });
    expect(enrolls(calls)[0].url.origin).toBe(owner.baseUrl);
    expect(calls.some((call) => call.url.origin === MACHINE_URL)).toBe(false);
  });

  it.each([null, undefined, '', 'http://pc1.acme.hangar.dev.br'])('keeps the owner password when the machine has no Connect (%s)', async (connect) => {
    const calls = machineWith(connect);
    expect(await upgradeToDevice(owner)).toBe(owner);
    expect(enrolls(calls)).toHaveLength(0);
  });

  it.each([401, 403, 409, 503])('mantém a senha e não remove a máquina quando o registro volta %i', async (status) => {
    network((url) => (url.pathname === '/api/alcance' ? json({ enderecos: [], connect: MACHINE_URL }) : json({}, status)));
    expect(await upgradeToDevice(owner)).toBe(owner);
    expect(onUnauthorized).not.toHaveBeenCalled();
  });

  it('mantém a senha quando /api/alcance falha', async () => {
    network(() => json({}, 401));
    expect(await upgradeToDevice(owner)).toBe(owner);
    expect(onUnauthorized).not.toHaveBeenCalled();
  });

  it('pedidos juntos registram um aparelho só', async () => {
    const calls = machineWith(MACHINE_URL);
    const [first, second] = await Promise.all([upgradeToDevice(owner), upgradeToDevice(owner)]);
    expect(first).toBe(second);
    expect(enrolls(calls)).toHaveLength(1);
  });

  it.each([
    { credential: 'device' as const, deviceId: 'dev1' }, { connectOff: true }, { invite: true }, { ephemeral: true }, { removed: true },
  ])('não toca a rede em %j', async (flags) => {
    const calls = machineWith(MACHINE_URL);
    const server = { ...owner, ...flags };
    expect(await upgradeToDevice(server)).toBe(server);
    expect(calls).toHaveLength(0);
  });
});
