import { afterEach, beforeAll, describe, expect, test, vi } from 'vitest';
import { deriveKeys, toBase64 } from './syncCrypto';
import { syncLogin } from './syncLogin';
import * as m from '../../paraglide/messages';

const HUB = 'https://hub.test';
const PASSWORD = 'senha-certa';
const ITERATIONS = 600_000;
const salt = toBase64(new Uint8Array(16));
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

type Blob = { iv: string; data: string };
let authHash = '';
let vault: Blob;
let loopback: Blob;

beforeAll(async () => {
  const keys = await deriveKeys(PASSWORD, salt, ITERATIONS);
  authHash = keys.authHash;
  const key = await crypto.subtle.importKey('raw', keys.encKey as Uint8Array<ArrayBuffer>, 'AES-GCM', false, ['encrypt']);
  const seal = async (list: unknown): Promise<Blob> => {
    const iv = new Uint8Array(12);
    const data = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode(JSON.stringify(list)));
    return { iv: toBase64(iv), data: toBase64(new Uint8Array(data)) };
  };
  vault = await seal([
    { id: 'a', label: 'Hub', baseUrl: '', token: 'ta' },
    { id: 'b', label: 'Casa', baseUrl: `${HUB}/casa/`, token: 'tb' },
    { id: 'c', label: 'Desligada', baseUrl: 'endereço torto', token: 'tc', disabled: true },
  ]);
  loopback = await seal([{ id: 'a', label: 'Hub', baseUrl: 'http://127.0.0.1:8765', token: 'ta' }]);
});

function stubHub({ iterations = ITERATIONS, blob = () => vault as Blob | null } = {}) {
  return vi.spyOn(globalThis, 'fetch').mockImplementation(async (url, init) => {
    const { pathname } = new URL(String(url));
    if (pathname === '/api/sync/prelogin') return json({ salt, iterations });
    if (pathname === '/api/sync/login') return JSON.parse(String(init?.body)).auth_hash === authHash ? json({ ok: true }) : json({}, 401);
    if (pathname === '/api/sync/logout') return json({ ok: true });
    return json({ enc_blob: blob(), rev: 1 });
  });
}

const paths = (hub: ReturnType<typeof stubHub>) => hub.mock.calls.map(([url]) => new URL(String(url)).pathname);

afterEach(() => { vi.restoreAllMocks(); });

describe('syncLogin', () => {
  test('traz as máquinas ligadas, leva o cookie à leitura do cofre e encerra a sessão', async () => {
    const hub = stubHub();
    const servers = await syncLogin(`${HUB}/`, 'ana', PASSWORD);
    expect(servers.map((s) => [s.label, s.baseUrl])).toEqual([['Hub', HUB], ['Casa', `${HUB}/casa`]]);
    expect(paths(hub)).toEqual(['/api/sync/prelogin', '/api/sync/login', '/api/sync/vault', '/api/sync/logout']);
    expect(hub.mock.calls.every(([, init]) => init?.credentials === 'include' && init.redirect === 'error')).toBe(true);
  });

  test('endereço de loopback no cofre vira o próprio hub', async () => {
    stubHub({ blob: () => loopback });
    expect((await syncLogin(HUB, 'ana', PASSWORD)).map((s) => s.baseUrl)).toEqual([HUB]);
  });

  test('cofre vazio não é erro', async () => {
    stubHub({ blob: () => null });
    expect(await syncLogin(HUB, 'ana', PASSWORD)).toEqual([]);
  });

  test('cofre que não abre diz isso, não que algo foi ignorado', async () => {
    stubHub({ blob: () => ({ iv: vault.iv, data: 'AAAA' }) });
    await expect(syncLogin(HUB, 'ana', PASSWORD)).rejects.toThrow(m.mobile_sync_login_vault());
  });

  test('senha errada para no login, sem ler o cofre', async () => {
    const hub = stubHub();
    await expect(syncLogin(HUB, 'ana', 'errada')).rejects.toThrow(m.sync_credenciais_invalidas());
    expect(paths(hub)).toEqual(['/api/sync/prelogin', '/api/sync/login']);
  });

  test('http não chega a chamar o hub', async () => {
    const hub = stubHub();
    await expect(syncLogin('http://hub.test', 'ana', PASSWORD)).rejects.toThrow(m.mobile_sync_login_https());
    expect(hub).not.toHaveBeenCalled();
  });

  test.each([1, 2_000_000_000])('recusa %d iterações sem mandar a senha derivada', async (iterations) => {
    const hub = stubHub({ iterations });
    await expect(syncLogin(HUB, 'ana', PASSWORD)).rejects.toThrow(m.mobile_sync_login_unavailable());
    expect(hub).toHaveBeenCalledTimes(1);
  });

  test('5xx de proxy não joga o HTML na tela', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('<html>bad gateway</html>', { status: 502 }));
    await expect(syncLogin(HUB, 'ana', PASSWORD)).rejects.toThrow(`${m.falha_conexao()}: HTTP 502`);
  });
});
