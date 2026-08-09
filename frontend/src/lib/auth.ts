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

// Listeners de mutacao local: o sync empurra pro hub, as views reconciliam seus streams SSE. E
// multi-listener de proposito — com um slot unico o 2o consumidor clobberava o 1o calado. Devolve o
// unsubscribe: quem registra por montagem (ou re-registra no relogin) TEM que chamar.
const _changed = new Set<() => void>();
export function onServersChanged(cb: () => void): () => void {
  _changed.add(cb);
  return () => {
    _changed.delete(cb);
  };
}
function notifyChanged(): void {
  // Itera uma copia: um listener que se desregistra durante o notify nao mexe na iteracao.
  for (const cb of [..._changed]) {
    // Isola por listener: sem o try, um que lanca (ex: o do Board faz `new EventSource(url)`, que
    // lanca SyntaxError com baseUrl malformado vindo do vault) abortava o loop e matava CALADO o
    // push do vault do App — a ordem e de insercao, entao quem morria dependia de timing.
    try { cb(); } catch (e) { console.error('listener de onServersChanged lancou:', e); }
  }
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

// Mesmo criterio do openSessionsStream (api.ts): so o servidor servido pela PROPRIA origem usa
// cookie; o resto vai por ?token= na URL. typeof window: auth.ts e importado em teste (env node,
// sem DOM) — sem origem conhecida nao da pra afirmar "mesma origem", entao false, que e o caminho
// conservador (cross-origin nao depende de cookie).
function isSameOrigin(baseUrl: string): boolean {
  if (typeof window === 'undefined') return false;
  return !baseUrl || baseUrl === window.location.origin;
}

function syncCookie(token: string | null): void {
  // Cookie same-origin pro SSE (EventSource não manda Authorization). Cross-origin usa ?token.
  // Max-Age de 1 ano DE PROPÓSITO: sem ele o cookie é de sessão e morre quando o navegador fecha.
  // O token continua no localStorage, então as chamadas REST (que mandam Authorization) seguem
  // funcionando e o app PARECE são — só o SSE volta 401, e a lista de sessões fica vazia.
  if (token) document.cookie = `cp_token=${token}; path=/; SameSite=Lax; Max-Age=31536000`;
  else document.cookie = 'cp_token=; path=/; expires=Thu, 01 Jan 1970 00:00:00 GMT';
}

// Cookie é por HOST (localhost e 127.0.0.1 são hosts diferentes) e morria ao fechar o navegador.
// Como só addServer/selectServer o escreviam, bastava abrir o app pela outra URL — ou reabrir o
// navegador — pra ficar com token no localStorage e sem cookie: REST 200, SSE 401, tela sem
// sessões. Reescrever no boot resolve os dois casos e é idempotente.
export function ensureCookie(): void {
  if (typeof document === 'undefined') return;
  cookieDaOrigem();
}

// O cookie é do servidor da PRÓPRIA ORIGEM — não do ativo. São coisas diferentes: o app fala com
// vários backends ao mesmo tempo, o remoto autentica o SSE por `?token=` na URL e só o da origem
// depende do cookie. Amarrar o cookie ao ATIVO fazia duas bobagens: com um remoto ativo, gravava a
// chave de OUTRA máquina no cookie desta origem (e agora, com Max-Age de um ano, ela ficaria lá);
// e trocar pro remoto apagava o cookie de que o stream do servidor local ainda precisava.
// Nenhum servidor na origem → limpa, em vez de deixar sobra de um ativo anterior.
function cookieDaOrigem(): void {
  const s = readServers().find((x) => isSameOrigin(x.baseUrl));
  syncCookie(s ? s.token : null);
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
  cookieDaOrigem();
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

// Atualiza token e/ou baseUrl de um servidor JA cadastrado, PRESERVANDO id e label. Sem isto, o
// unico jeito de trocar um token era remover + re-parear: perdia o label custom e a posicao na
// lista, e com 4 servidores rotacionar o CP_AUTH_TOKEN virava uma tarde de recadastro.
// Devolve false quando o id nao existe (mesma convencao do selectServer) pra UI poder avisar.
export function updateServer(id: string, patch: { token?: string; baseUrl?: string }): boolean {
  const list = readServers();
  const i = list.findIndex((s) => s.id === id);
  if (i < 0) return false;
  // Campo vazio = "nao mexe neste", nao "apaga": um token em branco desautenticaria o servidor
  // silenciosamente, e o formulario tem os dois campos.
  const token = patch.token?.trim() || list[i].token;
  const baseUrl = patch.baseUrl?.trim() ? normalizeBaseUrl(patch.baseUrl) : list[i].baseUrl;
  list[i] = { ...list[i], token, baseUrl };
  writeServers(list);
  // Cookie: ATIVO **ou** SAME-ORIGIN. O renameServer nao precisa disto (label nao autentica), mas
  // token sim — sem o resync, trocar o token gravava no storage e seguia mandando o ANTIGO.
  // O "ou same-origin" nao e zelo: openSessionsStream autentica o servidor same-origin PELO COOKIE
  // (withCredentials) e o cross-origin por ?token= na URL. So com "ativo", trocar o token do
  // servidor que HOSPEDA o PWA enquanto outro esta ativo atualizava o storage, e o reconnect
  // reabria o SSE dele com o cookie VELHO — justo o servidor que esta feature existe pra consertar.
  cookieDaOrigem();
  notifyChanged();
  return true;
}

export interface PareamentoValido {
  base: string;    // origin do backend (ou ?api=); '' quando token cru (só com aceitarTokenCru)
  token: string;
}

// Validação ESTRITA de texto de pareamento (manual e QR — uma função só, round 4 da 4b): a base
// precisa ser absoluta com hostname e protocolo http/https (htp://, ftp:// e api vazia caem fora),
// o token não pode ser vazio nem conter whitespace, nenhum parâmetro pode vir duplicado (token/api)
// e uma URL sem ?token= é recusada. Com `aceitarTokenCru` — USO EXCLUSIVO do ServerManager.saveToken
// — aceita um token cru (não vazio e sem whitespace) quando o texto NÃO é URL. Devolve null sem
// tocar storage.
export function validarPareamento(
  texto: string,
  opts?: { aceitarTokenCru?: boolean },
): PareamentoValido | null {
  const cru = texto.trim();
  if (!cru) return null;

  // Token cru (não é URL): só com a opção explícita, e sem whitespace interno.
  if (!cru.includes('://')) {
    if (!opts?.aceitarTokenCru) return null;
    if (/\s/.test(cru)) return null;
    return { base: '', token: cru };
  }

  let url: URL;
  try { url = new URL(cru); } catch { return null; }
  if ((url.protocol !== 'http:' && url.protocol !== 'https:') || !url.hostname) return null;

  // EXATAMENTE um token, não vazio, sem whitespace (getAll pega duplicatas; `+` decodifica pra espaço).
  const tokens = url.searchParams.getAll('token');
  if (tokens.length !== 1) return null;
  const token = tokens[0];
  if (!token || /\s/.test(token)) return null;

  // api ausente OU exatamente uma URL http/https com hostname e sem whitespace.
  const apis = url.searchParams.getAll('api');
  if (apis.length > 1) return null;
  let base = url.origin;
  if (apis.length === 1) {
    const api = apis[0];
    if (!api || /\s/.test(api)) return null;
    let apiUrl: URL;
    try { apiUrl = new URL(api); } catch { return null; }
    if ((apiUrl.protocol !== 'http:' && apiUrl.protocol !== 'https:') || !apiUrl.hostname) return null;
    base = apiUrl.origin;
  }
  return { base, token };
}

// Identidade COMPOSTA do alvo de configuração: id+label+baseUrl+token num JSON estável. O modal
// observa ISTO, não o id — trocar token/base/label com o MESMO id (rotação de token, re-parear) é
// OUTRA entidade pra config, e o store precisa recarregar em vez de achar que é o mesmo alvo.
// `null` (modo global, edita o servidor ATIVO) ganha um rótulo próprio.
export function serverIdentidade(s: Server | null): string {
  if (!s) return 'global';
  return JSON.stringify([s.id, s.label, s.baseUrl, s.token]);
}

export function selectServer(id: string): boolean {
  // Devolve false quando o id não existe localmente (push antigo, link de outra máquina) — quem
  // navega usa isso pra NÃO montar um chat contra o servidor ativo errado (cross-wire calado).
  const s = readServers().find((x) => x.id === id);
  if (!s) return false;
  localStorage.setItem(ACTIVE_KEY, id);
  cookieDaOrigem();
  return true;
}

export function removeServer(id: string, notify = true): void {
  const list = readServers().filter((s) => s.id !== id);
  writeServers(list);
  if (localStorage.getItem(ACTIVE_KEY) === id) {
    const next = list[0];
    if (next) {
      localStorage.setItem(ACTIVE_KEY, next.id);
      cookieDaOrigem();
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

// api.ts mira SEMPRE o servidor ativo -> aponta pro dono da operação e restaura no fim (mesmo em
// throw). Compartilhado por Sidebar e SessionContextMenu.
export async function withServer<T>(serverId: string, fn: () => Promise<T>): Promise<T> {
  const prev = getActiveId();
  selectServer(serverId);
  try { return await fn(); }
  finally { if (prev && prev !== serverId) selectServer(prev); }
}

// Logout total: limpa todos os servidores.
export function clearCredentials(): void {
  localStorage.removeItem(SERVERS_KEY);
  localStorage.removeItem(ACTIVE_KEY);
  localStorage.removeItem(LEGACY_BASE);
  localStorage.removeItem(LEGACY_TOKEN);
  syncCookie(null);
}
