import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { listGuests, removeGuest, saveGuest, type GuestAccount, type GuestDraft, type GuestMachine } from './guests';
import { _resetRotasForTests } from './rota';
import type { Server } from './servers';

const casa: Server = { id: 'a', label: 'Casa', baseUrl: 'https://casa.tail.ts.net', token: 'owner-a' };
const pc: Server = { id: 'b', label: 'PC', baseUrl: 'https://pc.lan', token: 'owner-b' };
const vps: Server = { id: 'c', label: 'VPS', baseUrl: 'https://vps.example', token: 'owner-c' };
const machines: GuestMachine[] = [
  { server: casa, connectUrl: 'https://casa.ana.hangar.dev.br' },
  { server: pc },
  { server: vps },
];

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const refusal = (status: number, code: string) => json({ detail: { code, params: {}, msg: code } }, status);

type Hit = { host: string; method: string; path: string; body: Record<string, unknown> | undefined; auth: string | null };
type Route = (hit: Hit) => Response | undefined;
// Cada máquina responde como o servidor: o acesso novo ganha id e token com o nome dela.
function network(route: Route = () => undefined) {
  const hits: Hit[] = [];
  vi.stubGlobal('fetch', vi.fn(async (input: string, init?: RequestInit) => {
    const url = new URL(String(input));
    const hit: Hit = {
      host: url.hostname, method: init?.method ?? 'GET', path: url.pathname,
      body: init?.body ? JSON.parse(String(init.body)) as Record<string, unknown> : undefined,
      auth: new Headers(init?.headers).get('Authorization'),
    };
    hits.push(hit);
    const answer = route(hit);
    if (answer) return answer;
    const name = hit.host.split('.')[0];
    if (hit.path === '/api/guests') return json({ id: `g-${name}`, token: `t-${name}` });
    if (/^\/api\/guests\/[^/]+\/delete$/.test(hit.path)) return json({ ok: true });
    if (/^\/api\/guests\/[^/]+$/.test(hit.path)) return json({ id: hit.path.split('/')[3] });
    return json({ ok: true });
  }));
  return hits;
}
const line = (hit: Hit) => `${hit.method} ${hit.host}${hit.path}`;
const stored = (hits: Hit[]) => hits.find((hit) => hit.method === 'POST' && hit.path === '/api/guest-accounts')?.body;

const draft = (over: Partial<GuestDraft> = {}): GuestDraft => ({
  user: 'lhais@casa', password: 'senha-da-lhais', seesOwner: false, ownerSees: true,
  servers: [{ serverId: 'a', root: '/home/ana/lhais' }, { serverId: 'b', root: '' }], ...over,
});
const existing: GuestAccount = {
  user: 'lhais@casa', seesOwner: false, ownerSees: true,
  servers: [
    { serverId: 'a', label: 'Casa', guestId: 'g-old-a', root: '/home/ana/lhais', baseUrl: 'https://casa.ana.hangar.dev.br' },
    { serverId: 'b', label: 'PC', guestId: 'g-old-b', root: '' },
  ],
};

beforeEach(() => { _resetRotasForTests(); });
afterEach(() => { vi.unstubAllGlobals(); });

describe('listGuests', () => {
  it('lê as contas na máquina que as guarda, com a credencial do dono', async () => {
    const hits = network(() => json([{
      user: 'lhais@casa', sees_owner: true, owner_sees: false,
      servers: [{ server_id: 'a', label: 'Casa', guest_id: 'g1', root: '/r', base_url: 'https://casa.ana.hangar.dev.br' }],
    }]));
    expect(await listGuests(casa)).toEqual([{
      user: 'lhais@casa', seesOwner: true, ownerSees: false,
      servers: [{ serverId: 'a', label: 'Casa', guestId: 'g1', root: '/r', baseUrl: 'https://casa.ana.hangar.dev.br' }],
    }]);
    expect(hits.map(line)).toEqual(['GET casa.tail.ts.net/api/guest-accounts']);
    expect(hits[0].auth).toBe('Bearer owner-a');
  });
});

describe('saveGuest', () => {
  it('cria o acesso em cada máquina antes de gravar a conta, com o host do Connect quando há', async () => {
    const hits = network();
    const { saved, errors, storeFailed } = await saveGuest(casa, machines, draft(), null);
    expect(errors).toEqual([]);
    expect(storeFailed).toBe(false);
    expect(hits.map(line)).toEqual(['POST casa.tail.ts.net/api/guests', 'POST pc.lan/api/guests', 'POST casa.tail.ts.net/api/guest-accounts']);
    expect(hits[0].body).toEqual({ name: 'lhais@casa', root: '/home/ana/lhais', sees_owner: false, owner_sees: true });
    expect(hits[1].auth).toBe('Bearer owner-b');
    expect(stored(hits)).toEqual({
      user: 'lhais@casa', password: 'senha-da-lhais', sees_owner: false, owner_sees: true,
      servers: [
        { server_id: 'a', label: 'Casa', guest_id: 'g-casa', root: '/home/ana/lhais', base_url: 'https://casa.ana.hangar.dev.br', token: 't-casa' },
        { server_id: 'b', label: 'PC', guest_id: 'g-pc', root: '', base_url: 'https://pc.lan', token: 't-pc' },
      ],
    });
    expect(saved.servers.map((s) => s.guestId)).toEqual(['g-casa', 'g-pc']);
  });

  it('nome em minúsculas ASCII; sem `.` nem `@` é recusado antes de tocar nas máquinas', async () => {
    const hits = network();
    await saveGuest(casa, machines, draft({ user: ' Lhais@Casa ' }), null);
    expect(hits[0].body?.name).toBe('lhais@casa');
    expect(stored(hits)?.user).toBe('lhais@casa');

    const none = network();
    await expect(saveGuest(casa, machines, draft({ user: 'lhais' }), null)).rejects.toMatchObject({ code: 'guest_user_invalid' });
    await expect(saveGuest(casa, machines, draft({ password: undefined }), null)).rejects.toMatchObject({ code: 'guest_password_required' });
    expect(none).toHaveLength(0);
  });

  it('falha numa máquina volta com o nome dela, e as outras seguem', async () => {
    const hits = network((hit) => (hit.host === 'pc.lan' ? refusal(500, 'erro_guests_store') : undefined));
    const { saved, errors, storeFailed } = await saveGuest(casa, machines, draft(), null);
    expect(errors).toEqual([expect.objectContaining({ label: 'PC', code: 'erro_guests_store', status: 500 })]);
    expect(storeFailed).toBe(false);
    expect(saved.servers.map((s) => s.serverId)).toEqual(['a']);
    expect((stored(hits)?.servers as unknown[]).length).toBe(1);
  });

  it('edição altera, tira e cria por máquina, sem senha e sem token do que já existia', async () => {
    const hits = network();
    const { errors } = await saveGuest(casa, machines,
      draft({ password: undefined, servers: [{ serverId: 'a', root: '/novo' }, { serverId: 'c', root: '' }] }), existing);
    expect(errors).toEqual([]);
    expect(hits.map(line)).toEqual([
      'POST casa.tail.ts.net/api/guests/g-old-a', 'POST vps.example/api/guests', 'POST pc.lan/api/guests/g-old-b/delete',
      'POST casa.tail.ts.net/api/guest-accounts']);
    expect(hits[0].body).toEqual({ root: '/novo', sees_owner: false, owner_sees: true });
    expect(stored(hits)).toEqual({
      user: 'lhais@casa', sees_owner: false, owner_sees: true,
      servers: [
        { server_id: 'a', label: 'Casa', guest_id: 'g-old-a', root: '/novo', base_url: 'https://casa.ana.hangar.dev.br' },
        { server_id: 'c', label: 'VPS', guest_id: 'g-vps', root: '', base_url: 'https://vps.example', token: 't-vps' },
      ],
    });
  });

  it('acesso que sumiu na máquina é criado de novo', async () => {
    const hits = network((hit) => (hit.path === '/api/guests/g-old-b' ? refusal(404, 'erro_convidado_inexistente') : undefined));
    const { saved, errors } = await saveGuest(casa, machines, draft({ password: undefined }), existing);
    expect(errors).toEqual([]);
    expect(hits.map(line)).toContain('POST pc.lan/api/guests');
    expect(saved.servers.find((s) => s.serverId === 'b')).toMatchObject({ guestId: 'g-pc', token: 't-pc' });
  });

  it('máquina fora da lista fica na conta e volta com o id dela', async () => {
    const hits = network();
    const { saved, errors } = await saveGuest(casa, [machines[1]], draft({ password: undefined }), existing);
    expect(errors).toEqual([{ label: 'a', code: 'guest_server_missing', message: '' }]);
    expect(saved.servers.find((s) => s.serverId === 'a')).toEqual(existing.servers[0]);
    expect(hits.some((hit) => hit.host === 'casa.tail.ts.net' && hit.path.startsWith('/api/guests'))).toBe(false);
  });

  it('gravação recusada devolve o que já mudou, e repetir com ele leva o token do acesso criado', async () => {
    let refuse = true;
    network((hit) => (hit.path === '/api/guest-accounts' && refuse ? refusal(503, 'erro_guest_accounts') : undefined));
    const first = await saveGuest(casa, machines, draft(), null);
    expect(first.storeFailed).toBe(true);
    expect(first.errors).toEqual([expect.objectContaining({ label: 'Casa', code: 'guest_store_failed', status: 503 })]);

    refuse = false;
    const hits = network();
    const again = await saveGuest(casa, machines, draft(), first.saved);
    expect(again.storeFailed).toBe(false);
    // Repetir altera o acesso que já existe, sem criar outro.
    expect(hits.filter((hit) => hit.path === '/api/guests')).toHaveLength(0);
    expect((stored(hits)?.servers as { token?: string }[]).map((s) => s.token)).toEqual(['t-casa', 't-pc']);
  });
});

describe('removeGuest', () => {
  it('tira o acesso de todas as máquinas e depois apaga a conta', async () => {
    const hits = network();
    expect(await removeGuest(casa, machines, existing)).toEqual([]);
    expect(hits.map(line)).toEqual([
      'POST casa.tail.ts.net/api/guests/g-old-a/delete', 'POST pc.lan/api/guests/g-old-b/delete',
      'POST casa.tail.ts.net/api/guest-accounts', 'POST casa.tail.ts.net/api/guest-accounts/lhais%40casa/delete']);
    expect(stored(hits)?.servers).toEqual([]);
  });

  it('máquina que não largou o acesso segura a conta', async () => {
    const hits = network((hit) => (hit.host === 'pc.lan' ? refusal(500, 'erro_guests_store') : undefined));
    expect(await removeGuest(casa, machines, existing)).toEqual([expect.objectContaining({ label: 'PC', code: 'erro_guests_store' })]);
    expect(hits.some((hit) => hit.path.endsWith('/guest-accounts/lhais%40casa/delete'))).toBe(false);
    expect((stored(hits)?.servers as { server_id: string }[]).map((s) => s.server_id)).toEqual(['b']);
  });

  it('acesso que já tinha saído da máquina conta como tirado', async () => {
    network((hit) => (hit.path.endsWith('/delete') && hit.host === 'pc.lan' ? refusal(404, 'erro_convidado_inexistente') : undefined));
    expect(await removeGuest(casa, machines, existing)).toEqual([]);
  });
});
