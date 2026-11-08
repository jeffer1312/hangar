import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { _resetRotasForTests, configureApi, definirArmazem, type ConnectMachine, type Server } from '@hangar/core';

const vault = vi.hoisted(() => new Map<string, string>());
vi.mock('expo-secure-store', () => ({
  getItemAsync: async (key: string) => vault.get(key) ?? null,
  setItemAsync: async (key: string, value: string) => { vault.set(key, value); },
  deleteItemAsync: async (key: string) => { vault.delete(key); },
}));
vi.mock('expo-constants', () => ({ default: { deviceName: 'fixture-phone' } }));

import { useServers } from '../../stores/servers';
import { CONNECT_SERVICE_URL, useConnect } from './connectStore';
import * as m from '../../paraglide/messages';

const ACCOUNT_KEY = 'cp_connect_account_v1';
const MACHINE_URL = 'https://pc1.acme.hangar.dev.br';
const online: ConnectMachine = { machine_id: 'pc1', name: 'pc1', host: 'pc1.acme.hangar.dev.br', online: true };
const offline: ConnectMachine = { ...online, online: false };
const entry: Server = { id: 'pc1', label: 'pc1', baseUrl: MACHINE_URL, token: 'device-token', credential: 'device', deviceId: 'dev1' };

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const empty = () => new Response(null, { status: 204 });

type Call = { url: URL; method: string; body: Record<string, unknown> | undefined; auth: string | null };
type Overrides = { statuses?: unknown[]; machines?: ConnectMachine[]; rows?: unknown[]; route?: (call: Call) => Response | undefined };

function network({ statuses = [], machines = [online], rows = [], route }: Overrides = {}) {
  const calls: Call[] = [];
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const call: Call = { url: new URL(input), method: init?.method ?? 'GET', body: init?.body ? JSON.parse(String(init.body)) : undefined,
      auth: new Headers(init?.headers).get('Authorization') };
    calls.push(call);
    const { pathname, origin } = call.url;
    const custom = route?.(call);
    if (custom) return custom;
    if (origin === CONNECT_SERVICE_URL) {
      if (pathname === '/v1/machines') return json(machines);
      if (pathname.endsWith('/devices')) return json({ url: MACHINE_URL, ticket: '{"nonce":"n"}', signature: 'c2ln' });
      if (pathname === '/v1/sessions' && call.method === 'DELETE') return empty();
      if (pathname === '/v1/accounts/password') return empty();
      return json({ token: 'account-token' });
    }
    if (pathname === '/api/connect/redeem') return json({ device_id: 'dev1', status: 'pending' }, 202);
    if (pathname === '/api/connect/redeem/status') return json(statuses.shift() ?? { status: 'pending' });
    if (pathname === '/api/connect/devices') return json(rows);
    if (pathname.startsWith('/api/connect/devices/')) return json({ ok: true });
    return json({ identificador: 'pc1', lan_url: '' });
  }));
  return calls;
}

const settle = () => new Promise<void>((resolve) => setImmediate(resolve));
// Cada passo cede ao laço real: o pedido seguinte só é agendado depois que a resposta foi lida.
async function elapse(ms: number, step = 500) {
  for (let spent = 0; spent < ms; spent += step) await vi.advanceTimersByTimeAsync(step);
}
const connect = () => useConnect.getState();
const servers = () => useServers.getState().servers;
const signal = () => new AbortController().signal;

async function seed(list: Server[]) {
  vault.set('cp_servers_v1', JSON.stringify({ servers: list, activeId: list[0]?.id ?? null }));
  await useServers.getState().load();
}

beforeEach(async () => {
  vault.clear();
  _resetRotasForTests();
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized: vi.fn(), origin: null,
    createEventSource: () => { throw new Error('sem SSE'); } });
  await useServers.getState().load();
  useConnect.setState(useConnect.getInitialState(), true);
});
afterEach(() => {
  definirArmazem(null);
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('conta do Connect', () => {
  it('stores the account token only in the secure store', async () => {
    const stored = vi.fn();
    definirArmazem({ getItem: () => null, setItem: stored, removeItem: vi.fn() });
    network();

    await connect().signIn(' acme ', 'long-password');
    await settle();

    expect(JSON.parse(vault.get(ACCOUNT_KEY)!)).toEqual({ serviceUrl: CONNECT_SERVICE_URL, token: 'account-token', name: 'acme' });
    expect(JSON.stringify(stored.mock.calls)).not.toContain('account-token');
    expect(JSON.stringify(servers())).not.toContain('account-token');
    expect(vault.get(ACCOUNT_KEY)).not.toContain('long-password');

    useConnect.setState(useConnect.getInitialState(), true);
    await connect().load();
    expect(connect().account?.token).toBe('account-token');
  });

  it('não entra quando o cofre recusa guardar o token', async () => {
    network();
    const set = vi.spyOn(vault, 'set').mockImplementation(() => { throw new Error('fixture-vault'); });
    await expect(connect().signIn('acme', 'long-password')).rejects.toThrow(m.connect_error_vault());
    set.mockRestore();
    expect(connect().account).toBeNull();
  });

  it('diz o que houve com nome ou senha errados', async () => {
    network({ route: () => json({}, 401) });
    await expect(connect().signIn('acme', 'wrong-password')).rejects.toThrow(m.connect_error_login());
    network({ route: () => json({}, 409) });
    await expect(connect().signUp('acme', 'long-password')).rejects.toThrow(m.connect_error_name_taken());
  });

  it('volta ao login quando a sessão da conta acabou', async () => {
    network();
    await connect().signIn('acme', 'long-password');
    await settle();
    network({ route: () => json({}, 401) });

    await connect().refreshMachines();

    expect(connect().account).toBeNull();
    expect(connect().notice).toBe(m.connect_session_ended());
    expect(vault.has(ACCOUNT_KEY)).toBe(false);
  });

  it('signs out without removing added servers', async () => {
    await seed([entry]);
    network();
    await connect().signIn('acme', 'long-password');
    await settle();

    await connect().signOut();

    expect(connect().account).toBeNull();
    expect(vault.has(ACCOUNT_KEY)).toBe(false);
    expect(servers().map((s) => s.id)).toEqual(['pc1']);
  });

  it('ends every session and changes the password', async () => {
    const calls = network();
    await connect().signIn('acme', 'long-password');
    await settle();

    await connect().changePassword('long-password', 'new-long-password');
    expect(calls.find((call) => call.url.pathname === '/v1/accounts/password')).toMatchObject({
      method: 'POST', auth: 'Bearer account-token', body: { old_password: 'long-password', new_password: 'new-long-password' } });
    expect(connect().account?.token).toBe('account-token');

    await connect().endSessions();
    expect(calls.find((call) => call.url.pathname === '/v1/sessions' && call.method === 'DELETE')?.auth).toBe('Bearer account-token');
    expect(connect().account).toBeNull();
    expect(connect().notice).toBe(m.connect_sessions_ended());
    expect(vault.has(ACCOUNT_KEY)).toBe(false);
  });

  it('a lista mais nova vence uma resposta atrasada, e máquina igual mantém o mesmo objeto', async () => {
    network();
    await connect().signIn('acme', 'long-password');
    await settle();
    const kept = connect().machines![0];
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    let reads = 0;
    vi.stubGlobal('fetch', vi.fn(async (input: string) => {
      if (new URL(input).pathname !== '/v1/machines') return json({});
      if (++reads === 1) { await gate; return json([offline]); }
      return json([online]);
    }));

    const late = connect().refreshMachines();
    await connect().refreshMachines();
    expect(connect().loadingMachines).toBe(false);
    expect(connect().machines![0]).toBe(kept);
    release();
    await late;

    expect(connect().machines).toEqual([online]);
    expect(connect().loadingMachines).toBe(false);
  });

  it('quem pediu a lista só ouve o fim quando a leitura mais nova acaba', async () => {
    network();
    await connect().signIn('acme', 'long-password');
    await settle();
    const gates: (() => void)[] = [];
    vi.stubGlobal('fetch', vi.fn(async (input: string) => {
      if (new URL(input).pathname !== '/v1/machines') return json({});
      await new Promise<void>((resolve) => gates.push(resolve));
      return json([offline]);
    }));

    let pullDone = false;
    const pull = connect().refreshMachines().then(() => { pullDone = true; });
    await settle();
    void connect().refreshMachines();
    await settle();
    gates[0]();
    await settle();
    expect(pullDone).toBe(false);
    expect(connect().loadingMachines).toBe(true);
    gates[1]();
    await pull;
    expect(connect().loadingMachines).toBe(false);
  });

  it('troca de senha sem conta é erro, não sucesso', async () => {
    await expect(connect().changePassword('long-password', 'new-long-password')).rejects.toThrow(m.connect_session_ended());
  });

  it('sair da conta leva junto os pedidos e aparelhos que ela mostrava', async () => {
    network();
    await connect().signIn('acme', 'long-password');
    await settle();
    useConnect.setState({ entries: { pc1: { phase: 'failed', error: 'x' } }, devices: { pc1: { rows: [], error: '' } } });

    await connect().signOut();

    expect(connect().entries).toEqual({});
    expect(connect().devices).toEqual({});
  });

  it('senha atual errada não derruba a conta', async () => {
    network({ route: ({ url }) => (url.pathname === '/v1/accounts/password' ? json({}, 401) : undefined) });
    await connect().signIn('acme', 'long-password');
    await settle();

    await expect(connect().changePassword('wrong-password', 'new-long-password')).rejects.toThrow(m.connect_error_wrong_password());
    expect(connect().account).not.toBeNull();
  });
});

describe('máquinas da conta', () => {
  beforeEach(async () => {
    network();
    await connect().signIn('acme', 'long-password');
    await settle();
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
  });

  it('shows an offline machine without adding it', async () => {
    const calls = network({ machines: [offline] });
    await connect().refreshMachines();

    expect(await connect().enter(offline)).toBeNull();

    expect(connect().machines).toEqual([offline]);
    expect(calls.some((call) => call.url.pathname.endsWith('/devices'))).toBe(false);
    expect(servers()).toEqual([]);
  });

  it('shows the waiting state until the machine approves', async () => {
    network({ statuses: [{ status: 'pending' }, { status: 'approved', token: 'device-token' }] });

    const entering = connect().enter(online);
    await elapse(2500);
    expect(connect().entries.pc1).toEqual({ phase: 'waiting' });
    await elapse(2500);
    expect(connect().entries.pc1).toEqual({ phase: 'waiting' });
    expect(servers()).toEqual([]);
    await elapse(5000);
    await entering;

    expect(connect().entries.pc1).toBeUndefined();
    expect(servers()).toHaveLength(1);
  });

  it('adds a machine as a normal server', async () => {
    const calls = network({ statuses: [{ status: 'approved', token: 'device-token' }] });

    const entering = connect().enter(online);
    await elapse(6000);
    const saved = await entering;

    expect(saved).toMatchObject({ id: 'pc1', baseUrl: MACHINE_URL, token: 'device-token', credential: 'device', deviceId: 'dev1',
      connect: { url: MACHINE_URL, id: 'pc1' } });
    expect(servers()).toEqual([saved]);
    expect(useServers.getState().activeId).toBe('pc1');
    expect(calls.find((call) => call.url.pathname.endsWith('/devices'))?.body).toMatchObject({ device_name: 'fixture-phone' });
  });

  it('não troca a máquina em uso quando a aprovação chega', async () => {
    await seed([{ id: 'other', label: 'other', baseUrl: 'https://other.test', token: 'other-token' }]);
    network({ statuses: [{ status: 'approved', token: 'device-token' }] });

    const entering = connect().enter(online);
    await elapse(6000);
    await entering;

    expect(servers().map((s) => s.id)).toEqual(['other', 'pc1']);
    expect(useServers.getState().activeId).toBe('other');
  });

  it('renova a entrada de quem já tem a máquina por outro endereço, guardando a senha', async () => {
    await seed([{ id: 's1', label: 'apelido', baseUrl: 'https://pc1.tail.ts.net', token: 'owner-password', lan: { url: '', id: 'pc1' }, connectOff: true }]);
    network({ statuses: [{ status: 'approved', token: 'device-token' }] });

    const entering = connect().enter(online);
    await elapse(6000);
    await entering;

    expect(servers()).toHaveLength(1);
    expect(servers()[0]).toMatchObject({ id: 's1', label: 'apelido', baseUrl: 'https://pc1.tail.ts.net', token: 'device-token',
      credential: 'device', deviceId: 'dev1', ownerToken: 'owner-password', connect: { url: MACHINE_URL, id: 'pc1' } });
    expect(servers()[0].connectOff).toBeUndefined();
  });

  it.each([
    [{ status: 'denied' }, () => m.connect_error_denied()],
    [{ status: 'approved' }, () => m.connect_error_token_lost()],
  ])('diz o que houve e deixa pedir de novo: %j', async (answer, message) => {
    network({ statuses: [answer, { status: 'approved', token: 'device-token' }] });

    const refused = connect().enter(online);
    await elapse(6000);
    await refused;
    expect(connect().entries.pc1).toEqual({ phase: 'failed', error: message() });
    expect(servers()).toEqual([]);

    const again = connect().enter(online);
    await elapse(6000);
    await again;
    expect(connect().entries.pc1).toBeUndefined();
    expect(servers()).toHaveLength(1);
  });

  it('diz que o pedido venceu', async () => {
    network();
    const entering = connect().enter(online);
    await elapse(11 * 60_000, 3000);
    await entering;
    expect(connect().entries.pc1).toEqual({ phase: 'failed', error: m.connect_error_expired() });
  });

  it('não pede duas vezes enquanto espera', async () => {
    const calls = network({ statuses: [{ status: 'approved', token: 'device-token' }] });
    const entering = connect().enter(online);
    await elapse(500);

    expect(await connect().enter(online)).toBeNull();

    await elapse(6000);
    await entering;
    expect(calls.filter((call) => call.url.pathname === '/api/connect/redeem')).toHaveLength(1);
  });

  it('sem gerador seguro diz o motivo e não sai pedido', async () => {
    const calls = network();
    vi.stubGlobal('crypto', undefined);

    await connect().enter(online);

    expect(connect().entries.pc1).toEqual({ phase: 'failed', error: m.connect_error_secure_random() });
    expect(calls.some((call) => call.url.pathname.endsWith('/devices'))).toBe(false);
  });

  it('entra de novo numa entrada recusada, sem duplicar a máquina', async () => {
    await seed([{ ...entry, token: 'dead-token', deviceId: 'old', disabled: true }]);
    network({ statuses: [{ status: 'approved', token: 'device-token' }] });

    const entering = connect().reenter(servers()[0]);
    await elapse(6000);
    await entering;

    expect(servers()).toHaveLength(1);
    expect(servers()[0]).toMatchObject({ id: 'pc1', token: 'device-token', deviceId: 'dev1', disabled: false });
  });

  it('entra de novo numa entrada que voltou à senha e perdeu a rota do Connect', async () => {
    await seed([{ id: 's1', label: 'pc1', baseUrl: 'https://pc1.tail.ts.net', token: 'owner-password', connectOff: true, disabled: true,
      connectHost: 'pc1.acme.hangar.dev.br' }]);
    network({ statuses: [{ status: 'approved', token: 'device-token' }] });

    const entering = connect().reenter(servers()[0]);
    await elapse(6000);
    await entering;

    expect(connect().notice).toBe('');
    expect(servers().some((s) => s.token === 'device-token')).toBe(true);
  });

  it('a resposta de outra conta não marca a entrada da conta nova', async () => {
    network({ statuses: [{ status: 'denied' }], route: ({ url, method, body }) => (url.origin === CONNECT_SERVICE_URL
      && url.pathname === '/v1/sessions' && method === 'POST' && body?.name === 'other' ? json({ token: 'other-token' }) : undefined) });
    const entering = connect().enter(online);
    await elapse(500);
    await connect().signOut();
    await connect().signIn('other', 'long-password');

    await elapse(6000);
    await entering;

    expect(connect().account?.token).toBe('other-token');
    expect(connect().entries.pc1).toBeUndefined();
  });

  it('avisa quando a máquina recusada não é desta conta', async () => {
    await seed([{ ...entry, baseUrl: 'https://pc9.other.hangar.dev.br', disabled: true }]);
    await connect().reenter(servers()[0]);
    expect(connect().notice).toBe(m.connect_reenter_unknown({ name: 'pc1' }));
  });
});

describe('aparelhos da máquina', () => {
  it('lets an approved device approve a new one', async () => {
    await seed([entry]);
    let rows = [{ device_id: 'dev1', name: 'Hangar ios', status: 'approved', last_seen: null },
      { device_id: 'dev2', name: 'fixture-signed', status: 'pending', last_seen: null }];
    const calls = network({ route: ({ url, method }) => {
      if (url.pathname === '/api/connect/devices/dev2/approve') {
        rows = rows.map((row) => ({ ...row, status: 'approved' }));
        return json({ ok: true });
      }
      if (url.pathname === '/api/connect/devices' && method === 'GET') return json(rows);
      return undefined;
    } });

    await connect().refreshDevices();
    expect(connect().devices.pc1.rows?.find((row) => row.status === 'pending')?.name).toBe('fixture-signed');

    await connect().answerDevice(servers()[0], 'dev2', true, signal());

    const approve = calls.find((call) => call.url.pathname.endsWith('/approve'));
    expect(approve).toMatchObject({ method: 'POST', auth: 'Bearer device-token' });
    expect(approve?.url.origin).toBe(MACHINE_URL);
    expect(connect().devices.pc1.rows?.every((row) => row.status === 'approved')).toBe(true);
  });

  it('a resposta à aprovação relê a lista mesmo com uma leitura antiga em curso', async () => {
    await seed([entry]);
    let rows = [{ device_id: 'dev2', name: 'fixture-signed', status: 'pending', last_seen: null }];
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    let first = true;
    vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
      const { pathname } = new URL(input);
      if (pathname === '/api/connect/devices' && (init?.method ?? 'GET') === 'GET') {
        const snapshot = rows;
        if (first) { first = false; await gate; }
        return json(snapshot);
      }
      if (pathname.endsWith('/approve')) { rows = rows.map((row) => ({ ...row, status: 'approved' })); return json({ ok: true }); }
      return json({});
    }));

    const polling = connect().refreshDevices();
    const answering = connect().answerDevice(servers()[0], 'dev2', true, signal());
    await settle();
    release();
    await polling;
    await answering;

    expect(connect().devices.pc1.rows?.map((row) => row.status)).toEqual(['approved']);
  });

  it('recusa um pedido pendente com DELETE', async () => {
    await seed([entry]);
    const calls = network();
    await connect().answerDevice(servers()[0], 'dev2', false, signal());
    expect(calls.some((call) => call.method === 'DELETE' && call.url.pathname === '/api/connect/devices/dev2')).toBe(true);
  });

  it('não lê aparelhos de entrada desligada ou sem credencial de aparelho', async () => {
    await seed([{ ...entry, disabled: true }, { id: 's1', label: 's1', baseUrl: 'https://s1.test', token: 'owner-password' }]);
    const calls = network();
    await connect().refreshDevices();
    expect(calls).toEqual([]);
  });

  it('mantém a lista na tela quando a leitura falha', async () => {
    await seed([entry]);
    network({ rows: [{ device_id: 'dev1', name: 'fixture-phone', status: 'approved', last_seen: null }] });
    await connect().refreshDevices();
    network({ route: () => json({ detail: 'fixture-failure' }, 500) });

    await connect().refreshDevices();

    expect(connect().devices.pc1.rows).toHaveLength(1);
    expect(connect().devices.pc1.error).not.toBe('');
  });
});

describe('religar o Connect', () => {
  const reverted: Server = { id: 's1', label: 's1', baseUrl: 'https://pc1.tail.ts.net', token: 'owner-password', connectOff: true };
  const reach = (connectUrl: string | null) => ({ url, method }: Call) => {
    if (url.pathname === '/api/alcance') return json({ connect: connectUrl });
    if (url.pathname === '/api/connect/devices/self' && method === 'POST') return json({ device_id: 'dev2', token: 'device-token' });
    return undefined;
  };

  it('tira a marca e a promoção volta a rodar', async () => {
    await seed([reverted]);
    network({ route: reach(MACHINE_URL) });

    expect(await connect().turnOnConnect(servers()[0])).toBe(true);

    expect(servers()[0]).toMatchObject({ token: 'device-token', credential: 'device', deviceId: 'dev2', ownerToken: 'owner-password' });
    expect(servers()[0].connectOff).toBeUndefined();
  });

  it('sem Connect na máquina tira a marca e mantém a senha', async () => {
    await seed([reverted]);
    network({ route: reach(null) });

    expect(await connect().turnOnConnect(servers()[0])).toBe(false);

    expect(servers()[0].token).toBe('owner-password');
    expect(servers()[0].connectOff).toBeUndefined();
  });
});
