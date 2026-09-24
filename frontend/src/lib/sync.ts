import * as m from '../paraglide/messages';
import { errorDetail, registrarDiag, novoReqDiag, comTeto, getSyncSetupForServer, setupSyncForServer, disableSyncForServer } from '@hangar/core';
import type { SyncSetup } from '@hangar/core';
import { listAllServers, type Server } from './auth';

export type { SyncSetup } from '@hangar/core';
export const getSyncSetup = getSyncSetupForServer;

// Zero-knowledge: the password never leaves the browser. From PBKDF2(masterKey) we split two HKDF
// branches — authHash (sent to the hub) and encKey (stays here, encrypts the server list).
const PBKDF2_ITERATIONS = 600000; // MUST match the backend PBKDF2_ITERATIONS
const enc = new TextEncoder();
const dec = new TextDecoder();

function b64(buf: ArrayBuffer): string {
  return btoa(String.fromCharCode(...new Uint8Array(buf)));
}
function unb64(s: string): Uint8Array<ArrayBuffer> {
  return new Uint8Array(Array.from(atob(s), (c) => c.charCodeAt(0)));
}

export async function deriveKeys(
  password: string,
  saltB64: string,
  iterations: number,
): Promise<{ authHash: string; encKey: CryptoKey }> {
  const salt = unb64(saltB64);
  const baseKey = await crypto.subtle.importKey('raw', enc.encode(password), 'PBKDF2', false, ['deriveBits']);
  const masterBits = await crypto.subtle.deriveBits(
    { name: 'PBKDF2', salt, iterations, hash: 'SHA-256' }, baseKey, 256,
  );
  const masterKey = await crypto.subtle.importKey('raw', masterBits, 'HKDF', false, ['deriveBits', 'deriveKey']);
  const authBits = await crypto.subtle.deriveBits(
    { name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(0), info: enc.encode('cp-auth') }, masterKey, 256,
  );
  const encKey = await crypto.subtle.deriveKey(
    { name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(0), info: enc.encode('cp-enc') },
    masterKey, { name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt'],  // extractable: persistir entre reloads
  );
  return { authHash: b64(authBits), encKey };
}

// Persiste a encKey no localStorage pra a sessao de sync sobreviver a fechar/reabrir o app (senao
// pediria login toda vez). Zero-knowledge protege o HUB (servidor), nao este device: a lista de
// servers ja fica no localStorage com os tokens em texto puro (cp_servers), entao a chave aqui nao
// expoe nada novo localmente. Some so no logout (clearKey) ou ao limpar os dados do site.
const KEY_STASH = 'cp_enckey';
export async function stashKey(encKey: CryptoKey): Promise<void> {
  const raw = await crypto.subtle.exportKey('raw', encKey);
  localStorage.setItem(KEY_STASH, b64(raw));
}
export async function loadKey(): Promise<CryptoKey | null> {
  const s = localStorage.getItem(KEY_STASH);
  if (!s) return null;
  try {
    return await crypto.subtle.importKey('raw', unb64(s), { name: 'AES-GCM' }, true, ['encrypt', 'decrypt']);
  } catch {
    return null;
  }
}
export function clearKey(): void {
  localStorage.removeItem(KEY_STASH);
}

export async function encryptList(encKey: CryptoKey, servers: Server[]): Promise<{ iv: string; data: string }> {
  const iv = new Uint8Array(12);
  crypto.getRandomValues(iv);
  const pt = enc.encode(JSON.stringify(servers));
  const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, encKey, pt); // ct includes the GCM tag
  return { iv: b64(iv.buffer), data: b64(ct) };
}

export async function decryptList(encKey: CryptoKey, blob: { iv: string; data: string }): Promise<Server[]> {
  const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: unb64(blob.iv) }, encKey, unb64(blob.data));
  return JSON.parse(dec.decode(pt));
}

// ── API client (same-origin; the front's reverse proxy forwards /api to the co-located backend) ──
async function jf(path: string, init?: RequestInit): Promise<Response> {
  const inicio = Date.now();
  const req = novoReqDiag();
  const detalhe = `${init?.method ?? 'GET'} ${path.split('?')[0]}`;
  const destino = window.location.origin;
  try {
    const resposta = await fetch(path, { credentials: 'include', ...init,
      signal: comTeto(init?.signal ?? undefined, 8000),
      headers: { 'Content-Type': 'application/json', 'X-Hangar-Req': req, ...init?.headers } });
    if (init?.method || !resposta.ok) registrarDiag({ evento: 'sync.pedido', req, detalhe,
      nivel: resposta.ok ? 'ok' : 'aviso', codigo: String(resposta.status), ms: Date.now() - inicio }, destino);
    return resposta;
  } catch (e) {
    registrarDiag({ evento: 'sync.falhou', nivel: 'erro', req, detalhe, codigo: 'rede',
      ms: Date.now() - inicio }, destino);
    throw e;
  }
}

type SyncStatus = { enabled: boolean; registered: boolean };
const STATUS_KEY = 'cp_sync_status';
let statusEmVoo: Promise<SyncStatus | null> | null = null;

export function cachedSyncStatus(): SyncStatus | null {
  try {
    const value = JSON.parse(localStorage.getItem(STATUS_KEY) ?? 'null');
    return typeof value?.enabled === 'boolean' && typeof value?.registered === 'boolean' ? value : null;
  } catch { return null; }
}

function rememberStatus(value: SyncStatus) {
  try { localStorage.setItem(STATUS_KEY, JSON.stringify(value)); } catch { /* Vale só nesta abertura. */ }
}

export function syncStatus(): Promise<SyncStatus | null> {
  return statusEmVoo ??= readStatus().finally(() => { statusEmVoo = null; });
}

async function readStatus(): Promise<SyncStatus | null> {
  try {
    const r = await jf('/api/sync/status');
    const cached = cachedSyncStatus();
    if (r.status === 404) {
      const value = { enabled: false, registered: false };
      rememberStatus(value);
      return value;
    }
    if (!r.ok) return cached?.enabled ? cached : null;
    const value = await r.json();
    if (typeof value?.enabled !== 'boolean' || typeof value?.registered !== 'boolean') {
      return cached?.enabled ? cached : null;
    }
    rememberStatus(value);
    return value;
  } catch {
    const cached = cachedSyncStatus();
    return cached?.enabled ? cached : null;
  }
}

export async function activateSync(server: Server, credentials?: { user: string; password: string }): Promise<SyncSetup> {
  if (!credentials) {
    const result = await setupSyncForServer(server);
    if (sameOrigin(server)) rememberStatus(result);
    return result;
  }
  if (credentials.password.length < 8) throw new Error(m.sync_password_min());
  const salt = b64(crypto.getRandomValues(new Uint8Array(16)).buffer);
  const { authHash, encKey } = await deriveKeys(credentials.password, salt, PBKDF2_ITERATIONS);
  const result = await setupSyncForServer(server, {
    user: credentials.user.trim(), salt, auth_hash: authHash,
    enc_blob: await encryptList(encKey, listAllServers()),
  });
  if (sameOrigin(server)) {
    rememberStatus(result);
    try {
      const r = await jf('/api/sync/login', {
        method: 'POST', body: JSON.stringify({ user: credentials.user.trim(), auth_hash: authHash }),
      });
      if (!r.ok) throw new Error('login');
      await stashKey(encKey);
    } catch { throw new Error(m.sync_setup_login_error()); }
  }
  return result;
}

function sameOrigin(server: Server): boolean {
  return new URL(server.baseUrl || window.location.origin).origin === window.location.origin;
}

export async function disableSync(server: Server): Promise<SyncSetup> {
  const result = await disableSyncForServer(server);
  if (sameOrigin(server)) {
    rememberStatus(result);
    window.dispatchEvent?.(new Event('hangar-sync-disabled'));
  }
  return result;
}

export async function prelogin(user: string): Promise<{ salt: string; iterations: number }> {
  const r = await jf(`/api/sync/prelogin?user=${encodeURIComponent(user)}`);
  if (!r.ok) throw new Error('prelogin failed');
  return await r.json();
}

export async function register(user: string, password: string, bootstrap: string): Promise<void> {
  const salt = b64(crypto.getRandomValues(new Uint8Array(16)).buffer);
  const { authHash } = await deriveKeys(password, salt, PBKDF2_ITERATIONS);
  const r = await jf('/api/sync/register', {
    method: 'POST', body: JSON.stringify({ user, salt, auth_hash: authHash, bootstrap }),
  });
  if (!r.ok) throw new Error(await errorDetail(r));
}

export async function login(user: string, password: string): Promise<CryptoKey> {
  const { salt, iterations } = await prelogin(user);
  const { authHash, encKey } = await deriveKeys(password, salt, iterations);
  const r = await jf('/api/sync/login', {
    method: 'POST', body: JSON.stringify({ user, auth_hash: authHash }),
  });
  if (!r.ok) throw new Error(r.status === 429 ? m.sync_muitas_tentativas() : m.sync_credenciais_invalidas());
  return encKey;
}

export async function logout(): Promise<void> {
  await jf('/api/sync/logout', { method: 'POST' });
}

export async function getVault(): Promise<{ enc_blob: { iv: string; data: string } | null; rev: number }> {
  const r = await jf('/api/sync/vault');
  if (!r.ok) throw new SyncRequestError('vault read failed', r.status);
  return await r.json();
}

export class SyncRequestError extends Error {
  constructor(message: string, readonly status: number) {
    super(message);
  }
}

export function isSyncUnauthorized(error: unknown): boolean {
  return error instanceof SyncRequestError && error.status === 401;
}

export async function putVault(
  blob: { iv: string; data: string } | null,
  baseRev: number,
): Promise<{ rev: number } | { conflict: { enc_blob: any; rev: number } }> {
  const r = await jf('/api/sync/vault', {
    method: 'PUT', body: JSON.stringify({ enc_blob: blob, base_rev: baseRev }),
  });
  if (r.status === 409) return { conflict: (await r.json()).detail };
  if (!r.ok) throw new Error('vault write failed');
  return await r.json();
}
