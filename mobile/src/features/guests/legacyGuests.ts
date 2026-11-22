// Conta de convidado do cofre de sincronização antigo (`/api/sync/guests`): só o dono logado na sincronização da
// máquina lê, e o `admin_blob` de cada uma (cifrado com a chave do dono) guarda a senha dela e os acessos.
import { comTeto, normalizeGuestUser, normalizeMachineUrl, type GuestAccount, type GuestDraft, type Server } from '@hangar/core';
import { syncCrypto } from '../login/syncCrypto';

type Sealed = { iv: string; data: string };
/** Formato do web (`frontend/src/lib/guests.ts`): `serverId` é o id da lista de servidores do cofre do dono. */
export type LegacyAdmin = {
  user: string; password: string; seesOwner: boolean; ownerSees: boolean;
  servers: { serverId: string; guestId: string; token: string; root: string }[];
};
export type LegacySession = { hub: string; ownerServers: Server[]; guests: LegacyAdmin[] };

const MIN_ITERATIONS = 600_000;
const MAX_ITERATIONS = 2_000_000;
const fail = (code: string, status?: number) => Object.assign(new Error(code), { code, status });

async function call(hub: string, path: string, init?: RequestInit): Promise<Response> {
  try {
    return await fetch(`${hub}${path}`, {
      credentials: 'include', redirect: 'error', ...init, signal: comTeto(undefined, 15_000),
      headers: { 'Content-Type': 'application/json' },
    });
  } catch {
    throw fail('network_failed');
  }
}

// A senha derivada vale como senha do dono: nunca sai em http.
function hubOf(server: Server): string {
  const address = server.connect?.url || server.baseUrl;
  if (!/^https:\/\//i.test(address)) throw fail('sync_insecure');
  return normalizeMachineUrl(address);
}

/** Entra na sincronização do dono nesta máquina e lê as contas antigas. Erros com `code` como os do `syncSignIn`. */
export async function openLegacy(server: Server, user: string, password: string): Promise<LegacySession> {
  const hub = hubOf(server);
  const pre = await call(hub, `/api/sync/prelogin?user=${encodeURIComponent(user)}`);
  const { salt, iterations } = ((pre.ok ? await pre.json().catch(() => null) : null) ?? {}) as { salt?: unknown; iterations?: unknown };
  if (typeof salt !== 'string' || typeof iterations !== 'number' || !Number.isInteger(iterations)
    || iterations < MIN_ITERATIONS || iterations > MAX_ITERATIONS) throw fail('sync_unavailable', pre.status);
  const keys = await syncCrypto.deriveKeys(password, salt, iterations);
  const login = await call(hub, '/api/sync/login', { method: 'POST', body: JSON.stringify({ user, auth_hash: keys.authHash }) });
  if (login.status === 429) throw fail('sync_too_many_attempts', 429);
  if (login.status === 401) throw fail('credentials_mismatch', 401);
  if (!login.ok) throw fail('sync_request_failed', login.status);
  try {
    const vault = await call(hub, '/api/sync/vault');
    const rows = await call(hub, '/api/sync/guests');
    if (!vault.ok) throw fail('sync_request_failed', vault.status);
    if (!rows.ok) throw fail('sync_request_failed', rows.status);
    const blob = (await vault.json() as { enc_blob?: Sealed | null }).enc_blob;
    const list = await rows.json() as { user: string; admin_blob: Sealed }[];
    try {
      const ownerServers = blob ? syncCrypto.decryptJson<Server[]>(keys.encKey, blob) : [];
      return { hub, ownerServers, guests: list.map((row) => syncCrypto.decryptJson<LegacyAdmin>(keys.encKey, row.admin_blob)) };
    } catch {
      throw fail('sync_vault_unreadable');
    }
  } catch (error) {
    await closeLegacy(hub);
    throw error;
  }
}

/** O cookie do login escreve no cofre do dono: sai assim que a conta antiga foi trazida ou a folha fechou. */
export async function closeLegacy(hub: string): Promise<void> {
  await call(hub, '/api/sync/logout', { method: 'POST' }).catch(() => {});
}

export async function deleteLegacy(hub: string, user: string): Promise<void> {
  const response = await call(hub, `/api/sync/guests/${encodeURIComponent(user)}/delete`, { method: 'POST' });
  if (!response.ok && response.status !== 404) throw fail('sync_request_failed', response.status);
}

const hostOf = (url?: string) => {
  try { return url ? new URL(url).hostname : ''; } catch { return ''; }
};

/**
 * Conta nova com a mesma senha e os mesmos acessos: `prev` aponta os acessos que já existem em cada máquina, então
 * nada é criado de novo. A máquina do cofre do dono é achada na lista deste aparelho pelo id ou pelo endereço.
 */
export function legacyPlan(admin: LegacyAdmin, ownerServers: Server[], mine: Server[]):
  { draft: GuestDraft; prev: GuestAccount } | { missing: string[] } {
  const missing: string[] = [];
  const found = admin.servers.flatMap((entry) => {
    const owner = ownerServers.find((server) => server.id === entry.serverId);
    const hosts = [hostOf(owner?.baseUrl), hostOf(owner?.connect?.url)].filter(Boolean);
    const local = mine.find((server) => server.id === entry.serverId)
      ?? mine.find((server) => [server.baseUrl, server.connect?.url, server.lan?.url].some((url) => hosts.includes(hostOf(url))));
    if (!local) {
      missing.push(owner?.label || entry.serverId);
      return [];
    }
    return [{ entry, local }];
  });
  if (missing.length) return { missing };
  const user = normalizeGuestUser(admin.user);
  return {
    draft: { user, password: admin.password, seesOwner: admin.seesOwner, ownerSees: admin.ownerSees,
      servers: found.map(({ entry, local }) => ({ serverId: local.id, root: entry.root })) },
    prev: { user, seesOwner: admin.seesOwner, ownerSees: admin.ownerSees,
      servers: found.map(({ entry, local }) => ({ serverId: local.id, label: local.label, guestId: entry.guestId, root: entry.root, token: entry.token })) },
  };
}
