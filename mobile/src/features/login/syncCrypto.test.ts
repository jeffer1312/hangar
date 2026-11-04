import { describe, expect, test } from 'vitest';
import { decryptJson, deriveKeys, fromBase64, toBase64 } from './syncCrypto';

const encoder = new TextEncoder();
const empty = new Uint8Array(0);

// A mesma conta de frontend/src/lib/sync.ts, com o Web Crypto de verdade.
async function webCryptoKeys(password: string, salt: Uint8Array<ArrayBuffer>, iterations: number) {
  const base = await crypto.subtle.importKey('raw', encoder.encode(password), 'PBKDF2', false, ['deriveBits']);
  const bits = await crypto.subtle.deriveBits({ name: 'PBKDF2', salt, iterations, hash: 'SHA-256' }, base, 256);
  const master = await crypto.subtle.importKey('raw', bits, 'HKDF', false, ['deriveBits', 'deriveKey']);
  const auth = await crypto.subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt: empty, info: encoder.encode('cp-auth') }, master, 256);
  const encKey = await crypto.subtle.deriveKey({ name: 'HKDF', hash: 'SHA-256', salt: empty, info: encoder.encode('cp-enc') },
    master, { name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']);
  return { authHash: Buffer.from(auth).toString('base64'), encKey };
}

describe('syncCrypto', () => {
  test('base64 igual ao do Node em todos os restos', () => {
    for (const size of [0, 1, 2, 3, 16, 32, 33]) {
      const bytes = Uint8Array.from({ length: size }, (_, i) => (i * 37 + 250) % 256);
      const text = Buffer.from(bytes).toString('base64');
      expect(toBase64(bytes)).toBe(text);
      expect(fromBase64(text)).toEqual(bytes);
    }
  });

  test('deriva as chaves do Web Crypto e abre o cofre que ele cifrou', async () => {
    const salt = Uint8Array.from({ length: 16 }, (_, i) => i * 11);
    const password = 'senha çom açento 🔑';
    const web = await webCryptoKeys(password, salt, 1000);
    const mine = await deriveKeys(password, toBase64(salt), 1000);
    expect(mine.authHash).toBe(web.authHash);

    const iv = Uint8Array.from({ length: 12 }, (_, i) => i + 1);
    const list = [{ id: 'a', label: 'Máquina', baseUrl: '', token: 't' }];
    const data = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, web.encKey, encoder.encode(JSON.stringify(list)));
    expect(decryptJson(mine.encKey, { iv: toBase64(iv), data: Buffer.from(data).toString('base64') })).toEqual(list);
  });

  test('senha errada não abre o cofre', async () => {
    const salt = toBase64(new Uint8Array(16));
    const right = await webCryptoKeys('certa', new Uint8Array(16), 1000);
    const wrong = await deriveKeys('errada', salt, 1000);
    const iv = new Uint8Array(12);
    const data = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, right.encKey, encoder.encode('[]'));
    expect(() => decryptJson(wrong.encKey, { iv: toBase64(iv), data: Buffer.from(data).toString('base64') })).toThrow();
  });
});
