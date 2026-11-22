import { afterEach, describe, expect, it, vi } from 'vitest';
import { configureDiag } from './diag';
import { openSyncSession, syncSignIn, type SealedBlob, type SyncCrypto } from './syncLogin';

const HUB = 'https://casa.ana.hangar.dev.br';
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const crypto: SyncCrypto = {
  deriveKeys: async () => ({ authHash: 'auth-ok', encKey: new Uint8Array(32) }),
  decryptJson: <T>(_key: Uint8Array, blob: SealedBlob) => JSON.parse(blob.data) as T,
};

type Hit = { method: string; path: string; headers: Headers };
function network(vault: unknown) {
  const hits: Hit[] = [];
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const url = new URL(String(input));
    hits.push({ method: init?.method ?? 'GET', path: url.pathname, headers: new Headers(init?.headers) });
    if (url.pathname === '/api/sync/prelogin') return json({ salt: 'AAAA', iterations: 600_000 });
    if (url.pathname === '/api/sync/vault') return json(vault);
    return json({ ok: true });
  }));
  return hits;
}

afterEach(() => { vi.unstubAllGlobals(); });

describe('syncSignIn', () => {
  it.each([{}, { enc_blob: '' }, null])('resposta sem cofre não vira lista vazia, vai ao diário e sai do login: %j', async (vault) => {
    const events: { codigo?: string }[] = [];
    configureDiag({ registrar: (event) => { events.push(event); }, novoReq: () => '' });
    const hits = network(vault);
    await expect(syncSignIn(HUB, 'lhais@casa', 'senha', crypto)).rejects.toMatchObject({ code: 'sync_vault_missing' });
    expect(hits.map((hit) => `${hit.method} ${hit.path}`)).toContain('POST /api/sync/logout');
    expect(events.map((event) => event.codigo)).toEqual(['sync_vault_missing']);
  });

  it('cofre `null` é a conta sem máquinas', async () => {
    network({ enc_blob: null });
    await expect(syncSignIn(HUB, 'lhais@casa', 'senha', crypto)).resolves.toEqual([]);
  });
});

describe('openSyncSession', () => {
  it('`call` mantém os cabeçalhos de quem chama, junto do Content-Type', async () => {
    const hits = network({ enc_blob: null });
    const session = await openSyncSession(HUB, 'dono', 'senha', crypto);
    await session.call('/api/sync/guests', { headers: { 'X-Fixture': 'sim' } });
    await session.call('/api/sync/guests', { headers: { 'Content-Type': 'text/plain' } });
    const [first, second] = hits.filter((hit) => hit.path === '/api/sync/guests');
    expect([first.headers.get('X-Fixture'), first.headers.get('Content-Type')]).toEqual(['sim', 'application/json']);
    expect(second.headers.get('Content-Type')).toBe('text/plain');
  });
});
