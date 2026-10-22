import { describe, test, expect, vi, beforeEach } from 'vitest';

const vault = vi.hoisted(() => new Map<string, string>());
const diag = vi.hoisted(() => vi.fn());
const probe = vi.hoisted(() => vi.fn());
const realCore = await vi.importActual<typeof import('@hangar/core')>('@hangar/core');
vi.mock('@hangar/core', () => ({ ...realCore, registrarDiag: diag, probeServerResponse: probe }));
vi.mock('expo-secure-store', () => ({
  getItemAsync: vi.fn(),
  setItemAsync: vi.fn(),
  deleteItemAsync: vi.fn(),
}));

import * as SecureStore from 'expo-secure-store';
let useServers: typeof import('./servers').useServers;
let ownServers: typeof import('./servers').ownServers;

beforeEach(async () => {
  vi.resetModules();
  vault.clear();
  diag.mockClear();
  probe.mockReset().mockImplementation(async () => new Response(JSON.stringify({ identificador: '', lan_url: '' })));
  vi.mocked(SecureStore.getItemAsync).mockReset().mockImplementation(async (k) => vault.get(k) ?? null);
  vi.mocked(SecureStore.setItemAsync).mockReset().mockImplementation(async (k, v) => { vault.set(k, v); });
  vi.mocked(SecureStore.deleteItemAsync).mockReset().mockImplementation(async (k) => { vault.delete(k); });
  ({ useServers, ownServers } = await import('./servers'));
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

const settle = () => new Promise<void>((resolve) => setImmediate(resolve));

const SAVED_SERVER = {
  id: 'saved', label: 'Servidor salvo', baseUrl: 'http://10.0.0.2:8765', token: 'saved-token',
  disabled: false, invite: false, inviteEnded: false, removed: false, ephemeral: false,
  lan: { url: 'http://192.168.0.2:8765', id: 'saved-machine' },
};
const SAVED = { servers: [SAVED_SERVER], activeId: SAVED_SERVER.id };
const VAULT_KEY = 'cp_servers_v1';

test('trocar a identidade confirmada descarta a alternativa ligada ao identificador anterior', async () => {
  const saved = { ...SAVED_SERVER, remote: { id: 'saved-machine', url: 'https://saved.tail.ts.net' } };
  vault.set(VAULT_KEY, JSON.stringify({ servers: [saved], activeId: saved.id }));
  await useServers.getState().load();

  useServers.getState().rememberLan(saved.id, { id: 'renamed-machine', url: saved.lan.url });

  expect(useServers.getState().servers[0].remote).toBeUndefined();
});

test('guarda a alternativa remota confirmada e ignora a resposta de uma credencial antiga', async () => {
  vault.set(VAULT_KEY, JSON.stringify(SAVED));
  await useServers.getState().load();
  const remote = { id: 'saved-machine', url: 'https://saved.tail.ts.net' };
  useServers.getState().rememberRemote(SAVED_SERVER.id, remote, SAVED_SERVER);
  await settle();
  expect(useServers.getState().servers[0].remote).toEqual(remote);
  expect(JSON.parse(vault.get(VAULT_KEY)!).servers[0].remote).toEqual(remote);

  useServers.getState().add({ baseUrl: SAVED_SERVER.baseUrl, token: 'new-token' });
  const late = { id: 'saved-machine', url: 'https://old-response.tail.ts.net' };
  useServers.getState().rememberRemote(SAVED_SERVER.id, late, SAVED_SERVER);
  await settle();
  expect(useServers.getState().servers[0].remote).toEqual(remote);
});

const mutations = [
  () => useServers.getState().add({ baseUrl: 'http://10.0.0.3:8765', token: 'new-token' }),
  () => useServers.getState().remove(SAVED_SERVER.id),
  () => useServers.getState().setActive(SAVED_SERVER.id),
  () => useServers.getState().rememberLan(useServers.getState().servers[0].id, { url: '', id: '' }),
];

describe('proteção do cofre', () => {
  test.each([
    ['leitura rejeitada', JSON.stringify(SAVED), true],
    ['JSON incompleto', '{', false],
    ['texto vazio', '', false],
    ['JSON nulo', 'null', false],
    ['lista no lugar do objeto', '[]', false],
    ['servers ausente', '{"activeId":null}', false],
    ['servers não é array', '{"servers":{},"activeId":null}', false],
    ['servidor nulo', '{"servers":[null],"activeId":null}', false],
    ['token ausente', JSON.stringify({ ...SAVED, servers: [{ ...SAVED_SERVER, token: undefined }] }), false],
    ['id inválido', JSON.stringify({ ...SAVED, servers: [{ ...SAVED_SERVER, id: 3 }] }), false],
    ['label inválido', JSON.stringify({ ...SAVED, servers: [{ ...SAVED_SERVER, label: null }] }), false],
    ['baseUrl inválida', JSON.stringify({ ...SAVED, servers: [{ ...SAVED_SERVER, baseUrl: false }] }), false],
    ...['disabled', 'invite', 'inviteEnded', 'removed', 'ephemeral'].map((field) => [
      `${field} inválido`, JSON.stringify({ ...SAVED, servers: [{ ...SAVED_SERVER, [field]: 'false' }] }), false,
    ] as const),
    ['lan nula', JSON.stringify({ ...SAVED, servers: [{ ...SAVED_SERVER, lan: null }] }), false],
    ['lan sem URL', JSON.stringify({ ...SAVED, servers: [{ ...SAVED_SERVER, lan: { id: 'machine' } }] }), false],
    ['lan sem id', JSON.stringify({ ...SAVED, servers: [{ ...SAVED_SERVER, lan: { url: '' } }] }), false],
    ['activeId inválido', JSON.stringify({ ...SAVED, activeId: 3 }), false],
    ['activeId ausente', JSON.stringify({ servers: [] }), false],
  ] as const)('%s conserva estado e bloqueia todas as escritas mesmo após limpar aviso', async (_name, raw, rejected) => {
    await SecureStore.setItemAsync(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    await SecureStore.setItemAsync(VAULT_KEY, raw);
    vi.mocked(SecureStore.setItemAsync).mockClear();
    if (rejected) vi.mocked(SecureStore.getItemAsync)
      .mockRejectedValueOnce(new Error('cofre indisponível'))
      .mockRejectedValueOnce(new Error('cofre indisponível'));

    await useServers.getState().load();

    expect(useServers.getState()).toMatchObject({
      ...SAVED, ready: true, vaultReadable: false, aviso: { tipo: 'vault_unread' },
    });
    for (const [index, mutate] of mutations.entries()) {
      useServers.getState().limparAviso();
      mutate();
      await settle();
      expect(useServers.getState().aviso).toEqual(index < 3 ? { tipo: 'vault_unread' } : null);
      expect(useServers.getState().vaultReadable).toBe(false);
      expect(SecureStore.setItemAsync).not.toHaveBeenCalled();
      expect(SecureStore.deleteItemAsync).not.toHaveBeenCalled();
      expect(vault.get(VAULT_KEY)).toBe(raw);
    }
  });

  test('add antes da primeira leitura sobrevive, junta o salvo e deduplica load em voo', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);
    const added = useServers.getState().add({ baseUrl: 'http://10.0.0.4:8765', token: 'memory-token' });
    const loading = useServers.getState().load();
    const again = useServers.getState().load();
    await settle();
    expect(SecureStore.getItemAsync).toHaveBeenCalledTimes(1);
    expect(SecureStore.setItemAsync).not.toHaveBeenCalled();
    expect(vault.get(VAULT_KEY)).toBe(JSON.stringify(SAVED));

    read.resolve(JSON.stringify(SAVED));
    await Promise.all([loading, again]);
    expect(useServers.getState()).toMatchObject({
      servers: [SAVED_SERVER, added], activeId: added.id, ready: true, vaultReadable: true, aviso: null,
    });
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual({ servers: [SAVED_SERVER, added], activeId: added.id });
  });

  test('cofre ausente libera escrita e guarda o servidor com seu token', async () => {
    await useServers.getState().load();
    expect(useServers.getState()).toMatchObject({ servers: [], activeId: null, vaultReadable: true, aviso: null });

    const added = useServers.getState().add({ baseUrl: 'http://10.0.0.2:8765', token: 'new-token' });
    expect(SecureStore.setItemAsync).toHaveBeenCalledTimes(1);
    expect(JSON.parse((await SecureStore.getItemAsync(VAULT_KEY))!)).toEqual({ servers: [added], activeId: added.id });
  });

  test('primeiro add após falha temporária relê o cofre e junta salvo com novo', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(new Error('bloqueado'));
    await useServers.getState().load();
    useServers.getState().limparAviso();
    const added = useServers.getState().add({ baseUrl: 'http://10.0.0.3:8765', token: 'memory-token', label: 'fixture-memory' });
    await settle();

    expect(SecureStore.getItemAsync).toHaveBeenCalledTimes(2);
    expect(useServers.getState()).toMatchObject({
      servers: [SAVED_SERVER, added], activeId: added.id, vaultReadable: true, aviso: null,
    });
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual({ servers: [SAVED_SERVER, added], activeId: added.id });
  });

  test('falha persistente não repete leitura nem aviso a cada rememberLan', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    vi.mocked(SecureStore.getItemAsync).mockRejectedValue(new Error('bloqueado'));
    await useServers.getState().load();
    expect(useServers.getState().aviso).toEqual({ tipo: 'vault_unread' });
    useServers.getState().limparAviso();
    const added = useServers.getState().add({ baseUrl: 'http://10.0.0.3:8765', token: 'memory-token' });
    await settle();
    expect(takeWarnings()).toEqual([{ tipo: 'vault_unread' }]);
    for (let i = 0; i < 10; i++) {
      useServers.getState().rememberLan(added.id, { url: '', id: '' });
      await settle();
      expect(useServers.getState().aviso).toBeNull();
    }
    expect(SecureStore.getItemAsync).toHaveBeenCalledTimes(2);
    expect(SecureStore.setItemAsync).not.toHaveBeenCalled();
    expect(SecureStore.deleteItemAsync).not.toHaveBeenCalled();
    expect(vault.get(VAULT_KEY)).toBe(JSON.stringify(SAVED));
    expect(useServers.getState().vaultReadable).toBe(false);
  });

  test.each(['add', 'remove', 'setActive'] as const)('%s manual reavisa após dispensar a mesma falha', async (action) => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    vi.mocked(SecureStore.getItemAsync).mockRejectedValue(new Error('bloqueado'));
    await useServers.getState().load();
    useServers.getState().limparAviso();
    const mutate = mutations[['add', 'remove', 'setActive'].indexOf(action)];
    mutate();
    await settle();
    expect(takeWarnings()).toEqual([{ tipo: 'vault_unread' }]);
    mutate();
    mutate();
    await settle();
    expect(takeWarnings()).toEqual([{ tipo: 'vault_unread' }]);
    expect(SecureStore.getItemAsync).toHaveBeenCalledTimes(3);
    expect(SecureStore.setItemAsync).not.toHaveBeenCalled();
    expect(vault.get(VAULT_KEY)).toBe(JSON.stringify(SAVED));
  });

  test('load ilegível reavisa após dispensar e não duplica aviso ainda na fila', async () => {
    vault.set(VAULT_KEY, '{');
    await useServers.getState().load();
    expect(takeWarnings()).toEqual([{ tipo: 'vault_unread' }]);
    await useServers.getState().load();
    await useServers.getState().load();
    expect(takeWarnings()).toEqual([{ tipo: 'vault_unread' }]);
  });

  test('rememberLan em releitura automática não repete aviso já dispensado', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    vi.mocked(SecureStore.getItemAsync).mockRejectedValue(new Error('bloqueado'));
    await useServers.getState().load();
    useServers.getState().limparAviso();
    for (let i = 0; i < 10; i++) {
      useServers.getState().rememberLan(SAVED_SERVER.id, { url: '', id: '' });
      await settle();
    }
    expect(takeWarnings()).toEqual([]);
    expect(SecureStore.getItemAsync).toHaveBeenCalledTimes(3);
    expect(SecureStore.setItemAsync).not.toHaveBeenCalled();
  });

  test('ação manual durante releitura automática só avisa se a releitura falhar', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(new Error('bloqueado'));
    await useServers.getState().load();
    useServers.getState().limparAviso();
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);
    useServers.getState().rememberLan(SAVED_SERVER.id, { url: '', id: '' });
    useServers.getState().setActive(SAVED_SERVER.id);
    expect(useServers.getState().aviso).toBeNull();
    read.reject(new Error('bloqueado'));
    await settle();
    expect(takeWarnings()).toEqual([{ tipo: 'vault_unread' }]);
  });

  test('load público recupera no retorno ao primeiro plano e reavisa apenas uma falha nova', async () => {
    vault.set(VAULT_KEY, '{');
    await useServers.getState().load();
    const added = useServers.getState().add({ baseUrl: 'http://10.0.0.3:8765', token: 'memory-token' });
    await settle();
    useServers.getState().limparAviso();
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    expect(useServers.getState()).toMatchObject({
      servers: [SAVED_SERVER, added], activeId: added.id, vaultReadable: true, aviso: null,
    });
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual({ servers: [SAVED_SERVER, added], activeId: added.id });
    vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(new Error('bloqueado novamente'));
    await useServers.getState().load();
    expect(useServers.getState().aviso).toEqual({ tipo: 'vault_unread' });
    expect(useServers.getState().vaultReadable).toBe(false);
  });

  test('vários adds durante load mantêm tokens novos e o ID salvo para a mesma base', async () => {
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);
    const loading = useServers.getState().load();
    const updated = useServers.getState().add({ baseUrl: `${SAVED_SERVER.baseUrl}/`, token: 'fresh-token', label: 'não substituir' });
    const added = useServers.getState().add({ baseUrl: 'http://10.0.0.3:8765', token: 'b' });
    const third = useServers.getState().add({ baseUrl: 'http://10.0.0.4:8765', token: 'c' });
    useServers.getState().add({ baseUrl: SAVED_SERVER.baseUrl, token: 'latest-token' });
    useServers.getState().rememberLan(updated.id, { url: '', id: 'new-lan' });
    useServers.getState().setActive(updated.id);
    read.resolve(JSON.stringify(SAVED));
    await loading;

    const expected = { servers: [{ ...SAVED_SERVER, token: 'latest-token', lan: { url: '', id: 'new-lan' } }, added, third], activeId: SAVED_SERVER.id };
    expect(new Set([updated.id, added.id, third.id]).size).toBe(3);
    expect(useServers.getState()).toMatchObject({ ...expected, vaultReadable: true });
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual(expected);
  });

  test('remove, ativa e LAN pendentes são reaplicados sem perder máquinas desconhecidas', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(new Error('bloqueado'));
    await useServers.getState().load();
    const other = { ...SAVED_SERVER, id: 'other', baseUrl: 'http://10.0.0.5:8765', token: 'other-token' };
    const unseen = { ...SAVED_SERVER, id: 'unseen', baseUrl: 'http://10.0.0.6:8765', token: 'unseen-token' };
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);
    useServers.getState().remove(SAVED_SERVER.id);
    useServers.getState().rememberLan(other.id, { url: '', id: 'updated-machine' });
    useServers.getState().setActive(other.id);
    read.resolve(JSON.stringify({ servers: [SAVED_SERVER, other, unseen], activeId: SAVED_SERVER.id }));
    await useServers.getState().load();

    const expected = { servers: [{ ...other, lan: { url: '', id: 'updated-machine' } }, unseen], activeId: other.id };
    expect(useServers.getState()).toMatchObject(expected);
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual(expected);
  });

  test('remover add com ID provisório remove também a base descoberta no cofre', async () => {
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);
    const loading = useServers.getState().load();
    const added = useServers.getState().add({ baseUrl: SAVED_SERVER.baseUrl, token: 'fresh' });
    useServers.getState().remove(added.id);
    read.resolve(JSON.stringify(SAVED));
    await loading;
    expect(useServers.getState()).toMatchObject({ servers: [], activeId: null });
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual({ servers: [], activeId: null });
  });

  test('gravação da recuperação nunca termina por cima de alterações posteriores', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(new Error('bloqueado'));
    await useServers.getState().load();
    const write = deferred<void>();
    vi.mocked(SecureStore.setItemAsync).mockImplementationOnce(async (key, raw) => {
      await write.promise;
      vault.set(key, raw);
    });
    useServers.getState().add({ baseUrl: 'http://10.0.0.3:8765', token: 'first' });
    await settle();
    const last = useServers.getState().add({ baseUrl: 'http://10.0.0.4:8765', token: 'last' });
    write.resolve();
    await settle();
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual({ servers: useServers.getState().servers, activeId: last.id });
    expect(useServers.getState().servers).toHaveLength(3);
  });
});

function takeWarnings() {
  const warnings = [];
  while (useServers.getState().aviso) {
    warnings.push(useServers.getState().aviso);
    useServers.getState().limparAviso();
  }
  return warnings;
}

describe('avisos independentes', () => {
  test.each(['add', 'remove', 'setActive'] as const)('%s manual reavisa persistência e não duplica aviso atual', async (action) => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    vi.mocked(SecureStore.setItemAsync).mockRejectedValue(new Error('disco cheio'));
    useServers.getState().setActive(SAVED_SERVER.id);
    await settle();
    useServers.getState().limparAviso();
    const mutate = mutations[['add', 'remove', 'setActive'].indexOf(action)];
    mutate();
    mutate();
    await settle();
    expect(takeWarnings()).toEqual([{ tipo: 'persistencia' }]);
    expect(vault.get(VAULT_KEY)).toBe(JSON.stringify(SAVED));
  });

  test('persistência automática avisa só na primeira falha da fase, sucesso libera novo aviso', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    vi.mocked(SecureStore.setItemAsync).mockRejectedValue(new Error('disco cheio'));
    useServers.getState().rememberLan(SAVED_SERVER.id, { url: '', id: '' });
    await settle();
    expect(takeWarnings()).toEqual([{ tipo: 'persistencia' }]);
    for (let i = 0; i < 10; i++) {
      useServers.getState().rememberLan(SAVED_SERVER.id, { url: '', id: '' });
      await settle();
    }
    expect(takeWarnings()).toEqual([]);
    vi.mocked(SecureStore.setItemAsync).mockResolvedValueOnce();
    useServers.getState().rememberLan(SAVED_SERVER.id, { url: '', id: '' });
    await settle();
    useServers.getState().rememberLan(SAVED_SERVER.id, { url: '', id: '' });
    await settle();
    expect(takeWarnings()).toEqual([{ tipo: 'persistencia' }]);
  });

  test('recuperação com escrita recusada conserva intenção manual e fila de todos os motivos', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    vi.mocked(SecureStore.setItemAsync).mockRejectedValue(new Error('disco cheio'));
    useServers.getState().setActive(SAVED_SERVER.id);
    await settle();
    useServers.getState().limparAviso();
    vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(new Error('bloqueado'));
    await useServers.getState().load();
    useServers.getState().markInvalid(SAVED_SERVER.id);
    await settle();
    expect(takeWarnings()).toEqual([
      { tipo: 'token', label: SAVED_SERVER.label }, { tipo: 'persistencia' },
    ]);
    expect(useServers.getState()).toMatchObject({ vaultReadable: true, servers: [] });
    expect(vault.get(VAULT_KEY)).toBe(JSON.stringify(SAVED));
    vi.mocked(SecureStore.getItemAsync).mockRejectedValue(new Error('bloqueado'));
    await useServers.getState().load();
    useServers.getState().markInvalid('another');
    await settle();
    useServers.getState().setActive('another');
    expect(takeWarnings()).toEqual([{ tipo: 'vault_unread' }, { tipo: 'token', label: '' }]);
  });

  test.each([true, false])('token e leitura recusada coexistem, token primeiro: %s', async (tokenFirst) => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise).mockRejectedValue(new Error('bloqueado'));
    const loading = useServers.getState().load();
    if (tokenFirst) useServers.getState().markInvalid(SAVED_SERVER.id);
    read.reject(new Error('bloqueado'));
    await loading;
    if (!tokenFirst) useServers.getState().markInvalid(SAVED_SERVER.id);
    await settle();
    const token = { tipo: 'token', label: SAVED_SERVER.label };
    const unread = { tipo: 'vault_unread' };
    expect(takeWarnings()).toEqual(tokenFirst ? [token, unread] : [unread, token]);
    expect(useServers.getState().vaultReadable).toBe(false);
    expect(SecureStore.setItemAsync).not.toHaveBeenCalled();
  });

  test('leitura válida apaga só aviso do cofre, não o token pendente', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(new Error('bloqueado'));
    await useServers.getState().load();
    useServers.getState().markInvalid(SAVED_SERVER.id);
    await useServers.getState().load();
    expect(takeWarnings()).toEqual([{ tipo: 'token', label: SAVED_SERVER.label }]);
    expect(useServers.getState().servers).toEqual([]);
  });

  test.each([true, false])('token e persistência recusada coexistem, token primeiro: %s', async (tokenFirst) => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    const write = deferred<void>();
    vi.mocked(SecureStore.setItemAsync).mockReturnValueOnce(write.promise).mockRejectedValue(new Error('disco cheio'));
    useServers.getState().setActive(SAVED_SERVER.id);
    if (tokenFirst) useServers.getState().markInvalid(SAVED_SERVER.id);
    write.reject(new Error('disco cheio'));
    await settle();
    if (!tokenFirst) useServers.getState().markInvalid(SAVED_SERVER.id);
    await settle();
    const token = { tipo: 'token', label: SAVED_SERVER.label };
    const failed = { tipo: 'persistencia' };
    expect(takeWarnings()).toEqual(tokenFirst ? [token, failed] : [failed, token]);
    useServers.getState().setActive(SAVED_SERVER.id);
    await settle();
    expect(takeWarnings()).toEqual([failed]);
    vi.mocked(SecureStore.setItemAsync).mockImplementationOnce(async (key, raw) => { vault.set(key, raw); });
    useServers.getState().setActive(SAVED_SERVER.id);
    await settle();
    useServers.getState().setActive(SAVED_SERVER.id);
    await settle();
    expect(takeWarnings()).toEqual([failed]);
  });
});

describe('diário seguro do cofre', () => {
  test.each([
    ['read', 'servers_vault_read_failed', 'vault_error'],
    ['parse', 'servers_vault_corrupt', 'SyntaxError'],
    ['schema', 'servers_vault_corrupt', 'vault_error'],
    ['write', 'servers_vault_write_failed', 'vault_error'],
  ] as const)('%s registra etapa, código e tipo, nunca o segredo', async (stage, code, errorType) => {
    const secret = 'token-ultrassecreto-NAO-REGISTRAR';
    const error = new Error(`SecureStore recusou ${secret}`);
    error.name = secret;
    vault.set(VAULT_KEY, JSON.stringify({ ...SAVED, servers: [{ ...SAVED_SERVER, token: secret }] }));
    if (stage === 'read') vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(error);
    if (stage === 'parse') vault.set(VAULT_KEY, `{"token":"${secret}"`);
    if (stage === 'schema') vault.set(VAULT_KEY, JSON.stringify({ token: secret }));
    await useServers.getState().load();
    if (stage === 'write') {
      vi.mocked(SecureStore.setItemAsync).mockRejectedValueOnce(error);
      useServers.getState().setActive(SAVED_SERVER.id);
      await settle();
    }
    expect(diag).toHaveBeenCalledExactlyOnceWith({
      evento: 'servidores.cofre_falhou', nivel: 'erro', tela: 'servidores',
      codigo: code, operacao: stage, detalhe: errorType,
    });
    expect(JSON.stringify(diag.mock.calls)).not.toContain(secret);
    expect(JSON.stringify(diag.mock.calls)).not.toContain(SAVED_SERVER.baseUrl);
    expect(useServers.getState().aviso).toEqual({ tipo: stage === 'write' ? 'persistencia' : 'vault_unread' });
  });
});

describe('label no add', () => {
  test('sem nome explícito usa rótulo neutro e persiste o identificador da máquina', async () => {
    await useServers.getState().load();
    const response = deferred<Response>();
    probe.mockReturnValueOnce(response.promise);
    const m = await import('../paraglide/messages');

    const added = useServers.getState().add({ baseUrl: 'http://10.0.2.2:8765', token: 'abc' });

    expect(added.label).toBe(m.native_settings_group_server());
    expect(useServers.getState().active()?.label).toBe(m.native_settings_group_server());
    await settle();
    expect(JSON.parse(vault.get(VAULT_KEY)!).servers[0].label).toBe(m.native_settings_group_server());
    expect(probe).toHaveBeenCalledWith(expect.objectContaining({ baseUrl: added.baseUrl, token: 'abc' }), '/api/peers/identificador', expect.objectContaining({ signal: expect.any(AbortSignal) }));
    response.resolve(new Response(JSON.stringify({ identificador: 'machine-home', lan_url: '' })));
    await settle();
    expect(useServers.getState().active()?.label).toBe('machine-home');
    expect(JSON.parse(vault.get(VAULT_KEY)!).servers[0].label).toBe('machine-home');
    expect(useServers.getState().aviso).toBeNull();
  });

  test('conserva o nome explícito em memória e no cofre', async () => {
    await useServers.getState().load();

    const added = useServers.getState().add({ baseUrl: 'http://10.0.2.2:8765', token: 'abc', label: 'Minha máquina' });

    expect(added.label).toBe('Minha máquina');
    expect(useServers.getState().active()?.label).toBe('Minha máquina');
    await settle();
    expect(JSON.parse(vault.get(VAULT_KEY)!).servers[0].label).toBe('Minha máquina');
    expect(probe).not.toHaveBeenCalled();
  });

  test.each(['outro nome', 'mesmo rótulo neutro'])('resposta atrasada não substitui edição manual: %s', async (edit) => {
    await useServers.getState().load();
    const response = deferred<Response>();
    probe.mockReturnValueOnce(response.promise);
    const added = useServers.getState().add({ baseUrl: 'http://10.0.2.2:8765', token: 'abc' });
    await settle();
    const label = edit === 'outro nome' ? 'Meu notebook' : added.label;

    useServers.getState().update(added.id, { label });
    response.resolve(new Response(JSON.stringify({ identificador: 'stale-machine', lan_url: '' })));
    await settle();

    expect(useServers.getState().active()?.label).toBe(label);
    expect(JSON.parse(vault.get(VAULT_KEY)!).servers[0].label).toBe(label);
  });

  test.each(['endereço', 'token', 'remover e recadastrar'])('resposta atrasada não alcança cadastro alterado: %s', async (edit) => {
    await useServers.getState().load();
    const response = deferred<Response>();
    probe.mockReturnValueOnce(response.promise);
    const added = useServers.getState().add({ baseUrl: 'http://10.0.2.2:8765', token: 'abc' });
    await settle();

    if (edit === 'endereço') useServers.getState().update(added.id, { baseUrl: 'http://10.0.2.3:8765' });
    if (edit === 'token') useServers.getState().add({ baseUrl: added.baseUrl, token: 'changed' });
    if (edit === 'remover e recadastrar') {
      useServers.getState().remove(added.id);
      useServers.getState().add({ baseUrl: added.baseUrl, token: 'new', label: 'Outra máquina' });
    }
    const expected = structuredClone(useServers.getState().servers);
    response.resolve(new Response(JSON.stringify({ identificador: 'stale-machine', lan_url: '' })));
    await settle();

    expect(useServers.getState().servers).toEqual(expected);
    expect(JSON.parse(vault.get(VAULT_KEY)!).servers).toEqual(expected);
  });

  test('resposta durante releitura aguarda o cofre antes de persistir o identificador', async () => {
    await useServers.getState().load();
    const response = deferred<Response>();
    probe.mockReturnValueOnce(response.promise);
    const added = useServers.getState().add({ baseUrl: 'http://10.0.2.2:8765', token: 'abc' });
    await settle();
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);
    const loading = useServers.getState().load();

    response.resolve(new Response(JSON.stringify({ identificador: 'machine-home', lan_url: '' })));
    await settle();
    expect(useServers.getState().active()?.label).toBe(added.label);
    read.resolve(JSON.stringify({ servers: [added], activeId: added.id }));
    await loading;
    await settle();

    expect(useServers.getState().active()?.label).toBe('machine-home');
    expect(JSON.parse(vault.get(VAULT_KEY)!).servers[0].label).toBe('machine-home');
  });

  test.each([false, true])('falha durante releitura aguarda o cofre e respeita edição manual: %s', async (edited) => {
    await useServers.getState().load();
    const response = deferred<Response>();
    probe.mockReturnValueOnce(response.promise);
    const added = useServers.getState().add({ baseUrl: 'http://10.0.2.2:8765', token: 'abc' });
    await settle();
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);
    const loading = useServers.getState().load();

    response.reject(new Error('identity-offline'));
    await settle();
    expect(useServers.getState().aviso).toBeNull();
    if (edited) useServers.getState().update(added.id, { label: 'Meu notebook' });
    read.resolve(JSON.stringify({ servers: [added], activeId: added.id }));
    await loading;
    await settle();

    expect(useServers.getState().active()?.label).toBe(edited ? 'Meu notebook' : added.label);
    expect(takeWarnings()).toEqual(edited ? [] : [{ tipo: 'identification', message: 'identity-offline' }]);
  });

  test.each(['resposta vazia', 'HTTP recusado', 'rede indisponível'])('identificação falha mantém rótulo neutro e mostra a causa: %s', async (failure) => {
    await useServers.getState().load();
    const m = await import('../paraglide/messages');
    if (failure === 'HTTP recusado') probe.mockResolvedValueOnce(new Response(JSON.stringify({ detail: 'identity-refused' }), { status: 503 }));
    if (failure === 'rede indisponível') probe.mockRejectedValueOnce(new Error('identity-offline'));

    useServers.getState().add({ baseUrl: 'http://10.0.2.2:8765', token: 'abc' });
    await settle();

    expect(useServers.getState().active()?.label).toBe(m.native_settings_group_server());
    expect(JSON.parse(vault.get(VAULT_KEY)!).servers[0].label).toBe(m.native_settings_group_server());
    const cause = failure === 'resposta vazia' ? m.mobile_machines_invalid_response()
      : failure === 'HTTP recusado' ? '503: identity-refused' : 'identity-offline';
    expect(takeWarnings()).toEqual([{ tipo: 'identification', message: cause }]);
  });

  test('cadastro salvo descoberto durante load conserva apelido e não consulta nome', async () => {
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);
    const loading = useServers.getState().load();
    const saved = { ...SAVED_SERVER, baseUrl: 'http://10.0.2.2:8765', label: '10' };
    useServers.getState().add({ baseUrl: saved.baseUrl, token: 'renewed' });

    read.resolve(JSON.stringify({ servers: [saved], activeId: saved.id }));
    await loading;
    await settle();
    useServers.getState().add({ baseUrl: saved.baseUrl, token: 'latest' });
    await settle();

    const expected = { ...saved, token: 'latest' };
    expect(useServers.getState().servers).toEqual([expected]);
    expect(JSON.parse(vault.get(VAULT_KEY)!).servers).toEqual([expected]);
    expect(probe).not.toHaveBeenCalled();
  });
});

test('add/setActive/remove persistem', async () => {
  await useServers.getState().load();
  const s = useServers.getState().add({ baseUrl: 'http://10.0.0.2:8765/', token: 'abc' });
  expect(s.baseUrl).toBe('http://10.0.0.2:8765'); // sem barra final
  expect(useServers.getState().active()?.id).toBe(s.id);
  useServers.getState().remove(s.id);
  expect(useServers.getState().active()).toBeNull();
});

const PAIR = { local_session: 'api', alias: 'casa', owner: 'Ana', session: 'web', address: 'https://casa.ts.net:8443', token: 'tp' };

describe('entradas de par externo', () => {
  test('entrada temporária nunca troca o token do dono no mesmo endereço', async () => {
    await useServers.getState().load();
    useServers.getState().applyPairs([PAIR]);
    const dono = useServers.getState().add({ baseUrl: 'https://casa.ts.net:8443', token: 'dono' });
    const lista = useServers.getState().servers;
    expect(lista).toHaveLength(2);
    expect(lista.find((s) => s.ephemeral)?.token).toBe('tp');
    expect(dono.token).toBe('dono');
    expect(dono.invite).toBeFalsy();
  });

  test('não é gravada, e abrir a sessão dela não troca a ativa guardada', async () => {
    await useServers.getState().load();
    const dono = useServers.getState().add({ baseUrl: 'http://10.0.0.2:8765', token: 'abc' });
    useServers.getState().applyPairs([PAIR]);
    const par = useServers.getState().servers.find((s) => s.ephemeral)!;
    useServers.getState().ensureActive(par.id);
    useServers.getState().rememberLan(dono.id, { url: '', id: '' });
    await Promise.resolve();
    const SecureStore = await import('expo-secure-store');
    const salvo = JSON.parse((await SecureStore.getItemAsync('cp_servers_v1'))!);
    expect(salvo.servers.map((s: { id: string }) => s.id)).toEqual([dono.id]);
    expect(salvo.activeId).toBe(dono.id);
  });

  test('401 em entrada de convite marca encerrado, nunca remove nem acusa o token', async () => {
    await useServers.getState().load();
    useServers.getState().applyPairs([PAIR]);
    const par = useServers.getState().servers.find((s) => s.ephemeral)!;
    useServers.getState().markInvalid(par.id);
    const depois = useServers.getState().servers.find((s) => s.id === par.id);
    expect(depois?.inviteEnded).toBe(true);
    expect(useServers.getState().aviso).toBeNull();
  });

  test('par que acabou leva a entrada embora', async () => {
    await useServers.getState().load();
    useServers.getState().applyPairs([PAIR]);
    useServers.getState().applyPairs([]);
    expect(useServers.getState().servers).toHaveLength(0);
  });

  test('sair da conversa de fora volta a ativa para a máquina própria', async () => {
    await useServers.getState().load();
    const dono = useServers.getState().add({ baseUrl: 'http://10.0.0.2:8765', token: 'abc' });
    useServers.getState().applyPairs([PAIR]);
    useServers.getState().ensureActive(useServers.getState().servers.find((s) => s.ephemeral)!.id);
    useServers.getState().ensureOwnActive();
    expect(useServers.getState().activeId).toBe(dono.id);
  });

  test('remover a ativa nunca escolhe a entrada temporária como nova ativa', async () => {
    await useServers.getState().load();
    useServers.getState().applyPairs([PAIR]);
    const outra = { id: 's_b', label: 'b', baseUrl: 'http://10.0.0.3:8765', token: 'b' };
    const ativa = { id: 's_a', label: 'a', baseUrl: 'http://10.0.0.2:8765', token: 'a' };
    useServers.setState({ servers: [...useServers.getState().servers, outra, ativa], activeId: ativa.id });
    expect(useServers.getState().servers[0].ephemeral).toBe(true);
    useServers.getState().remove(ativa.id);
    expect(useServers.getState().activeId).toBe(outra.id);
    useServers.getState().remove(useServers.getState().servers.find((s) => s.ephemeral)!.id);
    expect(useServers.getState().activeId).toBe(outra.id);
  });

  test('máquinas próprias deixam a entrada temporária de fora', async () => {
    await useServers.getState().load();
    useServers.getState().add({ baseUrl: 'http://10.0.0.2:8765', token: 'abc' });
    useServers.getState().applyPairs([PAIR]);
    expect(ownServers(useServers.getState().servers).map((s) => s.baseUrl)).toEqual(['http://10.0.0.2:8765']);
  });
});

describe('importação só acrescenta', () => {
  const imported = { id: 'saved', label: 'fixture-imported', baseUrl: 'https://imported.test', token: 'imported-token' };

  test('preserva credenciais, flags e ativa e deduplica duas importações', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();
    const first = useServers.getState().appendImported([{ ...SAVED_SERVER, token: 'overwrite', label: 'overwrite', disabled: true }, imported]);
    const second = useServers.getState().appendImported([imported]);
    await settle();
    expect(first).toHaveLength(1);
    expect(first[0].id).not.toBe(SAVED_SERVER.id);
    expect(second).toEqual([]);
    expect(useServers.getState().servers).toEqual([SAVED_SERVER, first[0]]);
    expect(useServers.getState().activeId).toBe(SAVED_SERVER.id);
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual({ servers: [SAVED_SERVER, first[0]], activeId: SAVED_SERVER.id });
  });

  test('reaplica importação durante hidratação contra a lista recuperada sem sobrescrevê-la', async () => {
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);
    const loading = useServers.getState().load();
    useServers.getState().appendImported([{ ...SAVED_SERVER, token: 'overwrite' }, imported]);
    expect(SecureStore.setItemAsync).not.toHaveBeenCalled();
    read.resolve(JSON.stringify(SAVED));
    await loading;
    expect(useServers.getState().servers[0]).toEqual(SAVED_SERVER);
    expect(useServers.getState().servers).toHaveLength(2);
    expect(new Set(useServers.getState().servers.map((s) => s.id)).size).toBe(2);
    expect(useServers.getState().activeId).toBe(SAVED_SERVER.id);
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual({ servers: useServers.getState().servers, activeId: SAVED_SERVER.id });
  });

  test('primeira nova vira ativa quando não há ativa', async () => {
    await useServers.getState().load();
    useServers.getState().appendImported([imported]);
    expect(useServers.getState().active()?.baseUrl).toBe(imported.baseUrl);
  });
});

describe('update de máquina própria', () => {
  const serverWithIdentity = { ...SAVED_SERVER, lan: { url: '', id: SAVED_SERVER.lan.id } };

  test('trocar URL descarta a LAN antiga, preserva a identidade e não duplica na importação', async () => {
    const { baseOf, decidirRota } = await import('@hangar/core');
    const oldServer = { ...SAVED_SERVER, baseUrl: 'http://127.0.0.1:8765' };
    vault.set(VAULT_KEY, JSON.stringify({ servers: [oldServer], activeId: oldServer.id }));
    await useServers.getState().load();
    await decidirRota(useServers.getState().active()!);
    expect(baseOf(oldServer)).toBe(oldServer.baseUrl);

    useServers.getState().update(oldServer.id, { baseUrl: 'http://10.0.0.4:8765' });

    expect(baseOf(useServers.getState().active()!)).not.toBe(oldServer.baseUrl);
    expect(useServers.getState().active()!.lan).toEqual({ url: '', id: oldServer.lan!.id });
    await useServers.getState().load();
    expect(useServers.getState().active()).toEqual({ ...serverWithIdentity, baseUrl: 'http://10.0.0.4:8765' });
    expect(useServers.getState().appendImported([{ ...oldServer, id: 'imported-alias', baseUrl: 'https://owner.tail.ts.net' }])).toEqual([]);
    expect(useServers.getState().servers).toHaveLength(1);
    expect(useServers.getState().activeId).toBe(oldServer.id);
  });

  test.each([
    { label: 'Outro nome' },
    { baseUrl: 'http://127.0.0.1:8765' },
  ])('mantém rota e LAN sem mudança de URL: %j', async (patch) => {
    const { baseOf, decidirRota, rotaDecidida } = await import('@hangar/core');
    const oldServer = { ...SAVED_SERVER, baseUrl: 'http://127.0.0.1:8765' };
    vault.set(VAULT_KEY, JSON.stringify({ servers: [oldServer], activeId: oldServer.id }));
    await useServers.getState().load();
    await decidirRota(useServers.getState().active()!);

    useServers.getState().update(oldServer.id, patch);
    await useServers.getState().load();

    expect(rotaDecidida(oldServer.id)).toBe(true);
    expect(baseOf(useServers.getState().active()!)).toBe(oldServer.baseUrl);
    expect(useServers.getState().active()).toEqual({ ...oldServer, ...patch });
  });

  test('persiste a edição sem trocar id, token ou ativa e sobrevive ao load', async () => {
    const other = { ...SAVED_SERVER, id: 'other', token: 'other-token', baseUrl: 'http://10.0.0.3:8765' };
    vault.set(VAULT_KEY, JSON.stringify({ servers: [SAVED_SERVER, other], activeId: other.id }));
    await useServers.getState().load();
    const patch = { label: 'Máquina editada', baseUrl: 'http://10.0.0.4:8765', id: 'changed', token: 'changed', activeId: 'changed' };

    useServers.getState().update(SAVED_SERVER.id, patch);
    await settle();

    const expected = { servers: [{ ...serverWithIdentity, label: patch.label, baseUrl: patch.baseUrl }, other], activeId: other.id };
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual(expected);
    await useServers.getState().load();
    expect(useServers.getState()).toMatchObject(expected);
  });

  test.each([
    [{ label: 'Outro nome' }, { ...SAVED_SERVER, label: 'Outro nome' }],
    [{ baseUrl: 'http://10.0.0.4:8765' }, { ...serverWithIdentity, baseUrl: 'http://10.0.0.4:8765' }],
  ])('aceita edição parcial: %j', async (patch, expected) => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();

    useServers.getState().update(SAVED_SERVER.id, patch);
    await useServers.getState().load();

    expect(useServers.getState().servers).toEqual([expected]);
    expect(useServers.getState().activeId).toBe(SAVED_SERVER.id);
  });

  test('id inexistente não cria entrada nem troca a ativa', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();

    useServers.getState().update('missing', { label: 'Não criar', baseUrl: 'http://10.0.0.4:8765' });
    await useServers.getState().load();

    expect(useServers.getState()).toMatchObject(SAVED);
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual(SAVED);
  });

  test('não edita convite salvo nem temporário', async () => {
    const invite = { ...SAVED_SERVER, id: 'invite', label: 'Convite', token: 'guest-token', invite: true };
    const saved = { servers: [SAVED_SERVER, invite], activeId: SAVED_SERVER.id };
    vault.set(VAULT_KEY, JSON.stringify(saved));
    await useServers.getState().load();
    useServers.getState().applyPairs([PAIR]);
    const before = structuredClone(useServers.getState().servers);

    for (const server of before.filter((s) => s.invite)) {
      useServers.getState().update(server.id, { label: 'Não editar', baseUrl: 'http://10.0.0.4:8765' });
    }
    await useServers.getState().load();

    expect(useServers.getState().servers).toEqual(before);
    expect(useServers.getState().activeId).toBe(SAVED_SERVER.id);
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual(saved);
  });

  test('aguarda o cofre ilegível e reaplica a edição sem perder máquinas recuperadas', async () => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(new Error('bloqueado'));
    await useServers.getState().load();
    useServers.getState().limparAviso();
    const read = deferred<string>();
    vi.mocked(SecureStore.getItemAsync).mockReturnValueOnce(read.promise);

    useServers.getState().update(SAVED_SERVER.id, { label: 'Máquina recuperada' });
    await settle();

    expect(useServers.getState().vaultReadable).toBe(false);
    expect(SecureStore.setItemAsync).not.toHaveBeenCalled();
    expect(vault.get(VAULT_KEY)).toBe(JSON.stringify(SAVED));
    const unseen = { ...SAVED_SERVER, id: 'unseen', baseUrl: 'http://10.0.0.5:8765', token: 'unseen-token' };
    read.resolve(JSON.stringify({ servers: [SAVED_SERVER, unseen], activeId: unseen.id }));
    await useServers.getState().load();

    const expected = { servers: [{ ...SAVED_SERVER, label: 'Máquina recuperada' }, unseen], activeId: unseen.id };
    expect(useServers.getState()).toMatchObject({ ...expected, vaultReadable: true, aviso: null });
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual(expected);
    await useServers.getState().load();
    expect(useServers.getState()).toMatchObject(expected);
  });

  test.each([
    ['http://10.0.0.3:8765', 'http://10.0.0.3:8765'],
    ['http://10.0.0.3:8765///', 'http://10.0.0.3:8765'],
    ['http://10.0.0.3:8765', 'http://10.0.0.3:8765///'],
  ])('recusa URL de outra máquina sem alterar memória ou cofre: %s / %s', async (baseUrl, savedUrl) => {
    const other = { ...SAVED_SERVER, id: 'other', baseUrl: savedUrl, token: 'other-token' };
    const saved = { servers: [SAVED_SERVER, other], activeId: other.id };
    vault.set(VAULT_KEY, JSON.stringify(saved));
    await useServers.getState().load();

    expect(() => useServers.getState().update(SAVED_SERVER.id, { label: 'Não editar', baseUrl }))
      .toThrowError('machine_address_duplicate');

    expect(useServers.getState()).toMatchObject(saved);
    expect(vault.get(VAULT_KEY)).toBe(JSON.stringify(saved));
    await useServers.getState().load();
    expect(useServers.getState()).toMatchObject(saved);
    expect(vault.get(VAULT_KEY)).toBe(JSON.stringify(saved));
  });

  test.each(['', '/', '///'])('permite a própria URL normalizada sem trocar identidade ou LAN: %s', async (suffix) => {
    vault.set(VAULT_KEY, JSON.stringify(SAVED));
    await useServers.getState().load();

    useServers.getState().update(SAVED_SERVER.id, { label: 'Novo nome', baseUrl: `${SAVED_SERVER.baseUrl}${suffix}` });
    await useServers.getState().load();

    const expected = { servers: [{ ...SAVED_SERVER, label: 'Novo nome' }], activeId: SAVED_SERVER.id };
    expect(useServers.getState()).toMatchObject(expected);
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual(expected);
  });

  test('URL de convite não conflita nem funde as entradas', async () => {
    const invite = { ...SAVED_SERVER, id: 'invite', baseUrl: 'http://10.0.0.3:8765', token: 'guest-token', invite: true };
    vault.set(VAULT_KEY, JSON.stringify({ servers: [SAVED_SERVER, invite], activeId: SAVED_SERVER.id }));
    await useServers.getState().load();

    useServers.getState().update(SAVED_SERVER.id, { baseUrl: invite.baseUrl });
    await useServers.getState().load();

    const expected = { servers: [{ ...serverWithIdentity, baseUrl: invite.baseUrl }, invite], activeId: SAVED_SERVER.id };
    expect(useServers.getState()).toMatchObject(expected);
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual(expected);
  });

  test('recusa conhecida com cofre ilegível não fica pendente nem impede uma edição válida de recuperar', async () => {
    const other = { ...SAVED_SERVER, id: 'other', baseUrl: 'http://10.0.0.3:8765', token: 'other-token' };
    const saved = { servers: [SAVED_SERVER, other], activeId: other.id };
    vault.set(VAULT_KEY, JSON.stringify(saved));
    await useServers.getState().load();
    vi.mocked(SecureStore.getItemAsync).mockRejectedValueOnce(new Error('bloqueado'));
    await useServers.getState().load();

    expect(() => useServers.getState().update(SAVED_SERVER.id, { baseUrl: `${other.baseUrl}/` }))
      .toThrowError('machine_address_duplicate');
    expect(useServers.getState()).toMatchObject({ ...saved, vaultReadable: false });
    expect(vault.get(VAULT_KEY)).toBe(JSON.stringify(saved));

    useServers.getState().update(SAVED_SERVER.id, { label: 'Edição válida' });
    await useServers.getState().load();

    const expected = { servers: [{ ...SAVED_SERVER, label: 'Edição válida' }, other], activeId: other.id };
    expect(useServers.getState()).toMatchObject({ ...expected, vaultReadable: true, aviso: null });
    expect(JSON.parse(vault.get(VAULT_KEY)!)).toEqual(expected);
    await useServers.getState().load();
    expect(useServers.getState()).toMatchObject(expected);
  });
});
