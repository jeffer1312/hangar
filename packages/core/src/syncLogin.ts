// Login de sincronização de convidado (`/api/sync/*`) numa máquina: só lê o cofre e devolve as máquinas dele.
// A cripto vem de quem chama: o Hermes não tem `crypto.subtle`, e o core não leva biblioteca de cifra.
import { comTeto } from './api';
import { registrar as registrarDiag } from './diag';
import { isLocalMachineUrl, normalizeMachineUrl, parseMachineImportPayload } from './machineImport';
import type { Server } from './servers';

export type SealedBlob = { iv: string; data: string };

/** Par do `deriveKeys` de frontend/src/lib/sync.ts (PBKDF2-SHA256 + HKDF `cp-auth`/`cp-enc`) e do AES-GCM do cofre. */
export interface SyncCrypto {
  deriveKeys(password: string, salt: string, iterations: number): Promise<{ authHash: string; encKey: Uint8Array }>;
  decryptJson<T>(encKey: Uint8Array, blob: SealedBlob): T;
}

const REQUEST_TIMEOUT_MS = 8000;
// O `authHash` vale como senha na máquina: nunca sai em http, e o custo dele não fica a critério de quem responde.
const MIN_ITERATIONS = 600_000;
const MAX_ITERATIONS = 2_000_000;

const fail = (code: string, status?: number) => Object.assign(new Error(code), { code, status });

async function request(hub: string, path: string, init?: RequestInit): Promise<Response> {
  try {
    return await fetch(`${hub}${path}`, {
      credentials: 'include', redirect: 'error', ...init, signal: comTeto(undefined, REQUEST_TIMEOUT_MS),
      headers: { 'Content-Type': 'application/json' },
    });
  } catch {
    throw fail('network_failed');
  }
}

function hubAddress(address: string): string {
  if (/^http:\/\//i.test(address)) throw fail('sync_insecure');
  try { return normalizeMachineUrl(/^https:\/\//i.test(address) ? address : `https://${address}`); }
  catch { throw fail('sync_unavailable'); }
}

async function readPrelogin(hub: string, user: string): Promise<{ salt: string; iterations: number }> {
  const response = await request(hub, `/api/sync/prelogin?user=${encodeURIComponent(user)}`);
  const body: unknown = response.ok ? await response.json().catch(() => null) : null;
  const { salt, iterations } = (body ?? {}) as { salt?: unknown; iterations?: unknown };
  if (typeof salt !== 'string' || typeof iterations !== 'number' || !Number.isInteger(iterations)
    || iterations < MIN_ITERATIONS || iterations > MAX_ITERATIONS) throw fail('sync_unavailable', response.status);
  return { salt, iterations };
}

/** Erros com `code`: `credentials_mismatch`, `sync_too_many_attempts`, `network_failed`, `sync_insecure`,
 *  `sync_unavailable`, `sync_request_failed` e `sync_vault_unreadable`. */
export async function syncSignIn(address: string, user: string, password: string, crypto: SyncCrypto): Promise<Server[]> {
  const hub = hubAddress(address);
  const { salt, iterations } = await readPrelogin(hub, user);
  const { authHash, encKey } = await crypto.deriveKeys(password, salt, iterations);
  const login = await request(hub, '/api/sync/login', { method: 'POST', body: JSON.stringify({ user, auth_hash: authHash }) });
  if (login.status === 429) throw fail('sync_too_many_attempts', 429);
  if (login.status === 401) throw fail('credentials_mismatch', 401);
  if (!login.ok) throw fail('sync_request_failed', login.status);
  const read = await request(hub, '/api/sync/vault');
  // O cookie do login dá escrita no cofre e só servia para esta leitura.
  const logout = () => request(hub, '/api/sync/logout', { method: 'POST' }).catch(() => {});
  if (!read.ok) {
    await logout();
    throw fail('sync_request_failed', read.status);
  }
  const vault = await read.json().catch(() => null) as { enc_blob?: SealedBlob | null } | null;
  await logout();
  if (vault?.enc_blob === null) return [];
  try {
    // Endereço vazio ou de loopback foi gravado de dentro da própria máquina.
    const list = crypto.decryptJson<Server[]>(encKey, vault!.enc_blob!).filter((server) => !server.disabled)
      .map((server) => ({ ...server, baseUrl: !server.baseUrl || isLocalMachineUrl(server.baseUrl) ? hub : server.baseUrl }));
    return parseMachineImportPayload(JSON.stringify(list));
  } catch {
    registrarDiag({ evento: 'sync.cofre_ilegivel', nivel: 'erro', codigo: 'sync_vault_unreadable' }, hub);
    throw fail('sync_vault_unreadable');
  }
}
