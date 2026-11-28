import { createHash } from 'node:crypto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import { addMachine, approveAccountDevice, changeAccountPassword, deviceStatus, endAccountSessions, listAccountDevices, listMachines, lookupName,
  registerDevice, removeAccountMachine, revokeAccountDevice, signIn, signInAnywhere, signUp, upgradeToDevice, type AccountSession, type ConnectMachine } from './connectAccount';
import { installCode } from './installCode';
import { _resetRotasForTests } from './rota';
import type { Server } from './servers';
import type { SealedBlob, SyncCrypto } from './syncLogin';

const SERVICE = 'https://connect.hangar.dev.br';
const MACHINE_URL = 'https://pc1.acme.hangar.dev.br';
const session: AccountSession = { serviceUrl: SERVICE, token: 'account-token' };
const machine: ConnectMachine = { machine_id: 'm1', name: 'pc1', host: 'pc1.acme.hangar.dev.br', online: true };
const owner: Server = { id: 's1', label: 'pc1', baseUrl: 'https://pc1.tail.ts.net', token: 'owner-password', lan: { url: '', id: 'pc1' } };

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

type Call = { url: URL; method: string; body: Record<string, unknown> | undefined; auth: string | null; headers: Headers };
function network(route: (url: URL) => Response) {
  const calls: Call[] = [];
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const url = new URL(input);
    calls.push({ url, method: init?.method ?? 'GET', body: init?.body ? JSON.parse(String(init.body)) : undefined,
      auth: new Headers(init?.headers).get('Authorization'), headers: new Headers(init?.headers) });
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

  it.each([
    { machine_id: 7, name: 'pc1', host: 'pc1.acme.hangar.dev.br', online: true },
    { name: 'pc1', host: 'pc1.acme.hangar.dev.br', online: true },
    { machine_id: 'm1', name: 'pc1', host: 'pc1.acme.hangar.dev.br', online: 'yes' },
    null,
  ])('recusa item da lista fora do contrato: %j', async (item) => {
    network(() => json([machine, item]));
    await expect(listMachines(session)).rejects.toMatchObject({ code: 'connect_response_invalid' });
  });

  it('encerra as sessões e troca a senha, que respondem sem corpo', async () => {
    const calls = network(() => new Response(null, { status: 204 }));
    await endAccountSessions(session);
    await changeAccountPassword(session, 'senha-velha', 'senha-nova');
    expect(calls.map((call) => `${call.method} ${call.url.pathname} ${call.auth}`)).toEqual([
      'DELETE /v1/sessions Bearer account-token', 'POST /v1/accounts/password Bearer account-token']);
    expect(calls[1].body).toEqual({ old_password: 'senha-velha', new_password: 'senha-nova' });
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

  it('para num status que não conhece em vez de esperar o prazo', async () => {
    enrollment([{ status: 'revoked' }]);
    const outcome = addMachine(session, machine, 'Pixel', () => {}).catch((error: unknown) => error);
    await elapse(6000);
    expect(await outcome).toMatchObject({ code: 'connect_response_invalid' });
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

describe('aparelho da conta', () => {
  const released = { ...session, device: { id: 'ad1', secret: 'ad-secret' } };
  const credential = (call: Call) => [call.auth, call.headers.get('X-Hangar-Device'), call.headers.get('X-Hangar-Device-Secret')];
  const row = { device_id: 'ad2', name: 'Pixel', status: 'pending', created_at: 1_794_000_000.5, approved_at: null, revoked_at: null };

  it('registra o aparelho com a sessão da conta', async () => {
    const calls = network(() => json({ device_id: 'ad1', device_secret: 'ad-secret', status: 'pending' }, 201));
    expect(await registerDevice(session, 'iPhone', 'install-1')).toEqual({ id: 'ad1', secret: 'ad-secret' });
    expect(calls.map((call) => `${call.method} ${call.url.href} ${call.auth}`)).toEqual([`POST ${SERVICE}/v1/devices Bearer account-token`]);
    expect(calls[0].body).toEqual({ name: 'iPhone', install_id: 'install-1' });
    await registerDevice(session, 'iPhone');
    expect(calls[1].body).toEqual({ name: 'iPhone' });
  });

  it.each([{ device_id: 'ad1', status: 'pending' }, { device_id: '', device_secret: 's', status: 'pending' }, null])(
    'recusa registro fora do contrato: %j', async (body) => {
      network(() => json(body, 201));
      await expect(registerDevice(session, 'iPhone')).rejects.toMatchObject({ code: 'connect_response_invalid' });
    });

  it('consulta o próprio status com a credencial do aparelho', async () => {
    const calls = network(() => json({ status: 'approved' }));
    expect(await deviceStatus(released)).toBe('approved');
    expect(calls[0].url.pathname).toBe('/v1/devices/ad1');
    expect(credential(calls[0])).toEqual(['Bearer account-token', 'ad1', 'ad-secret']);
    network(() => json({ status: 'denied' }));
    await expect(deviceStatus(released)).rejects.toMatchObject({ code: 'connect_response_invalid' });
  });

  it('lista, libera e revoga aparelhos da conta com a credencial do aparelho', async () => {
    const calls = network((url) => (url.pathname === '/v1/devices' ? json([row]) : url.pathname.endsWith('/approve')
      ? json({ status: 'approved' }) : new Response(null, { status: 204 })));
    expect(await listAccountDevices(released)).toEqual([row]);
    await approveAccountDevice(released, 'ad2');
    await revokeAccountDevice(released, 'ad 2');
    expect(calls.map((call) => `${call.method} ${call.url.pathname}`)).toEqual(['GET /v1/devices', 'POST /v1/devices/ad2/approve', 'DELETE /v1/devices/ad%202']);
    expect(calls.map(credential)).toEqual(Array(3).fill(['Bearer account-token', 'ad1', 'ad-secret']));
  });

  it('remove o computador da conta com a sessão e a credencial do aparelho', async () => {
    const calls = network(() => new Response(null, { status: 204 }));
    await removeAccountMachine(released, 'pc 2');
    expect(calls.map((call) => `${call.method} ${call.url.href}`)).toEqual([`DELETE ${SERVICE}/v1/machines/pc%202`]);
    expect(calls.map(credential)).toEqual([['Bearer account-token', 'ad1', 'ad-secret']]);
  });

  it('recusa da remoção sai com o status e sem o texto do serviço', async () => {
    network(() => json({ error: 'eco: senha' }, 403));
    const error: unknown = await removeAccountMachine(released, 'outra').catch((e: unknown) => e);
    expect(error).toMatchObject({ code: 'connect_request_failed', status: 403 });
    expect(String(error)).not.toContain('senha');
  });

  it('404 do serviço conta como já removido', async () => {
    network(() => json({ error: 'não encontrado' }, 404));
    await expect(removeAccountMachine(released, 'outra')).resolves.toBeUndefined();
  });

  it('404 que não veio do serviço não conta como removido', async () => {
    for (const answer of [new Response('<html>not found</html>', { status: 404 }), json({ error: 5 }, 404), json(['error'], 404)]) {
      network(() => answer);
      const error: unknown = await removeAccountMachine(released, 'outra').catch((e: unknown) => e);
      expect(error).toMatchObject({ code: 'connect_machine_remove_unavailable', status: 404 });
    }
  });

  it('o código de conferência do pendente vem pronto do serviço e é o mesmo que o celular calcula', async () => {
    const code = installCode('install-1');
    network(() => json([{ ...row, install_code: code }, { ...row, device_id: 'ad3', install_code: null }]));
    const [withCode, without] = await listAccountDevices(released);
    expect(withCode.install_code).toBe(code);
    expect(without.install_code).toBeNull();
  });

  it.each([{ ...row, status: 'denied' }, { ...row, created_at: 'ontem' }, { ...row, name: 7 }, { ...row, approved_at: 'x' },
    { ...row, install_code: 7 }])(
    'recusa linha da lista fora do contrato: %j', async (item) => {
      network(() => json([row, item]));
      await expect(listAccountDevices(released)).rejects.toMatchObject({ code: 'connect_response_invalid' });
    });

  it.each([['device_pending', 'connect_device_pending'], ['device_revoked', 'connect_device_revoked'], ['device_required', 'connect_device_required'],
    ['eco: senha', 'connect_request_failed'], [undefined, 'connect_request_failed']])('o motivo %s do 403 vira o código %s, sem o texto', async (reason, code) => {
    network(() => json({ error: 'eco: senha', reason }, 403));
    const error: unknown = await addMachine(released, machine, 'Pixel', () => {}).catch((e: unknown) => e);
    expect(error).toMatchObject({ code, status: 403 });
    expect(String(error)).not.toContain('senha');
  });

  it('o pedido de bilhete leva a credencial do aparelho, e o bilhete pré-aprovado entra sem esperar', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
    const calls = enrollment([{ status: 'approved', token: 'device-token' }]);
    const onPending = vi.fn();
    const added = await addMachine(released, machine, 'Pixel', onPending);
    expect(added.token).toBe('device-token');
    expect(onPending).not.toHaveBeenCalled();
    expect(credential(calls.find((call) => call.url.origin === SERVICE)!)).toEqual(['Bearer account-token', 'ad1', 'ad-secret']);
    expect(calls.filter((call) => call.url.origin === MACHINE_URL).some((call) => call.headers.has('X-Hangar-Device-Secret'))).toBe(false);
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

  it('manda o nome do aparelho e a instalação, para a máquina reaproveitar a entrada dela', async () => {
    configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized, origin: null,
      createEventSource: () => { throw new Error('sem SSE'); },
      deviceIdentity: () => ({ kind: 'android', id: 'install-1' }), deviceName: () => 'Pixel do Jefferson' });
    const calls = machineWith(MACHINE_URL);
    await upgradeToDevice(owner);
    expect(enrolls(calls)[0].body).toEqual({ device_name: 'Pixel do Jefferson', install_id: 'install-1' });
  });

  it('sem nome e sem instalação, fica o nome genérico', async () => {
    const calls = machineWith(MACHINE_URL);
    await upgradeToDevice(owner);
    expect(enrolls(calls)[0].body).toEqual({ device_name: 'Hangar app' });
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

describe('lookupName', () => {
  it.each([
    [{ kind: 'account' }, { kind: 'account' }],
    [{ kind: 'none' }, { kind: 'none' }],
    [{ kind: 'guest', hosts: ['casa.ana.hangar.dev.br'] }, { kind: 'guest', hosts: ['casa.ana.hangar.dev.br'] }],
  ])('responde %j', async (answer, expected) => {
    network(() => json(answer));
    expect(await lookupName('ana', SERVICE)).toEqual(expected);
  });

  it('manda só o nome, aparado e em minúsculas ASCII', async () => {
    const calls = network(() => json({ kind: 'none' }));
    await lookupName('  Lhais@Casa ', SERVICE);
    expect(calls.map((call) => `${call.method} ${call.url.href}`)).toEqual([`POST ${SERVICE}/v1/lookup`]);
    expect(calls[0].body).toEqual({ name: 'lhais@casa' });
  });

  it('serviço sem a rota (404) é login de conta', async () => {
    network(() => json({}, 404));
    expect(await lookupName('ana', SERVICE)).toEqual({ kind: 'account' });
  });

  it('429 tem código próprio', async () => {
    network(() => json({}, 429));
    await expect(lookupName('ana', SERVICE)).rejects.toMatchObject({ code: 'connect_too_many_attempts' });
  });

  it.each([
    { kind: 'guest', hosts: [] },
    { kind: 'guest', hosts: ['https://casa.ana.hangar.dev.br'] },
    { kind: 'guest', hosts: ['casa.ana.hangar.dev.br/caminho'] },
    { kind: 'guest' },
    { kind: 'outro' },
  ])('recusa resposta fora do contrato: %j', async (answer) => {
    network(() => json(answer));
    await expect(lookupName('ana', SERVICE)).rejects.toMatchObject({ code: 'connect_response_invalid' });
  });

  it('pedido sem resposta é rede', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => { throw new TypeError('Network request failed'); }));
    await expect(lookupName('ana', SERVICE)).rejects.toMatchObject({ code: 'network_failed' });
  });
});

describe('signInAnywhere', () => {
  const PASSWORD = 'senha-da-lhais';
  const SALT = 'c2Fs';
  const ITERATIONS = 600_000;
  // Cifra falsa na fronteira: o par real (PBKDF2 + HKDF + AES-GCM) é de quem chama e tem teste próprio.
  const syncCrypto: SyncCrypto = {
    deriveKeys: async (password, salt, iterations) => ({ authHash: `${password}|${salt}|${iterations}`, encKey: new TextEncoder().encode(password) }),
    decryptJson: <T,>(key: Uint8Array, blob: SealedBlob): T => {
      if (new TextDecoder().decode(key) !== blob.iv) throw new Error('auth tag');
      return JSON.parse(blob.data) as T;
    },
  };
  const seal = (password: string, list: unknown): SealedBlob => ({ iv: password, data: JSON.stringify(list) });
  const VAULT = [
    { id: 'a', label: 'Casa', baseUrl: '', token: 'ta' },
    { id: 'b', label: 'PC', baseUrl: 'https://pc.ana.hangar.dev.br', token: 'tb' },
    { id: 'c', label: 'Desligada', baseUrl: 'endereço torto', token: 'tc', disabled: true },
  ];
  const deps = { crypto: syncCrypto, serviceUrl: SERVICE };

  type Machine = { password?: string; down?: boolean; limited?: boolean; blob?: SealedBlob | null };
  type GuestCall = Call & { init?: RequestInit };
  function machines(hosts: Record<string, Machine>, lookup: unknown = { kind: 'guest', hosts: Object.keys(hosts) }) {
    const calls: GuestCall[] = [];
    vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
      const url = new URL(input);
      const body = init?.body ? JSON.parse(String(init.body)) as Record<string, unknown> : undefined;
      calls.push({ url, method: init?.method ?? 'GET', body, auth: new Headers(init?.headers).get('Authorization'), headers: new Headers(init?.headers), init });
      if (url.origin === SERVICE) return url.pathname === '/v1/lookup' ? json(lookup) : json({}, 401);
      const host = hosts[url.hostname];
      if (!host || host.down) throw new TypeError('Network request failed');
      if (url.pathname === '/api/sync/prelogin') return json({ salt: SALT, iterations: ITERATIONS });
      if (url.pathname === '/api/sync/login') {
        if (host.limited) return json({}, 429);
        return body?.auth_hash === `${host.password}|${SALT}|${ITERATIONS}` ? json({ ok: true }) : json({}, 401);
      }
      if (url.pathname === '/api/sync/logout') return json({ ok: true });
      return json({ enc_blob: host.blob === undefined ? seal(host.password!, VAULT) : host.blob, rev: 1 });
    }));
    return calls;
  }
  const route = (call: GuestCall) => `${call.method} ${call.url.hostname}${call.url.pathname}`;

  it('conta entra pelo login da conta', async () => {
    const calls = network((url) => (url.pathname === '/v1/lookup' ? json({ kind: 'account' }) : json({ token: 'account-token' }, 201)));
    expect(await signInAnywhere('ACME', 'senha', deps)).toEqual({ kind: 'account', account: session });
    expect(calls.map((call) => call.url.pathname)).toEqual(['/v1/lookup', '/v1/sessions']);
    expect(calls[1].body).toEqual({ name: 'acme', password: 'senha' });
  });

  it('senha da conta recusada vira "não confere"', async () => {
    network((url) => (url.pathname === '/v1/lookup' ? json({ kind: 'account' }) : json({}, 401)));
    await expect(signInAnywhere('acme', 'errada', deps)).rejects.toMatchObject({ code: 'credentials_mismatch' });
  });

  it('serviço antigo, sem lookup, segue para o login da conta', async () => {
    const calls = network((url) => (url.pathname === '/v1/lookup' ? json({}, 404) : json({ token: 'account-token' })));
    expect(await signInAnywhere('acme', 'senha', deps)).toMatchObject({ kind: 'account' });
    expect(calls.map((call) => call.url.pathname)).toEqual(['/v1/lookup', '/v1/sessions']);
  });

  it('nome desconhecido não confere, e a senha não sai do aparelho', async () => {
    const calls = machines({}, { kind: 'none' });
    await expect(signInAnywhere('ninguem@casa', PASSWORD, deps)).rejects.toMatchObject({ code: 'credentials_mismatch' });
    expect(calls.map(route)).toEqual(['POST connect.hangar.dev.br/v1/lookup']);
    expect(JSON.stringify(calls.map((call) => call.body))).not.toContain(PASSWORD);
  });

  it('convidada entra pela máquina que guarda o cofre e traz as máquinas ligadas', async () => {
    const calls = machines({ 'casa.ana.hangar.dev.br': { password: PASSWORD } });
    const result = await signInAnywhere('Lhais@Casa', PASSWORD, deps);
    expect(result).toMatchObject({ kind: 'guest', host: 'casa.ana.hangar.dev.br' });
    const servers = result.kind === 'guest' ? result.servers : [];
    // Endereço vazio no cofre foi gravado de dentro da própria máquina.
    expect(servers.map((s) => [s.id, s.baseUrl, s.token])).toEqual([
      ['a', 'https://casa.ana.hangar.dev.br', 'ta'], ['b', 'https://pc.ana.hangar.dev.br', 'tb']]);
    expect(calls.map(route)).toEqual([
      'POST connect.hangar.dev.br/v1/lookup', 'GET casa.ana.hangar.dev.br/api/sync/prelogin', 'POST casa.ana.hangar.dev.br/api/sync/login',
      'GET casa.ana.hangar.dev.br/api/sync/vault', 'POST casa.ana.hangar.dev.br/api/sync/logout']);
    expect(calls[1].url.searchParams.get('user')).toBe('lhais@casa');
    expect(calls[2].body).toEqual({ user: 'lhais@casa', auth_hash: `${PASSWORD}|${SALT}|${ITERATIONS}` });
    expect(calls.slice(1).every((call) => call.init?.credentials === 'include' && call.init.redirect === 'error')).toBe(true);
    // Ao serviço só vai o nome.
    expect(calls.filter((call) => call.url.origin === SERVICE).map((call) => call.body)).toEqual([{ name: 'lhais@casa' }]);
  });

  it('tenta uma máquina por vez e para na primeira que aceita', async () => {
    const calls = machines({
      'casa.ana.hangar.dev.br': { down: true },
      'pc.ana.hangar.dev.br': { password: PASSWORD },
      'velho.ana.hangar.dev.br': { password: PASSWORD },
    });
    expect(await signInAnywhere('lhais@casa', PASSWORD, deps)).toMatchObject({ kind: 'guest', host: 'pc.ana.hangar.dev.br' });
    expect(calls.some((call) => call.url.hostname === 'velho.ana.hangar.dev.br')).toBe(false);
  });

  it('senha recusada numa máquina passa para a seguinte', async () => {
    machines({ 'casa.ana.hangar.dev.br': { password: 'outra' }, 'pc.ana.hangar.dev.br': { password: PASSWORD } });
    expect(await signInAnywhere('lhais@casa', PASSWORD, deps)).toMatchObject({ kind: 'guest', host: 'pc.ana.hangar.dev.br' });
  });

  it('todas recusando a senha não confere', async () => {
    machines({ 'casa.ana.hangar.dev.br': { password: 'outra' }, 'pc.ana.hangar.dev.br': { password: 'outra' } });
    await expect(signInAnywhere('lhais@casa', PASSWORD, deps)).rejects.toMatchObject({ code: 'credentials_mismatch' });
  });

  it('máquina fora do ar é rede, não senha errada', async () => {
    machines({ 'casa.ana.hangar.dev.br': { password: 'outra' }, 'pc.ana.hangar.dev.br': { down: true } });
    await expect(signInAnywhere('lhais@casa', PASSWORD, deps)).rejects.toMatchObject({ code: 'network_failed' });
  });

  it('limite de tentativas tem código próprio', async () => {
    machines({ 'casa.ana.hangar.dev.br': { limited: true } });
    await expect(signInAnywhere('lhais@casa', PASSWORD, deps)).rejects.toMatchObject({ code: 'sync_too_many_attempts' });
  });

  it('cofre ilegível não vira lista vazia, e a sessão do cofre é encerrada', async () => {
    const calls = machines({ 'casa.ana.hangar.dev.br': { password: PASSWORD, blob: seal('outra', VAULT) } });
    await expect(signInAnywhere('lhais@casa', PASSWORD, deps)).rejects.toMatchObject({ code: 'sync_vault_unreadable' });
    expect(calls.map(route)).toContain('POST casa.ana.hangar.dev.br/api/sync/logout');
  });

  it.each([
    ['casa.outro.com'],
    ['hangar.dev.br'],
    ['casa.hangar.dev.br.outro.com'],
  ])('máquina fora de .hangar.dev.br é resposta inválida, e nada vai a ela: %s', async (host) => {
    const calls = machines({ [host]: { password: PASSWORD } }, { kind: 'guest', hosts: ['casa.ana.hangar.dev.br', host] });
    await expect(signInAnywhere('lhais@casa', PASSWORD, deps)).rejects.toMatchObject({ code: 'connect_response_invalid' });
    expect(calls.map(route)).toEqual(['POST connect.hangar.dev.br/v1/lookup']);
  });

  it('cofre vazio entra sem máquinas', async () => {
    machines({ 'casa.ana.hangar.dev.br': { password: PASSWORD, blob: null } });
    expect(await signInAnywhere('lhais@casa', PASSWORD, deps)).toEqual({ kind: 'guest', host: 'casa.ana.hangar.dev.br', servers: [] });
  });
});
