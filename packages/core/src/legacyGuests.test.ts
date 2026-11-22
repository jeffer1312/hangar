import { afterEach, describe, expect, it, vi } from 'vitest';
import { legacyGuestPlan, openLegacyGuests, type LegacyAdmin } from './legacyGuests';
import type { Server } from './servers';
import type { SealedBlob, SyncCrypto } from './syncLogin';

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
// A cifra fica de fora do core: a falsa guarda o JSON aberto em `data` e só aceita a senha certa do dono.
const crypto: SyncCrypto = {
  deriveKeys: async (password) => ({ authHash: password === 'senha-do-dono' ? 'auth-ok' : 'auth-errado', encKey: new Uint8Array(32) }),
  decryptJson: <T>(_key: Uint8Array, blob: SealedBlob) => JSON.parse(blob.data) as T,
};
const seal = (value: unknown): SealedBlob => ({ iv: 'x', data: JSON.stringify(value) });

const casa: Server = { id: 'casa', label: 'Casa', baseUrl: 'https://casa.tail.ts.net', token: 'owner-casa' };
const lia: LegacyAdmin = { user: 'lia', password: 'senha-da-lia', seesOwner: true, ownerSees: false,
  servers: [{ serverId: 'web-casa', guestId: 'g-old', token: 'tok-old', root: '/srv/lia' }] };

type Hit = { method: string; path: string; body: Record<string, unknown> | undefined };
function network(route: (hit: Hit) => Response | undefined = () => undefined) {
  const hits: Hit[] = [];
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const url = new URL(String(input));
    const hit: Hit = { method: init?.method ?? 'GET', path: `${url.pathname}${url.search}`,
      body: init?.body ? JSON.parse(String(init.body)) as Record<string, unknown> : undefined };
    hits.push(hit);
    const answer = route(hit);
    if (answer) return answer;
    if (url.pathname === '/api/sync/prelogin') return json({ salt: 'AAAA', iterations: 600_000 });
    if (url.pathname === '/api/sync/login') return hit.body?.auth_hash === 'auth-ok' ? json({ ok: true }) : json({}, 401);
    if (url.pathname === '/api/sync/vault') return json({ enc_blob: seal([{ id: 'web-casa', label: 'Casa', baseUrl: 'https://casa.tail.ts.net', token: 'x' }]) });
    if (url.pathname === '/api/sync/guests') return json([{ user: 'lia', admin_blob: seal(lia) }]);
    return json({ ok: true });
  }));
  return hits;
}
const paths = (hits: Hit[]) => hits.map((hit) => `${hit.method} ${hit.path.split('?')[0]}`);

afterEach(() => { vi.unstubAllGlobals(); });

describe('openLegacyGuests', () => {
  it('entra com a senha do dono, decifra as contas antigas e o cofre dele, e apaga e sai pelo mesmo login', async () => {
    const hits = network();
    const session = await openLegacyGuests(casa, 'dono', 'senha-do-dono', crypto);
    expect(session.hub).toBe('https://casa.tail.ts.net');
    expect(session.users).toEqual(['lia']);
    // A máquina do cofre do dono (`web-casa`) é a `casa` deste aparelho pelo endereço.
    expect(session.plan('lia', [casa])).toMatchObject({ draft: { user: 'lia', password: 'senha-da-lia', servers: [{ serverId: 'casa', root: '/srv/lia' }] },
      prev: { servers: [{ serverId: 'casa', guestId: 'g-old', token: 'tok-old' }] } });
    expect(Object.keys(session).sort()).toEqual(['close', 'hub', 'plan', 'remove', 'users']);
    expect(hits[0].path).toBe('/api/sync/prelogin?user=dono');
    expect(hits[1].body).toEqual({ user: 'dono', auth_hash: 'auth-ok' });
    await session.remove('lia');
    await session.close();
    expect(paths(hits).slice(-2)).toEqual(['POST /api/sync/guests/lia/delete', 'POST /api/sync/logout']);
  });

  it('conta antiga que já saiu conta como apagada; outra recusa sobe com o status', async () => {
    let status = 404;
    network((hit) => (hit.path.endsWith('/delete') ? json({}, status) : undefined));
    const session = await openLegacyGuests(casa, 'dono', 'senha-do-dono', crypto);
    await expect(session.remove('lia')).resolves.toBeUndefined();
    status = 500;
    await expect(session.remove('lia')).rejects.toMatchObject({ code: 'sync_request_failed', status: 500 });
  });

  it('senha errada é `credentials_mismatch`', async () => {
    network();
    await expect(openLegacyGuests(casa, 'dono', 'outra', crypto)).rejects.toMatchObject({ code: 'credentials_mismatch' });
  });

  it('endereço http é recusado antes de mandar a senha derivada', async () => {
    const hits = network();
    await expect(openLegacyGuests({ ...casa, baseUrl: 'http://192.168.0.2:8765' }, 'dono', 'senha-do-dono', crypto))
      .rejects.toMatchObject({ code: 'sync_insecure' });
    expect(hits).toEqual([]);
  });

  it('prefere o endereço do Connect da máquina', async () => {
    network();
    const session = await openLegacyGuests({ ...casa, baseUrl: 'http://192.168.0.2:8765', connect: { url: 'https://casa.ana.hangar.dev.br' } } as Server,
      'dono', 'senha-do-dono', crypto);
    expect(session.hub).toBe('https://casa.ana.hangar.dev.br');
  });

  it('leitura recusada ou ilegível sai do login antes de devolver o erro', async () => {
    let hits = network((hit) => (hit.path === '/api/sync/guests' ? json({}, 503) : undefined));
    await expect(openLegacyGuests(casa, 'dono', 'senha-do-dono', crypto)).rejects.toMatchObject({ code: 'sync_request_failed', status: 503 });
    expect(paths(hits).at(-1)).toBe('POST /api/sync/logout');

    hits = network((hit) => (hit.path === '/api/sync/guests' ? json([{ user: 'lia', admin_blob: { iv: 'x', data: 'não é json' } }]) : undefined));
    await expect(openLegacyGuests(casa, 'dono', 'senha-do-dono', crypto)).rejects.toMatchObject({ code: 'sync_vault_unreadable' });
    expect(paths(hits).at(-1)).toBe('POST /api/sync/logout');
  });

  it.each([
    { user: 'lia' },
    { ...lia, password: undefined },
    { ...lia, seesOwner: 'sim' },
    { ...lia, servers: undefined },
    { ...lia, servers: [{ serverId: 'web-casa', guestId: 'g-old', root: '/srv/lia' }] },
    { ...lia, user: '' },
  ])('conta antiga fora da forma é `sync_vault_unreadable` e sai do login: %j', async (admin) => {
    const hits = network((hit) => (hit.path === '/api/sync/guests' ? json([{ user: 'lia', admin_blob: seal(admin) }]) : undefined));
    await expect(openLegacyGuests(casa, 'dono', 'senha-do-dono', crypto)).rejects.toMatchObject({ code: 'sync_vault_unreadable' });
    expect(paths(hits).at(-1)).toBe('POST /api/sync/logout');
  });

  it('`close` zera a chave do cofre', async () => {
    network();
    const key = new Uint8Array(32).fill(7);
    const session = await openLegacyGuests(casa, 'dono', 'senha-do-dono', { ...crypto, deriveKeys: async () => ({ authHash: 'auth-ok', encKey: key }) });
    await session.close();
    expect([...key].every((byte) => byte === 0)).toBe(true);
  });

  it('conta que não está na lista não tem plano', async () => {
    network();
    const session = await openLegacyGuests(casa, 'dono', 'senha-do-dono', crypto);
    let error: unknown;
    try { session.plan('outra', [casa]); } catch (thrown) { error = thrown; }
    expect(error).toMatchObject({ code: 'sync_vault_unreadable' });
  });
});

describe('legacyGuestPlan', () => {
  const owner: Server[] = [{ id: 'web-casa', label: 'Casa', baseUrl: 'https://casa.tail.ts.net', token: 'x' }];

  it('acha a máquina deste aparelho pelo endereço e reaproveita o acesso que já existe', () => {
    const plan = legacyGuestPlan({ ...lia, user: 'Lia.Costa' }, owner, [casa]);
    expect(plan).toEqual({
      draft: { user: 'lia.costa', password: 'senha-da-lia', seesOwner: true, ownerSees: false, servers: [{ serverId: 'casa', root: '/srv/lia' }] },
      prev: { user: 'lia.costa', seesOwner: true, ownerSees: false,
        servers: [{ serverId: 'casa', label: 'Casa', guestId: 'g-old', root: '/srv/lia', token: 'tok-old' }] },
    });
  });

  it('nome novo vale na conta nova, com a mesma senha', () => {
    const plan = legacyGuestPlan(lia, owner, [casa], ' Lia@Dono ');
    expect('draft' in plan && [plan.draft.user, plan.prev.user, plan.draft.password]).toEqual(['lia@dono', 'lia@dono', 'senha-da-lia']);
  });

  it('máquina que não está neste aparelho volta pelo nome', () => {
    expect(legacyGuestPlan(lia, owner, [])).toEqual({ missing: ['Casa'] });
  });
});
