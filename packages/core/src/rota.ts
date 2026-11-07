// Rota até cada servidor: o endereço da rede local quando ele responde como a MESMA máquina, senão
// o `baseUrl` (Tailscale). `baseUrl` continua sendo a identidade da entrada; só a rede muda.
import { apiEnv } from './apiEnv';
import { registrar as registrarDiag } from './diag';
import { hmacSha256Hex, sha256Hex } from './hmac';
import type { LanInfo, Server } from './servers';

// Rede local responde em poucos ms; fora dela o IP não existe ou é outra máquina.
const PRAZO_LAN_MS = 800;
const PRAZO_PRINCIPAL_MS = 4000;

const rotas = new Map<string, { base: string; connection: string }>();
const emCurso = new Map<string, { promise: Promise<void>; connection: string }>();
const generations = new Map<string, object>();
const remoteReads = new Map<string, { promise: Promise<LanInfo | null>; expires: number }>();
const reachReads = new Map<string, { promise: Promise<unknown>; expires: number }>();
const connectionKey = (server: Server) => JSON.stringify([server.baseUrl, server.token]);
const isLoopbackUrl = (url: string) => /^https?:\/\/(127\.0\.0\.1|localhost|\[::1\])(:|\/|$)/.test(url);

// Servidor que saiu da lista: endereço que nunca resolve (RFC 2606). Vazio iria à origem da
// página, que leva o cookie de login dela; a rota local guardada iria à máquina antiga.
export const REMOVED_BASE = 'http://servidor-removido.invalid';
export const UNCONFIRMED_BASE = 'http://server-unconfirmed.invalid';

function requiresRouteProof(s: Server): boolean {
  return !!(s.lan?.id || s.remote?.id) && s.baseUrl.startsWith('http:')
    && !s.invite && !isLoopbackUrl(s.baseUrl);
}

export function baseOf(s: Server): string {
  if (s.removed) return REMOVED_BASE;
  const route = rotas.get(s.id);
  if (route?.connection === connectionKey(s)) return route.base;
  return requiresRouteProof(s) ? UNCONFIRMED_BASE : s.baseUrl;
}

export function rotaDecidida(id: string, server?: Server): boolean {
  const route = rotas.get(id);
  return !!route && (!server || route.connection === connectionKey(server));
}

/** Falha de conexão: a rede pode ter mudado, a próxima conexão decide de novo. */
export function esquecerRota(id: string): void {
  rotas.delete(id);
  emCurso.delete(id);
  generations.delete(id);
}

// A resposta testa todos os endereços da máquina: quem pergunta junto divide a mesma leitura.
function readReach(server: Server, base: string): Promise<unknown> {
  const key = JSON.stringify([server.id, base, server.token]);
  const existing = reachReads.get(key);
  if (existing && existing.expires > Date.now()) return existing.promise;
  const promise = (async () => {
    const response = await fetch(`${base}/api/alcance`, {
      headers: { Authorization: `Bearer ${server.token}` }, signal: AbortSignal.timeout(8000), redirect: 'error',
    });
    if (!response.ok) throw new Error(`remote_discovery_http_${response.status}`);
    return await response.json() as unknown;
  })();
  reachReads.set(key, { promise, expires: Date.now() + 30_000 });
  return promise;
}

/** Descobre uma alternativa remota depois de uma conexão autenticada. */
export function discoverRemoteRoute(server: Server): Promise<LanInfo | null> {
  if (server.invite || server.removed || server.ephemeral || !server.lan?.id) return Promise.resolve(null);
  if (server.remote?.id === server.lan.id) return Promise.resolve(server.remote);
  const key = JSON.stringify([server.id, server.baseUrl, server.token, server.lan.id]);
  const existing = remoteReads.get(key);
  if (existing && existing.expires > Date.now()) return existing.promise;
  const read = (async () => {
    const base = baseOf(server);
    if (base === UNCONFIRMED_BASE) return null;
    const value = await readReach(server, base);
    const addresses = (value as { enderecos?: unknown } | null)?.enderecos;
    if (!Array.isArray(addresses)) throw new Error('remote_discovery_invalid_response');
    for (const item of addresses) {
      if (item?.tipo !== 'tailscale' || typeof item.url !== 'string' || !item.url) continue;
      const url = new URL(item.url);
      if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) continue;
      const target = { url: url.toString().replace(/\/+$/, ''), id: server.lan!.id };
      if (await verifyServerIdentity(server, target, AbortSignal.timeout(PRAZO_PRINCIPAL_MS))) return target;
    }
    return null;
  })().catch((error: unknown) => {
    registrarDiag({ evento: 'rota.descoberta_falhou', nivel: 'aviso', codigo: error instanceof Error ? error.name : 'erro' }, server.baseUrl);
    return null;
  }).then((result) => {
    const current = remoteReads.get(key);
    if (current?.promise === read) current.expires = result ? Infinity : Date.now() + 30_000;
    return result;
  });
  remoteReads.set(key, { promise: read, expires: Infinity });
  return read;
}

/** Endereço que a máquina anuncia para o Connect; `null` sem Connect, em máquina antiga ou em falha. */
export async function readConnectUrl(server: Server): Promise<string | null> {
  const base = baseOf(server);
  if (base === UNCONFIRMED_BASE || base === REMOVED_BASE) return null;
  try {
    const raw = ((await readReach(server, base)) as { connect?: unknown } | null)?.connect;
    if (typeof raw !== 'string' || !raw) return null;
    const url = new URL(raw);
    if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash) return null;
    return url.toString().replace(/\/+$/, '');
  } catch (error) {
    registrarDiag({ evento: 'connect.leitura_falhou', nivel: 'aviso', codigo: error instanceof Error ? error.name : 'erro' }, server.baseUrl);
    return null;
  }
}

// Só com credencial de aparelho: pelo Connect a senha do dono volta 401, e 401 tira o servidor da lista.
export async function discoverConnectRoute(server: Server): Promise<LanInfo | null> {
  const id = server.lan?.id;
  if (server.credential !== 'device' || !server.deviceId || server.invite || !id) return null;
  const url = await readConnectUrl(server);
  if (!url) return null;
  if (server.connect?.url === url && server.connect.id === id) return server.connect;
  const target = { url, id };
  return await verifyServerIdentity(server, target, AbortSignal.timeout(PRAZO_PRINCIPAL_MS)) ? target : null;
}

function connectCandidate(s: Server): LanInfo | null {
  return s.credential === 'device' && s.deviceId && s.connect?.url && s.connect.url !== s.baseUrl ? s.connect : null;
}

async function perguntar(base: string, token: string, ms: number): Promise<{ identificador?: string; lan_url?: string } | null> {
  try {
    const r = await fetch(`${base}/api/peers/identificador`, {
      headers: { Authorization: `Bearer ${token}` }, signal: AbortSignal.timeout(ms),
    });
    return r.ok ? (await r.json()) as { identificador?: string; lan_url?: string } : null;
  } catch {
    return null;
  }
}

// Página HTTPS não pode chamar http:// (o navegador bloqueia): nem tenta.
function lanPermitida(url: string): boolean {
  return !(apiEnv().origin?.startsWith('https:') && url.startsWith('http:'));
}

async function aprender(s: Server, current: () => boolean, base = s.baseUrl, timeout = PRAZO_PRINCIPAL_MS): Promise<LanInfo | null> {
  const r = await perguntar(base, s.token, timeout);
  if (!r?.identificador || !current()) return null;
  const lan = { url: (r.lan_url ?? '').replace(/\/+$/, ''), id: r.identificador };
  if (lan.url !== s.lan?.url || lan.id !== s.lan?.id) apiEnv().rememberLan?.(s.id, lan, s);
  return lan;
}

function desafio(): string {
  const b = new Uint8Array(16);
  if (globalThis.crypto?.getRandomValues) globalThis.crypto.getRandomValues(b);
  else for (let i = 0; i < b.length; i++) b[i] = Math.floor(Math.random() * 256);
  return Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
}

// Mesmo IP em outra rede pode ser outra máquina: o token só vai ao destino depois de ele
// provar que conhece o token (HMAC do desafio) e dizer o mesmo nome.
export async function verifyServerIdentity(s: Server, target: LanInfo | null | undefined, signal?: AbortSignal): Promise<boolean> {
  if (!target?.url || !target.id || !lanPermitida(target.url)) return false;
  const d = desafio();
  // A máquina guarda só o hash do token do aparelho, e ele é a chave da prova.
  const device = s.credential === 'device' ? s.deviceId : undefined;
  const key = device ? sha256Hex(new TextEncoder().encode(s.token)) : s.token;
  const query = device ? `&device=${encodeURIComponent(device)}` : '';
  let r: { identificador?: string; prova?: string } | null = null;
  let bound = '';
  try {
    // A prova de aparelho é pública: sem o host, uma rede hostil a repassaria do endereço do Connect.
    if (device) bound = `|${new URL(target.url).host}`;
    const res = await fetch(`${target.url}/api/peers/prova?desafio=${d}${query}`, {
      signal: signal ?? AbortSignal.timeout(PRAZO_LAN_MS), credentials: 'omit',
    });
    r = res.ok ? (await res.json()) as { identificador?: string; prova?: string } : null;
  } catch {
    return false;
  }
  return r?.identificador === target.id && r.prova === hmacSha256Hex(key, `${d}|${target.id}${bound}`);
}

// O endereço local ganha só se provar a identidade ANTES de o principal responder: o mesmo IP
// alcançado por VPN de outra rede responde, mas mais devagar que o Tailscale.
async function decidir(s: Server, current: () => boolean): Promise<void> {
  const anterior = baseOf(s);
  const inicio = Date.now();
  let primaryAnswered = false;
  let primaryConfirmed = !requiresRouteProof(s);
  const inspect = async (base: string, timeout: number) => {
    const started = Date.now();
    const learned = await aprender(s, current, base, timeout);
    const elapsed = Date.now() - started;
    if (!learned) throw new Error('primary_unavailable');
    if (base === s.baseUrl) primaryAnswered = true;
    if (learned.url && learned.url !== s.lan?.url) {
      const probeStarted = Date.now();
      if (await verifyServerIdentity(s, learned) && Date.now() - probeStarted <= elapsed) return learned.url;
    }
    return base;
  };
  const principal = (async () => {
    const id = s.lan?.id || s.remote?.id;
    if (id && requiresRouteProof(s)) {
      if (!await verifyServerIdentity(s, { url: s.baseUrl, id }, AbortSignal.timeout(PRAZO_PRINCIPAL_MS))) {
        throw new Error('primary_unconfirmed');
      }
      primaryAnswered = true;
    }
    primaryConfirmed = true;
    const remaining = PRAZO_PRINCIPAL_MS - (Date.now() - inicio);
    return remaining > 0 ? inspect(s.baseUrl, remaining) : s.baseUrl;
  })();
  const prove = async (route: LanInfo, timeout: number) => {
    if (!await verifyServerIdentity(s, route, AbortSignal.timeout(timeout))) throw new Error('route_unconfirmed');
    return primaryAnswered ? s.baseUrl : route.url;
  };
  const candidates = [principal];
  if (s.lan?.url && s.lan.url !== s.baseUrl) candidates.push(prove(s.lan, PRAZO_LAN_MS));
  if (s.remote?.url && s.remote.url !== s.baseUrl) candidates.push((async () => {
    const base = await prove(s.remote!, PRAZO_PRINCIPAL_MS);
    const remaining = PRAZO_PRINCIPAL_MS - (Date.now() - inicio);
    if (remaining <= 0 || !current()) return base;
    return inspect(base, remaining).catch(() => base);
  })());
  // Prova em paralelo para não somar ao prazo, mas o Connect só vale depois de as outras falharem.
  const connect = connectCandidate(s);
  const connectProof = connect && verifyServerIdentity(s, connect, AbortSignal.timeout(PRAZO_PRINCIPAL_MS));
  const chosen = await new Promise<string | null>((resolve) => {
    let remaining = candidates.length;
    for (const candidate of candidates) void candidate.then(resolve, () => {
      if (--remaining === 0) resolve(null);
    });
  });
  const agora = chosen ?? (connect && await connectProof ? connect.url : primaryConfirmed ? s.baseUrl : UNCONFIRMED_BASE);
  if (!current()) return;
  rotas.set(s.id, { base: agora, connection: connectionKey(s) });
  if (agora !== anterior) {
    const detalhe = agora === s.baseUrl ? 'principal' : agora === s.remote?.url ? 'tailscale' : agora === connect?.url ? 'connect' : 'rede_local';
    registrarDiag({ evento: 'rota.escolhida', detalhe, ms: Date.now() - inicio }, s.baseUrl);
  }
}

/** Decide a rota de um servidor. Nunca rejeita; chamadas simultâneas dividem a mesma decisão. */
export function decidirRota(s: Server): Promise<void> {
  // Mesma origem ou esta própria máquina já é o caminho mais curto.
  if (s.invite || !s.baseUrl || isLoopbackUrl(s.baseUrl)) {
    rotas.set(s.id, { base: s.baseUrl, connection: connectionKey(s) });
    return Promise.resolve();
  }
  const connection = connectionKey(s);
  const existing = emCurso.get(s.id);
  if (existing?.connection === connection) return existing.promise;
  const generation = {};
  generations.set(s.id, generation);
  const current = () => generations.get(s.id) === generation;
  const promise = decidir(s, current).catch((e: unknown) => {
      if (!current()) return;
      rotas.set(s.id, { base: requiresRouteProof(s) ? UNCONFIRMED_BASE : s.baseUrl, connection });
      registrarDiag({ evento: 'rota.falhou', nivel: 'erro', codigo: e instanceof Error ? e.name : 'erro' }, s.baseUrl);
    }).finally(() => { if (current()) emCurso.delete(s.id); });
  emCurso.set(s.id, { promise, connection });
  return promise;
}

export function _resetRotasForTests(): void {
  rotas.clear();
  emCurso.clear();
  remoteReads.clear();
  reachReads.clear();
  generations.clear();
}
