import { describe, it, expect, vi } from 'vitest';
import { webcrypto } from 'node:crypto';
import { overwriteGetLocale as overwriteFront } from '../paraglide/runtime';
import { configureLocale } from '@hangar/core';
function overwriteGetLocale(fn: () => 'en' | 'pt') {
  overwriteFront(fn);
  configureLocale({ getLocale: fn });
}

// O import de './sync' puxa '@hangar/core' -> './auth', que roda migrate() no import-time e precisa de
// localStorage/document/window (mesmo stub do api.test.ts, antes do import DINAMICO — import
// estatico e hoisted e executaria antes destes stubs).
const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
(globalThis as any).document = { cookie: '' };
(globalThis as any).window = { location: { origin: 'https://app.test' } };

const { deriveKeys, encryptList, decryptList, register } = await import('./sync');

// Node 20+ exposes WebCrypto at globalThis.crypto; ensure it for the module under test.
if (!globalThis.crypto) (globalThis as any).crypto = webcrypto;

describe('sync crypto', () => {
  it('round-trips a server list through derive/encrypt/decrypt', async () => {
    const salt = btoa('0123456789abcdef');
    const { authHash, encKey } = await deriveKeys('hunter2', salt, 600000);
    expect(typeof authHash).toBe('string');
    expect(authHash.length).toBeGreaterThan(0);

    const servers = [{ id: 'a', label: 'casa', baseUrl: 'http://h:1', token: 't1' }];
    const blob = await encryptList(encKey, servers);
    expect(blob.iv).toBeTruthy();
    expect(blob.data).toBeTruthy();

    const out = await decryptList(encKey, blob);
    expect(out).toEqual(servers);
  });

  it('derives the same authHash for the same password+salt', async () => {
    const salt = btoa('0123456789abcdef');
    const a = await deriveKeys('pw', salt, 600000);
    const b = await deriveKeys('pw', salt, 600000);
    expect(a.authHash).toBe(b.authHash);
  });

  it('produces a different authHash for a different password', async () => {
    const salt = btoa('0123456789abcdef');
    const a = await deriveKeys('pw1', salt, 600000);
    const b = await deriveKeys('pw2', salt, 600000);
    expect(a.authHash).not.toBe(b.authHash);
  });
});

// Parecer task 10, bloqueador 1: register lia `(await r.json()).detail` cru — com o backend novo
// mandando dict {code, params, msg} (backend/app/mensagens.py), new Error(dict).message vira
// '[object Object]' na tela. Agora passa pelo MESMO errorDetail do api.ts (um parser so, o do
// endpoint migrado e o do sync nao divergem) e o texto sai legivel.
describe('register (erro da API de sync)', () => {
  it('detail em dict {code,params,msg} vira a mensagem traduzida, nunca [object Object]', async () => {
    overwriteGetLocale(() => 'pt');
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: { code: 'erro_bootstrap_invalido', params: {}, msg: 'bad bootstrap' } }), { status: 400 }),
    );
    await expect(register('u', 'p', 'b')).rejects.toThrow('bootstrap inválido');
  });

  it('detail em string (endpoint antigo) continua funcionando como hoje', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'register failed' }), { status: 400 }),
    );
    await expect(register('u', 'p', 'b')).rejects.toThrow('register failed');
  });
});
