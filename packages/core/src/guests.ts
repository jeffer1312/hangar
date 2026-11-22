// Contas de convidado guardadas numa máquina do dono (`/api/guest-accounts`, só com a credencial do dono). O acesso
// em cada máquina marcada é criado antes de gravar a conta, com a credencial que o dono já tem para ela; a máquina
// que guarda a conta deriva as chaves e cifra o cofre, então o dono não digita senha nenhuma além da dela.
import { apiFetchForServer, createGuestForServer, deleteGuestForServer, updateGuestForServer } from './api';
import type { Server } from './servers';

/** `token` só existe no acesso criado e ainda não gravado na conta (o GET não o devolve): repetir com `saved` o leva. */
export type GuestServer = { serverId: string; label: string; guestId: string; root: string; baseUrl?: string; token?: string };
/** `nameTaken`: o serviço do Connect disse que o nome já é de outra conta; só vem da leitura. */
export type GuestAccount = { user: string; seesOwner: boolean; ownerSees: boolean; servers: GuestServer[]; nameTaken?: boolean };
/** `password` só é obrigatória ao criar; ausente na edição mantém a senha. */
export type GuestDraft = { user: string; password?: string; seesOwner: boolean; ownerSees: boolean; servers: { serverId: string; root: string }[] };
/** Máquina que pode receber o convidado. `connectUrl`: host do Connect dela, que vai ao cofre no lugar do endereço salvo. */
export type GuestMachine = { server: Server; connectUrl?: string };
/** `code`: `guest_server_missing` (máquina fora da lista), `guest_store_failed` (gravar a conta) ou o código do servidor. */
export type GuestFailure = { label: string; code: string; message: string; status?: number };

type WireServer = { server_id: string; label: string; guest_id: string; root: string; base_url?: string; token?: string };
type WireAccount = { user: string; sees_owner: boolean; owner_sees: boolean; name_taken?: boolean; servers: WireServer[] };

const fail = (code: string) => Object.assign(new Error(code), { code });
// Mesma regra do servidor (`valid_user` em hangar-guest-accounts): minúsculas, com `.` ou `@`, que nome de conta do
// Connect não aceita.
export const normalizeGuestUser = (user: string) => user.trim().replace(/[A-Z]/g, (c) => c.toLowerCase());
export const validGuestUser = (user: string) => /^[a-z0-9._@+-]{3,64}$/.test(user) && /[.@]/.test(user);

/** Nome que serve no Connect para a conta antiga `old`: o mesmo, se já serve; senão o antigo com `@<dono>`. */
export function suggestGuestUser(old: string, owner: string): string {
  const user = normalizeGuestUser(old);
  if (validGuestUser(user)) return user;
  const clean = (text: string) => normalizeGuestUser(text).replace(/[^a-z0-9._+-]/g, '');
  const tail = (clean(owner) || 'hangar').slice(0, 32);
  return `${(clean(user) || 'convidado').slice(0, 63 - tail.length)}@${tail}`;
}

const failure = (label: string, error: unknown): GuestFailure => {
  const { code, status } = (error ?? {}) as { code?: unknown; status?: unknown };
  return {
    label, code: typeof code === 'string' ? code : 'guest_server_failed',
    message: error instanceof Error ? error.message : String(error), ...(typeof status === 'number' ? { status } : {}),
  };
};
const gone = (e: unknown) => (e as { status?: number } | null)?.status === 404;

export async function listGuests(store: Server): Promise<GuestAccount[]> {
  const rows = await apiFetchForServer<WireAccount[]>(store, '/api/guest-accounts');
  return rows.map((row) => ({
    user: row.user, seesOwner: row.sees_owner, ownerSees: row.owner_sees, nameTaken: row.name_taken === true,
    servers: row.servers.map((s) => ({
      serverId: s.server_id, label: s.label, guestId: s.guest_id, root: s.root, ...(s.base_url ? { baseUrl: s.base_url } : {}),
    })),
  }));
}

/**
 * Cria, altera e tira o acesso em cada máquina e só depois grava a conta em `store`. Falha numa máquina volta com o
 * nome dela, e a entrada antiga fica na conta para tentar de novo. `storeFailed`: as máquinas já mudaram, então quem
 * chama repete com `saved` como `prev` para não criar o acesso duas vezes.
 */
export async function saveGuest(
  store: Server, machines: GuestMachine[], draft: GuestDraft, prev: GuestAccount | null,
): Promise<{ saved: GuestAccount; errors: GuestFailure[]; storeFailed: boolean }> {
  const user = normalizeGuestUser(draft.user);
  // Antes de tocar nas máquinas: recusa depois deixaria acesso criado sem conta.
  if (!validGuestUser(user)) throw fail('guest_user_invalid');
  if (!prev && !draft.password) throw fail('guest_password_required');
  const byId = new Map(machines.map((m) => [m.server.id, m]));
  const before = new Map((prev?.servers ?? []).map((s) => [s.serverId, s]));
  const settings = { sees_owner: draft.seesOwner, owner_sees: draft.ownerSees };
  const kept: GuestServer[] = [];
  const errors: GuestFailure[] = [];
  const entry = (machine: GuestMachine, guestId: string, root: string): GuestServer => ({
    serverId: machine.server.id, label: machine.server.label, guestId, root, baseUrl: machine.connectUrl || machine.server.baseUrl,
  });
  const create = async (machine: GuestMachine, root: string) => {
    const r = await createGuestForServer(machine.server, { name: user, root, ...settings });
    kept.push({ ...entry(machine, r.id, root), token: r.token });
  };
  const missing = (serverId: string) => errors.push({ label: serverId, code: 'guest_server_missing', message: '' });

  for (const want of draft.servers) {
    const machine = byId.get(want.serverId);
    const old = before.get(want.serverId);
    if (!machine) {
      missing(want.serverId);
      if (old) kept.push(old);
      continue;
    }
    try {
      if (!old) {
        await create(machine, want.root);
        continue;
      }
      try {
        await updateGuestForServer(machine.server, old.guestId, { root: want.root, ...settings });
        kept.push({ ...entry(machine, old.guestId, want.root), ...(old.token ? { token: old.token } : {}) });
      } catch (e) {
        if (!gone(e)) throw e;
        // Sumiu na máquina: recria em vez de ficar com um id morto.
        await create(machine, want.root);
      }
    } catch (e) {
      errors.push(failure(machine.server.label, e));
      if (old) kept.push(old);
    }
  }
  for (const old of prev?.servers ?? []) {
    if (draft.servers.some((w) => w.serverId === old.serverId)) continue;
    const machine = byId.get(old.serverId);
    if (!machine) {
      missing(old.serverId);
      kept.push(old);
      continue;
    }
    try {
      await deleteGuestForServer(machine.server, old.guestId);
    } catch (e) {
      if (gone(e)) continue;
      // Fica na conta para poder tirar o acesso de novo.
      errors.push(failure(machine.server.label, e));
      kept.push(old);
    }
  }

  const saved: GuestAccount = { user, seesOwner: draft.seesOwner, ownerSees: draft.ownerSees, servers: kept };
  // Sem `token`, a máquina que guarda a conta mantém o que já tinha para aquele acesso.
  const body = {
    user, ...(draft.password ? { password: draft.password } : {}), ...settings,
    servers: kept.map((s) => ({
      server_id: s.serverId, label: s.label, guest_id: s.guestId, root: s.root,
      ...(s.baseUrl ? { base_url: s.baseUrl } : {}), ...(s.token ? { token: s.token } : {}),
    })),
  };
  try {
    await apiFetchForServer(store, '/api/guest-accounts', { method: 'POST', body: JSON.stringify(body) });
  } catch (e) {
    errors.push({ ...failure(store.label, e), code: 'guest_store_failed' });
    return { saved, errors, storeFailed: true };
  }
  return { saved, errors, storeFailed: false };
}

/**
 * Desfaz os acessos de uma conta que a máquina não gravou (nome recusado), sem escrever nela. Volta uma falha por
 * máquina que ficou com o acesso; o que já tinha saído (404) conta como tirado.
 */
export async function dropGuestAccesses(machines: GuestMachine[], guest: GuestAccount): Promise<GuestFailure[]> {
  const byId = new Map(machines.map((m) => [m.server.id, m]));
  const errors: GuestFailure[] = [];
  for (const access of guest.servers) {
    const machine = byId.get(access.serverId);
    if (!machine) {
      errors.push({ label: access.serverId, code: 'guest_server_missing', message: '' });
      continue;
    }
    try {
      await deleteGuestForServer(machine.server, access.guestId);
    } catch (e) {
      if (!gone(e)) errors.push(failure(machine.server.label, e));
    }
  }
  return errors;
}

/** Tira o acesso de todas as máquinas e só apaga a conta quando todas largaram e a gravação foi aceita. */
export async function removeGuest(store: Server, machines: GuestMachine[], guest: GuestAccount): Promise<GuestFailure[]> {
  const { saved, errors, storeFailed } = await saveGuest(store, machines, { ...guest, servers: [] }, guest);
  if (saved.servers.length > 0 || storeFailed) return errors;
  try {
    await apiFetchForServer(store, `/api/guest-accounts/${encodeURIComponent(saved.user)}/delete`, { method: 'POST' });
  } catch (e) {
    errors.push({ ...failure(store.label, e), code: 'guest_store_failed' });
  }
  return errors;
}
