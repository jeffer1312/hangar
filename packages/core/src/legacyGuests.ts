// Conta de convidado do cofre de sincronização antigo (`/api/sync/guests`): só o dono logado na sincronização da
// máquina lê, e o `admin_blob` de cada uma (cifrado com a chave do dono) guarda a senha dela e os acessos.
import { normalizeGuestUser, type GuestAccount, type GuestDraft } from './guests';
import { normalizeMachineUrl } from './machineImport';
import { openSyncSession, type SealedBlob, type SyncCrypto } from './syncLogin';
import type { Server } from './servers';

/** Formato do web (`frontend/src/lib/guests.ts`): `serverId` é o id da lista de servidores do cofre do dono. */
export type LegacyAdmin = {
  user: string; password: string; seesOwner: boolean; ownerSees: boolean;
  servers: { serverId: string; guestId: string; token: string; root: string }[];
};
/** Contas antigas lidas com o login de sincronização do dono, que fica aberto até `close`. */
export type LegacySession = {
  hub: string;
  ownerServers: Server[];
  guests: LegacyAdmin[];
  /** Apaga a conta antiga; a que já tinha saído (404) conta como apagada. */
  remove(user: string): Promise<void>;
  close(): Promise<void>;
};

const fail = (code: string, status?: number) => Object.assign(new Error(code), { code, status });

// A senha derivada vale como senha do dono: nunca sai em http.
function hubOf(server: Server): string {
  const address = server.connect?.url || server.baseUrl;
  if (!/^https:\/\//i.test(address)) throw fail('sync_insecure');
  return normalizeMachineUrl(address);
}

/** Entra na sincronização do dono em `server` e lê as contas antigas. Erros com `code` como os do `syncSignIn`. */
export async function openLegacyGuests(server: Server, user: string, password: string, crypto: SyncCrypto): Promise<LegacySession> {
  const session = await openSyncSession(hubOf(server), user, password, crypto);
  try {
    const vault = await session.call('/api/sync/vault');
    const rows = await session.call('/api/sync/guests');
    if (!vault.ok) throw fail('sync_request_failed', vault.status);
    if (!rows.ok) throw fail('sync_request_failed', rows.status);
    let ownerServers: Server[];
    let guests: LegacyAdmin[];
    try {
      const blob = (await vault.json() as { enc_blob?: SealedBlob | null }).enc_blob;
      const list = await rows.json() as { user: string; admin_blob: SealedBlob }[];
      ownerServers = blob ? crypto.decryptJson<Server[]>(session.encKey, blob) : [];
      guests = list.map((row) => crypto.decryptJson<LegacyAdmin>(session.encKey, row.admin_blob));
    } catch {
      throw fail('sync_vault_unreadable');
    }
    return {
      hub: session.hub, ownerServers, guests, close: session.close,
      async remove(guest) {
        const response = await session.call(`/api/sync/guests/${encodeURIComponent(guest)}/delete`, { method: 'POST' });
        if (!response.ok && response.status !== 404) throw fail('sync_request_failed', response.status);
      },
    };
  } catch (error) {
    await session.close();
    throw error;
  }
}

const hostOf = (url?: string) => {
  try { return url ? new URL(url).hostname : ''; } catch { return ''; }
};

/**
 * Conta nova com a mesma senha e os mesmos acessos: `prev` aponta os acessos que já existem em cada máquina, então
 * nada é criado de novo. A máquina do cofre do dono é achada na lista deste aparelho pelo id ou pelo endereço.
 * `user`: nome novo, quando o antigo não serve no Connect.
 */
export function legacyGuestPlan(admin: LegacyAdmin, ownerServers: Server[], mine: Server[], user: string = admin.user):
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
  const name = normalizeGuestUser(user);
  return {
    draft: { user: name, password: admin.password, seesOwner: admin.seesOwner, ownerSees: admin.ownerSees,
      servers: found.map(({ entry, local }) => ({ serverId: local.id, root: entry.root })) },
    prev: { user: name, seesOwner: admin.seesOwner, ownerSees: admin.ownerSees,
      servers: found.map(({ entry, local }) => ({ serverId: local.id, label: local.label, guestId: entry.guestId, root: entry.root, token: entry.token })) },
  };
}
