// Multi-servidor: o app guarda N backends (casa, trabalho, ...) e troca entre eles. Cada server
// guarda baseUrl ABSOLUTO (origin) + token. O ativo decide pra quem os requests vão. API usa
// header Bearer (cross-origin OK via CORS); SSE same-origin usa cookie, cross-origin usa ?token.

import { normalizeBaseUrl } from './url';

const SERVERS_KEY = 'cp_servers';
const ACTIVE_KEY = 'cp_active';
// Chaves legadas do modelo single-server: migradas pra lista uma vez.
const LEGACY_BASE = 'cp_base_url';
const LEGACY_TOKEN = 'cp_token';

export interface Server {
  id: string;
  label: string;
  baseUrl: string;
  token: string;
}

function readServers(): Server[] {
  try {
    const raw = localStorage.getItem(SERVERS_KEY);
    if (raw) {
      const arr = JSON.parse(raw);
      if (Array.isArray(arr)) {
        return arr.filter((s) => s && s.id && typeof s.token === 'string');
      }
    }
  } catch {
    /* JSON corrompido -> trata como vazio */
  }
  return [];
}

function writeServers(list: Server[]): void {
  localStorage.setItem(SERVERS_KEY, JSON.stringify(list));
}

// Listener unico: o sync registra aqui pra empurrar pro hub apos qualquer mutacao local.
let _changed: (() => void) | null = null;
export function onServersChanged(cb: () => void): void {
  _changed = cb;
}
function notifyChanged(): void {
  if (_changed) _changed();
}

// Une a lista do hub (remote) com a local: remote tem precedencia em duplicata (mesma baseUrl
// normalizada); servers locais que o hub ainda nao tem sao acrescentados. Usado no login do sync
// pra SEMEAR no hub os servers que ja existiam so no navegador (senao nunca subiam).
export function mergeServers(remote: Server[], local: Server[]): Server[] {
  const norm = (u: string) => u.replace(/\/+$/, '');
  const seen = new Set(remote.map((s) => norm(s.baseUrl)));
  return [...remote, ...local.filter((s) => !seen.has(norm(s.baseUrl)))];
}

// Sobrescreve a lista inteira (hidratacao a partir do vault decifrado). Mantem o ativo se ainda
// existir, senao cai pro primeiro. NAO dispara notifyChanged (veio do hub, nao re-empurrar).
export function setServers(list: Server[]): void {
  writeServers(list);
  const active = localStorage.getItem(ACTIVE_KEY);
  if (!active || !list.some((s) => s.id === active)) {
    if (list[0]) localStorage.setItem(ACTIVE_KEY, list[0].id);
    else localStorage.removeItem(ACTIVE_KEY);
  }
}

function makeId(): string {
  return 'srv-' + Math.random().toString(36).slice(2, 10);
}

// Cor estável por servidor (mesmo id -> mesma cor), pros badges/dots distinguirem origem.
const SERVER_COLORS = ['#7c6af7', '#3ba55d', '#e0a23b', '#e0563b', '#3b9fe0', '#c43be0'];
export function serverColor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return SERVER_COLORS[h % SERVER_COLORS.length];
}

function labelFor(baseUrl: string): string {
  try {
    const h = new URL(baseUrl).hostname;
    return h.split('.')[0] || h; // 1o rótulo do host (ex: jefferson-felizardo)
  } catch {
    return baseUrl || 'servidor';
  }
}

function syncCookie(token: string | null): void {
  // Cookie same-origin pro SSE (EventSource não manda Authorization). Cross-origin usa ?token.
  if (token) document.cookie = `cp_token=${token}; path=/; SameSite=Lax`;
  else document.cookie = 'cp_token=; path=/; expires=Thu, 01 Jan 1970 00:00:00 GMT';
}

// Migração single-server -> multi (uma vez). baseUrl vazio vira o origin atual (absoluto) pra
// não ficar ambíguo quando o app for carregado de outra origem.
function migrate(): void {
  if (localStorage.getItem(SERVERS_KEY)) return;
  const token = localStorage.getItem(LEGACY_TOKEN);
  if (!token) return;
  const base = localStorage.getItem(LEGACY_BASE) || window.location.origin;
  const s: Server = { id: makeId(), label: labelFor(base), baseUrl: base, token };
  writeServers([s]);
  localStorage.setItem(ACTIVE_KEY, s.id);
}
migrate();

function activeServer(): Server | null {
  const id = localStorage.getItem(ACTIVE_KEY);
  const list = readServers();
  return list.find((s) => s.id === id) ?? list[0] ?? null;
}

export function listServers(): Server[] {
  return readServers();
}

export function getActiveId(): string | null {
  return activeServer()?.id ?? null;
}

export function getBaseUrl(): string {
  return activeServer()?.baseUrl ?? '';
}

export function getToken(): string | null {
  return activeServer()?.token ?? null;
}

export function isAuthenticated(): boolean {
  return activeServer() !== null;
}

// Adiciona (ou atualiza por baseUrl normalizado) um servidor e o torna ATIVO. Não sobrescreve os
// outros. Devolve {id, existed} pra quem chama poder fazer rollback.
export function addServer(
  baseUrl: string,
  token: string,
  label?: string,
): { id: string; existed: boolean } {
  baseUrl = normalizeBaseUrl(baseUrl);
  const norm = (u: string) => u.replace(/\/+$/, '');
  const list = readServers();
  const i = list.findIndex((s) => norm(s.baseUrl) === norm(baseUrl));
  let id: string;
  let existed: boolean;
  if (i >= 0) {
    id = list[i].id;
    existed = true;
    list[i] = { ...list[i], token, label: label ?? list[i].label };
  } else {
    id = makeId();
    existed = false;
    list.push({ id, label: label ?? labelFor(baseUrl), baseUrl, token });
  }
  writeServers(list);
  localStorage.setItem(ACTIVE_KEY, id);
  syncCookie(token);
  notifyChanged();
  return { id, existed };
}

// Renomeia um servidor (label custom persistido). Vazio volta pro rotulo derivado da URL — nao da
// pra deixar sem nome. Nao mexe em token/baseUrl/ativo.
export function renameServer(id: string, label: string): void {
  const list = readServers();
  const i = list.findIndex((s) => s.id === id);
  if (i < 0) return;
  list[i] = { ...list[i], label: label.trim() || labelFor(list[i].baseUrl) };
  writeServers(list);
  notifyChanged();
}

// Extrai base (origin do backend) + token de um texto de pareamento — o mesmo conteudo que o QR
// carrega: uma URL com ?token=… (e opcional ?api=… apontando o backend por tras de um proxy). Token
// cru sem URL nao tem origem confiavel -> null. Pura/testavel (nao toca storage): usada tanto pelo
// scan do QR quanto pelo colar manual da URL no desktop (mesma rota de parse).
export function parseServerPairing(text: string): { base: string; token: string } | null {
  let tok = text.trim();
  let base = '';
  try {
    const u = new URL(text);
    const t = u.searchParams.get('token');
    if (t) tok = t;
    base = u.searchParams.get('api') ?? u.origin;
  } catch {
    base = ''; // token cru sem URL -> sem origem confiavel
  }
  if (!tok || !base) return null;
  return { base, token: tok };
}

export function selectServer(id: string): boolean {
  // Devolve false quando o id não existe localmente (push antigo, link de outra máquina) — quem
  // navega usa isso pra NÃO montar um chat contra o servidor ativo errado (cross-wire calado).
  const s = readServers().find((x) => x.id === id);
  if (!s) return false;
  localStorage.setItem(ACTIVE_KEY, id);
  syncCookie(s.token);
  return true;
}

export function removeServer(id: string, notify = true): void {
  const list = readServers().filter((s) => s.id !== id);
  writeServers(list);
  if (localStorage.getItem(ACTIVE_KEY) === id) {
    const next = list[0];
    if (next) {
      localStorage.setItem(ACTIVE_KEY, next.id);
      syncCookie(next.token);
    } else {
      localStorage.removeItem(ACTIVE_KEY);
      syncCookie(null);
    }
  }
  if (notify) notifyChanged();
}

// Token do servidor ATIVO furou (401): remove só ele e cai pro próximo (se houver). Assim um token
// expirado num PC não desloga dos outros.
// A 401 token-drop is local-only — it must not push the removal to the sync hub, so a
// rotated/expired token doesn't delete the server from the vault across all devices;
// on reload it re-hydrates and the user re-pairs.
export function dropActiveServer(): void {
  const id = getActiveId();
  if (id) removeServer(id, false);
}

// Logout total: limpa todos os servidores.
export function clearCredentials(): void {
  localStorage.removeItem(SERVERS_KEY);
  localStorage.removeItem(ACTIVE_KEY);
  localStorage.removeItem(LEGACY_BASE);
  localStorage.removeItem(LEGACY_TOKEN);
  syncCookie(null);
}
