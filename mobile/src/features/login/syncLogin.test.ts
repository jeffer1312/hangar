import { afterEach, beforeAll, describe, expect, test, vi } from 'vitest';
import { deriveKeys, toBase64 } from './syncCrypto';
import { syncLogin } from './syncLogin';
import * as m from '../../paraglide/messages';

const HUB = 'https://hub.test';
const PASSWORD = 'senha-certa';
const ITERATIONS = 600_000;
const salt = toBase64(new Uint8Array(16));
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

let authHash = '';
let vault: { iv: string; data: string };

beforeAll(async () => {
  const keys = await deriveKeys(PASSWORD, salt, ITERATIONS);
  authHash = keys.authHash;
  const key = await crypto.subtle.importKey('raw', keys.encKey as Uint8Array<ArrayBuffer>, 'AES-GCM', false, ['encrypt']);
  const iv = new Uint8Array(12);
  const list = [
    { id: 'a', label: 'Hub', baseUrl: '', token: 'ta' },
    { id: 'b', label: 'Casa', baseUrl: `${HUB}/casa/`, token: 'tb' },
    { id: 'c', label: 'Desligada', baseUrl: `${HUB}/off`, token: 'tc', disabled: true },
  ];
  const data = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode(JSON.stringify(list)));
  vault = { iv: toBase64(iv), data: toBase64(new Uint8Array(data)) };
});

function stubHub(iterations = ITERATIONS) {
  return vi.spyOn(globalThis, 'fetch').mockImplementation(async (url, init) => {
    const { pathname } = new URL(String(url));
    if (pathname === '/api/sync/prelogin') return json({ salt, iterations });
    if (pathname === '/api/sync/login') return JSON.parse(String(init?.body)).auth_hash === authHash ? json({ ok: true }) : json({}, 401);
    return json({ enc_blob: vault, rev: 1 });
  });
}

afterEach(() => { vi.restoreAllMocks(); });

describe('syncLogin', () => {
  test('traz as máquinas ligadas e leva o cookie do login à leitura do cofre', async () => {
    const hub = stubHub();
    const servers = await syncLogin(`${HUB}/`, 'ana', PASSWORD);
    expect(servers.map((s) => [s.label, s.baseUrl])).toEqual([['Hub', HUB], ['Casa', `${HUB}/casa`]]);
    expect(hub.mock.calls.map(([, init]) => init?.credentials)).toEqual(['include', 'include', 'include']);
  });

  test('senha errada para no login, sem ler o cofre', async () => {
    const hub = stubHub();
    await expect(syncLogin(HUB, 'ana', 'errada')).rejects.toThrow(m.sync_credenciais_invalidas());
    expect(hub).toHaveBeenCalledTimes(2);
  });

  test('http não chega a chamar o hub', async () => {
    const hub = stubHub();
    await expect(syncLogin('http://hub.test', 'ana', PASSWORD)).rejects.toThrow(m.mobile_sync_login_https());
    expect(hub).not.toHaveBeenCalled();
  });

  test.each([1, 2_000_000_000])('recusa %d iterações sem mandar a senha derivada', async (iterations) => {
    const hub = stubHub(iterations);
    await expect(syncLogin(HUB, 'ana', PASSWORD)).rejects.toThrow(m.mobile_sync_login_unavailable());
    expect(hub).toHaveBeenCalledTimes(1);
  });
});
