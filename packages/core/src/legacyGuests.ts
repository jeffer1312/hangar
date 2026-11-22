// Conta de convidado do cofre de sincronização antigo (`/api/sync/guests`): só o dono logado na sincronização da
// máquina lê, e o `admin_blob` de cada uma (cifrado com a chave do dono) guarda a senha dela e os acessos.
import { registrar as registrarDiag } from './diag';
import { normalizeGuestUser, type GuestAccount, type GuestDraft } from './guests';
import { normalizeMachineUrl } from './machineImport';
import { openSyncSession, type SealedBlob, type SyncCrypto } from './syncLogin';
import type { Server } from './servers';

/** Formato do web (`frontend/src/lib/guests.ts`): `serverId` é o id da lista de servidores do cofre do dono. */
export type LegacyAdmin = {
  user: string; password: string; seesOwner: boolean; ownerSees: boolean;
  servers: { serverId: string; guestId: string; token: string; root: string }[];
};
/** Máquina do cofre do dono, sem o token: o plano só precisa achá-la neste aparelho. */
export type LegacyOwnerServer = Pick<Server, 'id' | 'label' | 'baseUrl' | 'connect'>;
type Plan = { draft: GuestDraft; prev: GuestAccount } | { missing: string[] };

/**
 * Contas antigas lidas com o login de sincronização do dono, que fica aberto até `close`. Senha e acessos de cada
 * conta continuam cifrados e só são abertos no `plan` da conta escolhida.
 */
export type LegacySession = {
  hub: string;
  users: string[];
  /** Contas que não abriram (cifra ou forma) e ficaram fora de `users`; já registradas no diário. */
  skipped: number;
  /** `legacyGuestPlan` da conta `user`. Erro com `code` `sync_vault_unreadable` se ela não abrir. */
  plan(user: string, mine: Server[], name?: string): Plan;
  /** Apaga a conta antiga; a que já tinha saído (404) conta como apagada. */
  remove(user: string): Promise<void>;
  /** Sai do login e zera a chave do cofre. */
  close(): Promise<void>;
};

const fail = (code: string, status?: number) => Object.assign(new Error(code), { code, status });

// A senha derivada vale como senha do dono: nunca sai em http.
function hubOf(server: Server): string {
  const address = server.connect?.url || server.baseUrl;
  if (!/^https:\/\//i.test(address)) throw fail('sync_insecure');
  return normalizeMachineUrl(address);
}

const text = (value: unknown): value is string => typeof value === 'string';

function isLegacyAdmin(value: unknown): value is LegacyAdmin {
  const admin = (value ?? {}) as Partial<Record<keyof LegacyAdmin, unknown>>;
  return text(admin.user) && !!admin.user && text(admin.password) && typeof admin.seesOwner === 'boolean'
    && typeof admin.ownerSees === 'boolean' && Array.isArray(admin.servers)
    && admin.servers.every((entry: unknown) => {
      const { serverId, guestId, token, root } = (entry ?? {}) as Record<string, unknown>;
      return text(serverId) && text(guestId) && text(token) && text(root);
    });
}

/** Entra na sincronização do dono em `server` e lê as contas antigas. Erros com `code` como os do `syncSignIn`. */
export async function openLegacyGuests(server: Server, user: string, password: string, crypto: SyncCrypto): Promise<LegacySession> {
  const session = await openSyncSession(hubOf(server), user, password, crypto);
  const close = async () => {
    session.encKey.fill(0);
    await session.close();
  };
  try {
    const vault = await session.call('/api/sync/vault');
    const rows = await session.call('/api/sync/guests');
    if (!vault.ok) throw fail('sync_request_failed', vault.status);
    if (!rows.ok) throw fail('sync_request_failed', rows.status);
    let ownerServers: LegacyOwnerServer[];
    let list: { admin_blob: SealedBlob }[];
    const admin = (blob: SealedBlob) => {
      const opened: unknown = crypto.decryptJson<unknown>(session.encKey, blob);
      if (!isLegacyAdmin(opened)) throw fail('sync_vault_unreadable');
      return opened;
    };
    try {
      const blob = (await vault.json() as { enc_blob?: SealedBlob | null }).enc_blob;
      const body: unknown = await rows.json();
      if (!Array.isArray(body)) throw fail('sync_vault_unreadable');
      list = body as { admin_blob: SealedBlob }[];
      ownerServers = (blob ? crypto.decryptJson<Server[]>(session.encKey, blob) : [])
        .map(({ id, label, baseUrl, connect }) => ({ id, label, baseUrl, connect }));
    } catch {
      throw fail('sync_vault_unreadable');
    }
    // Cada conta é aberta uma vez para conferir a forma e o nome; o resto volta a ficar só cifrado. A que não abre
    // fica de fora sem levar as outras junto.
    const sealed = new Map<string, SealedBlob>();
    let skipped = 0;
    for (const row of list) {
      try { sealed.set(admin(row?.admin_blob).user, row.admin_blob); }
      catch {
        skipped += 1;
        registrarDiag({ evento: 'guests.legacy_unreadable', nivel: 'aviso', codigo: 'legacy_guest_unreadable' }, session.hub);
      }
    }
    return {
      hub: session.hub, users: [...sealed.keys()], skipped, close,
      plan(guest, mine, name) {
        const blob = sealed.get(guest);
        if (!blob) throw fail('sync_vault_unreadable');
        let opened: LegacyAdmin;
        try { opened = admin(blob); } catch { throw fail('sync_vault_unreadable'); }
        return legacyGuestPlan(opened, ownerServers, mine, name);
      },
      async remove(guest) {
        const response = await session.call(`/api/sync/guests/${encodeURIComponent(guest)}/delete`, { method: 'POST' });
        if (!response.ok && response.status !== 404) throw fail('sync_request_failed', response.status);
      },
    };
  } catch (error) {
    await close();
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
export function legacyGuestPlan(admin: LegacyAdmin, ownerServers: LegacyOwnerServer[], mine: Server[], user: string = admin.user): Plan {
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
