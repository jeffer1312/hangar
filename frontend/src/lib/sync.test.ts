import { describe, it, expect } from 'vitest';
import { webcrypto } from 'node:crypto';
import { deriveKeys, encryptList, decryptList } from './sync';

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
