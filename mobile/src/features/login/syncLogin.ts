import { comTeto, errorDetail, isLocalMachineUrl, normalizeMachineUrl, parseMachineImportPayload, registrarDiag, type Server } from '@hangar/core';
import { decryptJson, deriveKeys } from './syncCrypto';
import * as m from '../../paraglide/messages';

type Vault = { enc_blob: { iv: string; data: string } | null };

const request = (hub: string, path: string, init?: RequestInit) => fetch(`${hub}${path}`, {
  credentials: 'include', redirect: 'error', ...init, signal: comTeto(undefined, 8000),
  headers: { 'Content-Type': 'application/json' },
}).catch((cause: Error) => { throw new Error(`${m.falha_conexao()}: ${cause.message}`); });

// Proxy responde 5xx em HTML: o corpo não serve para a tela.
const failure = async (response: Response) =>
  new Error(response.status >= 500 ? `${m.falha_conexao()}: HTTP ${response.status}` : await errorDetail(response));

// O `authHash` vale como senha no hub: nunca sai em http, e o custo dele não fica a critério de quem responde.
const MIN_ITERATIONS = 600_000;
const MAX_ITERATIONS = 2_000_000;

function hubAddress(address: string): string {
  if (/^http:\/\//i.test(address)) throw new Error(m.mobile_sync_login_https());
  try { return normalizeMachineUrl(/^https:\/\//i.test(address) ? address : `https://${address}`); }
  catch { throw new Error(m.mobile_sync_login_unavailable()); }
}

async function readPrelogin(hub: string, user: string): Promise<{ salt: string; iterations: number }> {
  const response = await request(hub, `/api/sync/prelogin?user=${encodeURIComponent(user)}`);
  if (response.status >= 500) throw await failure(response);
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
  if (!login.ok) throw await failure(login);
  const read = await request(hub, '/api/sync/vault');
  if (!read.ok) throw await failure(read);
  const vault = await read.json().catch(() => null) as Vault | null;
  // O cookie do login dá escrita no cofre e só servia para esta leitura.
  await request(hub, '/api/sync/logout', { method: 'POST' }).catch(() => {});
  if (vault?.enc_blob === null) return [];
  try {
    // Endereço vazio ou de loopback foi gravado de dentro do próprio hub.
    const list = decryptJson<Server[]>(encKey, vault!.enc_blob).filter((server) => !server.disabled)
      .map((server) => ({ ...server, baseUrl: !server.baseUrl || isLocalMachineUrl(server.baseUrl) ? hub : server.baseUrl }));
    return parseMachineImportPayload(JSON.stringify(list));
  } catch {
    registrarDiag({ evento: 'sync.cofre_ilegivel', nivel: 'erro', tela: 'login', codigo: 'sync_vault_unreadable' });
    throw new Error(m.mobile_sync_login_vault());
  }
}
