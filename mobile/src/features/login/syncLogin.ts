import { comTeto, errorDetail, normalizeMachineUrl, parseMachineImportPayload, type Server } from '@hangar/core';
import { decryptJson, deriveKeys } from './syncCrypto';
import * as m from '../../paraglide/messages';

type Vault = { enc_blob: { iv: string; data: string } | null };

const request = (hub: string, path: string, init?: RequestInit) => fetch(`${hub}${path}`, {
  credentials: 'include', ...init, signal: comTeto(undefined, 8000),
  headers: { 'Content-Type': 'application/json' },
}).catch((cause: Error) => { throw new Error(`${m.falha_conexao()}: ${cause.message}`); });

// O `authHash` vale como senha no hub: nunca sai em http, e o custo dele não fica a critério de quem responde.
const MIN_ITERATIONS = 600_000;
const MAX_ITERATIONS = 10_000_000;

function hubAddress(address: string): string {
  if (/^http:\/\//i.test(address)) throw new Error(m.mobile_sync_login_https());
  try { return normalizeMachineUrl(/^https:\/\//i.test(address) ? address : `https://${address}`); }
  catch { throw new Error(m.mobile_sync_login_unavailable()); }
}

async function readPrelogin(hub: string, user: string): Promise<{ salt: string; iterations: number }> {
  const response = await request(hub, `/api/sync/prelogin?user=${encodeURIComponent(user)}`);
  if (response.status >= 500) throw new Error(await errorDetail(response));
  const body: unknown = response.ok ? await response.json().catch(() => null) : null;
  const { salt, iterations } = (body ?? {}) as { salt?: unknown; iterations?: unknown };
  if (typeof salt !== 'string' || typeof iterations !== 'number' || !Number.isInteger(iterations)
    || iterations < MIN_ITERATIONS || iterations > MAX_ITERATIONS) throw new Error(m.mobile_sync_login_unavailable());
  return { salt, iterations };
}

/** Só lê o cofre: a lista daqui nunca sobe, e a conta volta a ser lida entrando de novo. */
export async function syncLogin(address: string, user: string, password: string): Promise<Server[]> {
  const hub = hubAddress(address);
  const { salt, iterations } = await readPrelogin(hub, user);
  const { authHash, encKey } = await deriveKeys(password, salt, iterations);
  const login = await request(hub, '/api/sync/login', { method: 'POST', body: JSON.stringify({ user, auth_hash: authHash }) });
  if (login.status === 429) throw new Error(m.sync_muitas_tentativas());
  if (login.status === 401) throw new Error(m.sync_credenciais_invalidas());
  if (!login.ok) throw new Error(await errorDetail(login));
  const read = await request(hub, '/api/sync/vault');
  if (!read.ok) throw new Error(await errorDetail(read));
  try {
    const { enc_blob } = await read.json() as Vault;
    if (!enc_blob) return [];
    // Endereço vazio no cofre é o próprio hub.
    const list = decryptJson<Server[]>(encKey, enc_blob).map((server) => ({ ...server, baseUrl: server.baseUrl || hub }));
    return parseMachineImportPayload(JSON.stringify(list)).filter((server) => !server.disabled);
  } catch { throw new Error(m.machine_import_invalid_item()); }
}
