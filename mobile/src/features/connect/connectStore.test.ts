import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { _resetRotasForTests, configureApi, configureDiag, definirArmazem, type ConnectMachine, type Server } from '@hangar/core';

const vault = vi.hoisted(() => new Map<string, string>());
// Chave aqui: o cofre falha ao ler ou gravar só ela.
const broken = vi.hoisted(() => ({ read: new Set<string>(), write: new Set<string>(), hang: new Set<string>(), hangWrite: new Set<string>() }));
vi.mock('expo-secure-store', () => ({
  getItemAsync: async (key: string) => {
    if (broken.hang.has(key)) return new Promise<never>(() => {});
    if (broken.read.has(key)) throw new Error('cofre');
    return vault.get(key) ?? null;
  },
  setItemAsync: async (key: string, value: string) => {
    if (broken.hangWrite.has(key)) return new Promise<never>(() => {});
    if (broken.write.has(key)) throw new Error('cofre');
    vault.set(key, value);
  },
  deleteItemAsync: async (key: string) => { vault.delete(key); },
}));
vi.mock('expo-constants', () => ({ default: { deviceName: 'fixture-phone' } }));

import { useServers } from '../../stores/servers';
import { CONNECT_SERVICE_URL, useConnect } from './connectStore';
import * as m from '../../paraglide/messages';
import codes from '../../../../packages/core/src/installCode.fixtures.json';

const ACCOUNT_KEY = 'cp_connect_account_v1';
const MACHINE_URL = 'https://pc1.acme.hangar.dev.br';
const online: ConnectMachine = { machine_id: 'pc1', name: 'pc1', host: 'pc1.acme.hangar.dev.br', online: true };
const offline: ConnectMachine = { ...online, online: false };
const entry: Server = { id: 'pc1', label: 'pc1', baseUrl: MACHINE_URL, token: 'device-token', credential: 'device', deviceId: 'dev1' };

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const empty = () => new Response(null, { status: 204 });

type Call = { url: URL; method: string; body: Record<string, unknown> | undefined; auth: string | null; device: string | null };
// `device`: o status deste aparelho na conta (`GET /v1/devices/{id}`); `rows`: a lista de aparelhos da conta.
type Overrides = { statuses?: unknown[]; machines?: ConnectMachine[]; rows?: unknown[]; device?: string; route?: (call: Call) => Response | undefined };

function network({ statuses = [], machines = [online], rows = [], device = 'pending', route }: Overrides = {}) {
  const calls: Call[] = [];
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const headers = new Headers(init?.headers);
    const call: Call = { url: new URL(input), method: init?.method ?? 'GET', body: init?.body ? JSON.parse(String(init.body)) : undefined,
      auth: headers.get('Authorization'), device: headers.get('X-Hangar-Device') && `${headers.get('X-Hangar-Device')}:${headers.get('X-Hangar-Device-Secret')}` };
    calls.push(call);
    const { pathname, origin } = call.url;
    const custom = route?.(call);
    if (custom) return custom;
    if (origin === CONNECT_SERVICE_URL) {
      if (pathname === '/v1/devices' && call.method === 'POST') return json({ device_id: 'ad1', device_secret: 'ad-secret', status: 'pending' }, 201);
      if (pathname === '/v1/devices') return json(rows);
      if (pathname.startsWith('/v1/devices/') && call.method === 'GET') return json({ status: device });
      if (pathname.startsWith('/v1/devices/')) return call.method === 'DELETE' ? empty() : json({ status: 'approved' });
      if (pathname === '/v1/machines') return json(machines);
      if (pathname.endsWith('/devices')) return json({ url: MACHINE_URL, ticket: '{"nonce":"n"}', signature: 'c2ln' });
      if (pathname === '/v1/sessions' && call.method === 'DELETE') return empty();
      if (pathname === '/v1/accounts/password') return empty();
      return json({ token: 'account-token' });
    }
    if (pathname === '/api/connect/redeem') return json({ device_id: 'dev1', status: 'pending' }, 202);
    if (pathname === '/api/connect/redeem/status') return json(statuses.shift() ?? { status: 'pending' });
    if (pathname === '/api/connect/account-devices/self') return json({ status: 'approved' });
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

async function seed(list: Server[]) {
  vault.set('cp_servers_v1', JSON.stringify({ servers: list, activeId: list[0]?.id ?? null }));
  await useServers.getState().load();
}

beforeEach(async () => {
  vault.clear();
  broken.read.clear();
  broken.write.clear();
  broken.hang.clear();
  broken.hangWrite.clear();
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

  it.each([
    ['JSON fora da forma', JSON.stringify({ serviceUrl: CONNECT_SERVICE_URL, name: 'acme' })],
    ['JSON quebrado', '{'],
  ])('conta guardada ilegível (%s) vai ao diário e avisa, sem entrar', async (_case, raw) => {
    const events: { codigo?: string }[] = [];
    configureDiag({ registrar: (event) => { events.push(event); }, novoReq: () => '' });
    vault.set(ACCOUNT_KEY, raw);
    await connect().load();
    expect(connect()).toMatchObject({ ready: true, account: null, notice: m.connect_error_vault() });
    expect(events.map((event) => event.codigo)).toEqual(['connect_account_unreadable']);
  });

  it('sem conta guardada não avisa nada', async () => {
    const events: unknown[] = [];
    configureDiag({ registrar: (event) => { events.push(event); }, novoReq: () => '' });
    await connect().load();
    expect(connect()).toMatchObject({ ready: true, account: null, notice: '' });
    expect(events).toEqual([]);
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
    useConnect.setState({ entries: { pc1: { phase: 'failed', error: 'x' } }, devices: { rows: [], error: '' } });

    await connect().signOut();

    expect(connect().entries).toEqual({});
    expect(connect().devices).toEqual({ rows: null, error: '' });
    expect(connect().device).toBeNull();
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
    expect(servers()).toEqual([]);
    await elapse(5000);
    await entering;

    expect(connect().entries.pc1).toBeUndefined();
    expect(servers()).toHaveLength(1);
  });

  it('entra em todas com o mesmo id de instalação, guardado no cofre', async () => {
    const other: ConnectMachine = { machine_id: 'pc2', name: 'pc2', host: 'pc2.acme.hangar.dev.br', online: true };
    const calls = network({ machines: [online, other, { ...offline, machine_id: 'pc3', name: 'pc3', host: 'pc3.acme.hangar.dev.br' }],
      statuses: [{ status: 'approved', token: 'device-a' }, { status: 'approved', token: 'device-b' }],
      route: (call) => {
        const id = /^\/v1\/machines\/(\w+)\/devices$/.exec(call.url.pathname)?.[1];
        if (id) return json({ url: `https://${id}.acme.hangar.dev.br`, ticket: '{"nonce":"n"}', signature: 'c2ln' });
        const isPc2 = call.url.host === 'pc2.acme.hangar.dev.br' && !call.url.pathname.startsWith('/api/connect');
        return isPc2 ? json({ identificador: 'pc2', lan_url: '' }) : undefined;
      } });
    await connect().refreshMachines();

    const entering = connect().enterAll();
    await elapse(6000);
    await entering;

    const redeems = calls.filter((call) => call.url.pathname === '/api/connect/redeem');
    expect(redeems.map((call) => call.url.host).sort()).toEqual(['pc1.acme.hangar.dev.br', 'pc2.acme.hangar.dev.br']);
    const ids = [...new Set(redeems.map((call) => call.body?.install_id))];
    expect(ids).toHaveLength(1);
    expect(ids[0]).toMatch(/^[A-Za-z0-9_-]{16,64}$/);
    expect(vault.get('cp_connect_install_id_v1')).toBe(ids[0]);
    expect(servers().map((s) => s.id).sort()).toEqual(['pc1', 'pc2']);
  });

  it('com o cofre falhando ou mudo, a entrada para com erro e não inventa outro id', async () => {
    const events: { evento: string; codigo?: string }[] = [];
    configureDiag({ registrar: (event) => { events.push(event); }, novoReq: () => '' });
    vault.set('cp_connect_install_id_v1', codes[0].install_id);
    const calls = network({ statuses: [{ status: 'approved', token: 'device-token' }] });
    for (const fault of [broken.read, broken.hang]) {
      fault.add('cp_connect_install_id_v1');
      const entering = connect().enter(online);
      await elapse(6000);
      expect(await entering).toBeNull();
      expect(connect().entries.pc1).toEqual({ phase: 'failed', error: m.connect_error_install_id() });
      fault.clear();
      useConnect.setState({ entries: {} });
    }
    expect(vault.get('cp_connect_install_id_v1')).toBe(codes[0].install_id);
    expect(calls.some((call) => call.url.pathname === '/api/connect/redeem')).toBe(false);
    expect(events.map((event) => event.codigo)).toEqual(['connect_install_id_unreadable', 'connect_install_id_unreadable']);
  });

  it('id guardado fora da regra do servidor é trocado por um novo, gravado e registrado', async () => {
    const events: { codigo?: string }[] = [];
    configureDiag({ registrar: (event) => { events.push(event); }, novoReq: () => '' });
    vault.set('cp_connect_install_id_v1', 'não vale');
    const calls = network({ statuses: [{ status: 'approved', token: 'device-token' }] });
    const entering = connect().enter(online);
    await elapse(6000);
    await entering;
    const saved = vault.get('cp_connect_install_id_v1');
    expect(saved).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    expect(calls.find((call) => call.url.pathname === '/api/connect/redeem')?.body?.install_id).toBe(saved);
    expect(events.map((event) => event.codigo)).toEqual(['connect_install_id_invalid']);
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
    const calls = network({ statuses: [{ status: 'pending' }, { status: 'approved', token: 'device-token' }] });
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

describe('remover um computador da conta', () => {
  const other: Server = { id: 'pc9', label: 'pc9', baseUrl: 'https://pc9.tail.ts.net', token: 'owner-password' };
  beforeEach(async () => {
    network();
    await connect().signIn('acme', 'long-password');
    await settle();
  });

  it('apaga na conta com a credencial do aparelho, tira só o servidor dele e relê a lista', async () => {
    await seed([entry, other]);
    let removed = false;
    const calls = network({ route: (call) => {
      if (call.url.pathname === '/v1/machines/pc1' && call.method === 'DELETE') { removed = true; return empty(); }
      if (call.url.pathname === '/v1/machines') return json(removed ? [] : [online]);
      return undefined;
    } });
    await connect().refreshMachines();

    await connect().removeMachine(online);

    const remove = calls.find((call) => call.method === 'DELETE' && call.url.pathname === '/v1/machines/pc1');
    expect(remove?.device).toBe('ad1:ad-secret');
    expect(remove?.auth).toBe('Bearer account-token');
    expect(servers().map((server) => server.id)).toEqual(['pc9']);
    // A lista relida depois da remoção já não traz o computador.
    expect(calls.slice(calls.indexOf(remove!)).some((call) => call.method === 'GET' && call.url.pathname === '/v1/machines')).toBe(true);
    expect(connect().machines).toEqual([]);
  });

  it('acha o servidor pelo host do Connect, mesmo guardado por outro endereço', async () => {
    await seed([{ ...other, id: 'casa', baseUrl: 'https://casa.tail.ts.net', connectHost: online.host }, other]);
    network({ route: (call) => (call.method === 'DELETE' && call.url.pathname === '/v1/machines/pc1' ? empty() : undefined) });

    await connect().removeMachine(online);

    expect(servers().map((server) => server.id)).toEqual(['pc9']);
  });

  it('recusa do serviço vira a frase da tela e não tira o servidor', async () => {
    await seed([entry, other]);
    network({ route: (call) => (call.method === 'DELETE' && call.url.pathname === '/v1/machines/pc1' ? json({ error: 'eco' }, 503) : undefined) });

    await expect(connect().removeMachine(online)).rejects.toThrow(m.connect_error_unavailable());

    expect(servers().map((server) => server.id)).toEqual(['pc1', 'pc9']);
  });

  it('já fora da conta (404) conta como feito: tira o servidor dele e relê a lista', async () => {
    await seed([entry, other]);
    const calls = network({ route: (call) => {
      if (call.url.pathname === '/v1/machines/pc1' && call.method === 'DELETE') return json({ error: 'não encontrado', reason: null }, 404);
      if (call.url.pathname === '/v1/machines') return json([]);
      return undefined;
    } });

    await connect().removeMachine(online);

    expect(servers().map((server) => server.id)).toEqual(['pc9']);
    const remove = calls.findIndex((call) => call.method === 'DELETE' && call.url.pathname === '/v1/machines/pc1');
    expect(calls.slice(remove).some((call) => call.method === 'GET' && call.url.pathname === '/v1/machines')).toBe(true);
    expect(connect().machines).toEqual([]);
  });

  it('sessão que o serviço recusou (401) derruba a conta e não tira o servidor', async () => {
    await seed([entry, other]);
    network({ route: (call) => (call.method === 'DELETE' && call.url.pathname === '/v1/machines/pc1' ? json({ error: 'sessão' }, 401) : undefined) });

    await expect(connect().removeMachine(online)).rejects.toThrow(m.connect_session_ended());

    expect(connect().account).toBeNull();
    expect(connect().notice).toBe(m.connect_session_ended());
    expect(vault.has(ACCOUNT_KEY)).toBe(false);
    expect(servers().map((server) => server.id)).toEqual(['pc1', 'pc9']);
  });
});

describe('aparelho da conta', () => {
  const DEVICE_KEY = 'cp_connect_device_v1';
  const saveDevice = () => vault.set(DEVICE_KEY, JSON.stringify({ name: 'acme', serviceUrl: CONNECT_SERVICE_URL, id: 'ad1', secret: 'ad-secret' }));
  const tickets = (calls: Call[]) => calls.filter((call) => /^\/v1\/machines\/\w+\/devices$/.test(call.url.pathname));
  const registers = (calls: Call[]) => calls.filter((call) => call.url.pathname === '/v1/devices' && call.method === 'POST');
  const vouches = (calls: Call[]) => calls.filter((call) => call.url.pathname === '/api/connect/account-devices/self');
  const signIn = () => connect().signIn('acme', 'long-password');

  it('no login registra o aparelho, guarda id e segredo no cofre e, pendente, não entra em nada', async () => {
    const calls = network();
    await signIn();
    await settle();

    expect(connect().device).toEqual({ id: 'ad1', secret: 'ad-secret', status: 'pending' });
    expect(JSON.parse(vault.get(DEVICE_KEY)!)).toEqual({ name: 'acme', serviceUrl: CONNECT_SERVICE_URL, id: 'ad1', secret: 'ad-secret' });
    expect(JSON.parse(vault.get(ACCOUNT_KEY)!)).toEqual({ serviceUrl: CONNECT_SERVICE_URL, token: 'account-token', name: 'acme' });
    expect(registers(calls)[0]).toMatchObject({ auth: 'Bearer account-token', body: { name: 'fixture-phone', install_id: vault.get('cp_connect_install_id_v1') } });
    expect(tickets(calls)).toEqual([]);
  });

  it('reutiliza o aparelho guardado e, liberado, não entra sozinho: entra só nas máquinas que a pessoa escolheu', async () => {
    saveDevice();
    const pc2: ConnectMachine = { machine_id: 'pc2', name: 'pc2', host: 'pc2.acme.hangar.dev.br', online: true };
    const calls = network({ device: 'approved', statuses: [{ status: 'approved', token: 'device-a' }],
      route: (call) => {
        if (call.url.pathname === '/v1/machines') return json([online, pc2]);
        const id = /^\/v1\/machines\/(\w+)\/devices$/.exec(call.url.pathname)?.[1];
        if (id) return json({ url: `https://${id}.acme.hangar.dev.br`, ticket: '{"nonce":"n"}', signature: 'c2ln' });
        const isPc2 = call.url.host === 'pc2.acme.hangar.dev.br' && !call.url.pathname.startsWith('/api/connect');
        return isPc2 ? json({ identificador: 'pc2', lan_url: '' }) : undefined;
      } });

    await signIn();
    await vi.waitFor(() => expect(connect().machines?.map((machine) => machine.machine_id)).toEqual(['pc1', 'pc2']));
    await settle();
    expect(connect().device?.status).toBe('approved');
    expect(tickets(calls)).toEqual([]);
    expect(servers()).toEqual([]);

    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
    const entering = connect().enterAll(['pc2']);
    await elapse(6000);
    const saved = await entering;

    expect(saved.map((server) => server.id)).toEqual(['pc2']);
    expect(servers().map((s) => s.id)).toEqual(['pc2']);
    expect(registers(calls)).toEqual([]);
    expect(tickets(calls).map((call) => [call.url.pathname, call.device])).toEqual([['/v1/machines/pc2/devices', 'ad1:ad-secret']]);
  });

  it('entrar com outra caixa no nome usa a conta em minúsculas e reaproveita o aparelho guardado', async () => {
    vault.set(DEVICE_KEY, JSON.stringify({ name: 'Acme', serviceUrl: CONNECT_SERVICE_URL, id: 'ad1', secret: 'ad-secret' }));
    const calls = network({ device: 'approved' });
    await connect().signIn(' ACME ', 'long-password');
    await settle();

    expect(calls.find((call) => call.url.pathname === '/v1/sessions' && call.method === 'POST')?.body?.name).toBe('acme');
    expect(connect().account?.name).toBe('acme');
    expect(JSON.parse(vault.get(ACCOUNT_KEY)!).name).toBe('acme');
    expect(connect().device).toEqual({ id: 'ad1', secret: 'ad-secret', status: 'approved' });
    expect(registers(calls)).toEqual([]);
  });

  it('conta guardada com maiúsculas abre em minúsculas e acha o aparelho', async () => {
    vault.set(ACCOUNT_KEY, JSON.stringify({ serviceUrl: CONNECT_SERVICE_URL, token: 'account-token', name: 'Acme' }));
    saveDevice();
    const calls = network({ device: 'approved' });
    await connect().load();
    expect(connect().account?.name).toBe('acme');
    await connect().refreshDevices();
    expect(connect().device?.id).toBe('ad1');
    expect(registers(calls)).toEqual([]);
  });

  it('pendente com entrada aprovada numa máquina pede a ela que o libere na conta (migração)', async () => {
    await seed([entry]);
    saveDevice();
    let status = 'pending';
    const calls = network({ route: (call) => {
      if (call.url.pathname === '/v1/devices/ad1' && call.method === 'GET') return json({ status });
      if (vouches([call]).length) { status = 'approved'; return json({ status: 'approved' }); }
      return undefined;
    } });

    await signIn();

    expect(connect().device?.status).toBe('approved');
    expect(vouches(calls)).toHaveLength(1);
    expect(vouches(calls)[0]).toMatchObject({ method: 'POST', auth: 'Bearer device-token', body: { device_id: 'ad1', device_secret: 'ad-secret' } });
    expect(vouches(calls)[0].url.origin).toBe(MACHINE_URL);
  });

  it('máquina que não libera deixa o aparelho pendente, e não é perguntada de novo', async () => {
    await seed([entry]);
    saveDevice();
    const calls = network({ route: (call) => (vouches([call]).length ? json({}, 404) : undefined) });

    await signIn();
    await connect().refreshDevices();

    expect(connect().device?.status).toBe('pending');
    expect(connect().deviceError).toBe('');
    expect(vouches(calls)).toHaveLength(1);
  });

  it('pedido vencido registra outro; revogado fica revogado até pedir de novo', async () => {
    saveDevice();
    const newId = (call: Call) => (registers([call]).length ? json({ device_id: 'ad9', device_secret: 'ad9-secret', status: 'pending' }, 201) : undefined);
    network({ route: (call) => newId(call) ?? (call.url.pathname === '/v1/devices/ad1' ? json({}, 404) : undefined) });
    await signIn();
    expect(connect().device).toEqual({ id: 'ad9', secret: 'ad9-secret', status: 'pending' });
    expect(JSON.parse(vault.get(DEVICE_KEY)!)).toMatchObject({ id: 'ad9', secret: 'ad9-secret' });

    await connect().signOut();
    saveDevice();
    const calls = network({ device: 'revoked', route: newId });
    await signIn();
    expect(connect().device?.status).toBe('revoked');
    expect(registers(calls)).toEqual([]);

    await connect().requestDeviceAgain();
    expect(connect().device).toEqual({ id: 'ad9', secret: 'ad9-secret', status: 'pending' });
  });

  it('cofre ilegível não registra outro aparelho por cima do guardado', async () => {
    saveDevice();
    broken.read.add(DEVICE_KEY);
    const calls = network();
    await signIn();
    expect(connect().device).toBeNull();
    expect(connect().deviceError).toBe(m.connect_error_vault());
    expect(registers(calls)).toEqual([]);
  });

  it('sessão vencida na consulta do aparelho volta ao login', async () => {
    saveDevice();
    network({ route: (call) => (call.url.pathname === '/v1/devices/ad1' ? json({}, 401) : undefined) });
    await signIn();
    expect(connect().account).toBeNull();
    expect(connect().notice).toBe(m.connect_session_ended());
  });

  it('liberado, lista os aparelhos da conta e libera, recusa ou tira acesso pelo serviço', async () => {
    saveDevice();
    const rows = [{ device_id: 'ad1', name: 'fixture-phone', status: 'approved', created_at: 1, approved_at: 2, revoked_at: null },
      { device_id: 'ad2', name: 'Pixel', status: 'pending', created_at: 3, approved_at: null, revoked_at: null }];
    const calls = network({ device: 'approved', rows, machines: [] });
    await signIn();
    expect(connect().devices).toEqual({ rows, error: '' });

    await connect().answerDevice('ad2', true);
    await connect().answerDevice('ad3', false);

    expect(calls.filter((call) => /^\/v1\/devices\/ad[23]/.test(call.url.pathname)).map((call) => `${call.method} ${call.url.pathname} ${call.device}`))
      .toEqual(['POST /v1/devices/ad2/approve ad1:ad-secret', 'DELETE /v1/devices/ad3 ad1:ad-secret']);
  });

  it('mantém a lista na tela quando a leitura falha', async () => {
    saveDevice();
    const rows = [{ device_id: 'ad1', name: 'fixture-phone', status: 'approved', created_at: 1, approved_at: 2, revoked_at: null }];
    network({ device: 'approved', rows, machines: [] });
    await signIn();
    network({ device: 'approved', machines: [], route: (call) => (call.url.pathname === '/v1/devices' ? json({}, 500) : undefined) });

    await connect().refreshDevices();

    expect(connect().devices.rows).toEqual(rows);
    expect(connect().devices.error).toBe(m.connect_error_unavailable());
  });

  it('bilhete recusado porque o aparelho perdeu o acesso marca o aparelho e diz o motivo', async () => {
    saveDevice();
    network({ device: 'approved', machines: [],
      route: (call) => (tickets([call]).length ? json({ error: 'fixture', reason: 'device_revoked' }, 403) : undefined) });
    await signIn();

    expect(await connect().enter(online)).toBeNull();

    expect(connect().entries.pc1).toEqual({ phase: 'failed', error: m.connect_error_device_revoked() });
    expect(connect().device?.status).toBe('revoked');
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
