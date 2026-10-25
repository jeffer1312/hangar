import { comTeto, errorDetail, getConfigForServer, isLocalMachineUrl, normalizeMachineUrl, probeServerResponse, verifyServerIdentity, type LanInfo, type Server } from '@hangar/core';
import * as m from '../../paraglide/messages';

export type MachineId = { identificador: string; lan_url: string };
export type Warning = { id: string; estado: string; motivo: string };
export type Peer = { id: string; base_url: string; enabled: boolean };
export type Check = { estado: 'ok' | 'estranho' | 'falhou' | 'recusou' | 'nao_configurado'; identificador: string; motivo: string; tempo_ms: number | null };
export type Reach = { loopback: boolean; bind: string; enderecos: { tipo: 'nesta_maquina' | 'rede_local' | 'tailscale' | 'publico'; url: string; estado: string; tempo_ms: number | null; motivo: string }[] };
export type Discovered = { nome: string; base_url: string; hosts?: string[] };
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const duration = (value: unknown) => value === null || (typeof value === 'number' && Number.isFinite(value) && value >= 0);
export const validIdentifier = (value: string) => value === '' || /^[a-z0-9][a-z0-9_-]{0,31}$/.test(value);
const invalid = () => new Error(m.mobile_machines_invalid_response());
export const reason = (error: unknown) => error instanceof Error
  ? error.message === 'machine_address_duplicate' ? m.mobile_machines_duplicate_address() : error.message
  : m.erro_desconhecido();
export function ensureActive(signal: AbortSignal) {
  if (signal.aborted) throw Object.assign(new Error('cancelled'), { name: 'AbortError' });
}

export function normalizeUrl(raw: string): string {
  try { return normalizeMachineUrl(raw); }
  catch { throw new Error(m.mobile_machines_invalid_url()); }
}

export const isLoopback = isLocalMachineUrl;

async function request(server: Server, path: string, signal: AbortSignal, method = 'GET', body?: unknown, timeout = 15_000): Promise<unknown> {
  ensureActive(signal);
  const response = await probeServerResponse(server, path, {
    method, signal: comTeto(signal, timeout), ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok) throw Object.assign(new Error(`${response.status}: ${await errorDetail(response)}`), { status: response.status });
  let value: unknown;
  try { value = await response.json(); }
  catch (error) {
    ensureActive(signal);
    if (error instanceof SyntaxError) throw invalid();
    throw error;
  }
  ensureActive(signal);
  return value;
}

export async function readIdentifier(server: Server, signal: AbortSignal): Promise<MachineId> {
  const value = await request(server, '/api/peers/identificador', signal);
  if (!object(value) || typeof value.identificador !== 'string' || !validIdentifier(value.identificador) || typeof value.lan_url !== 'string') throw invalid();
  return { identificador: value.identificador, lan_url: value.lan_url };
}

export async function confirmMachineRoute(server: Server, address: string, signal: AbortSignal): Promise<LanInfo> {
  const url = normalizeUrl(address);
  const id = server.lan?.id || server.remote?.id || (await readIdentifier(server, signal)).identificador;
  ensureActive(signal);
  if (!id) throw new Error(m.mobile_machines_known_identifier_required());
  const target = { url, id };
  if (!await verifyServerIdentity(server, target, comTeto(signal, 15_000))) throw new Error(m.mobile_machines_identity_unconfirmed());
  ensureActive(signal);
  // A sonda usa uma chave própria para atingir o endereço escolhido, sem reutilizar a rota antiga.
  const candidate = { id: `machine-probe:${url}`, label: server.label, baseUrl: url, token: server.token };
  const config = await getConfigForServer(candidate, 15_000);
  ensureActive(signal);
  if (!config || !config.campos || typeof config.campos !== 'object') throw invalid();
  return target;
}

export async function saveIdentifier(server: Server, identificador: string, signal: AbortSignal): Promise<{ identificador: string; avisos: Warning[] }> {
  if (!validIdentifier(identificador)) throw new Error(m.mobile_machines_identifier_hint());
  const value = await request(server, '/api/peers/identificador', signal, 'PUT', { identificador });
  if (!object(value) || typeof value.identificador !== 'string' || !validIdentifier(value.identificador) || !Array.isArray(value.avisos)
    || !value.avisos.every((w) => object(w) && typeof w.id === 'string' && typeof w.estado === 'string' && typeof w.motivo === 'string')) throw invalid();
  return value as { identificador: string; avisos: Warning[] };
}

/** `GET`/`PUT /api/web-ui`. `null`: servidor anterior à rota (404/405), que não oferece a escolha. */
export async function webUi(server: Server, signal: AbortSignal, on?: boolean): Promise<boolean | null> {
  let value: unknown;
  try { value = await request(server, '/api/web-ui', signal, on === undefined ? 'GET' : 'PUT', on === undefined ? undefined : { web_ui: on }); }
  catch (error) {
    if ([404, 405].includes((error as { status?: number }).status ?? 0)) return null;
    throw error;
  }
  if (!object(value) || typeof value.web_ui !== 'boolean') throw invalid();
  return value.web_ui;
}

export type DevMode = { developer: boolean; channel: string; checkout: string };

/** O modo da máquina migrada, do `GET /api/atualizacao`. `null`: servidor sem a troca pela tela ou máquina sem migrar. */
export async function readDevMode(server: Server, signal: AbortSignal): Promise<DevMode | null> {
  let value: unknown;
  try { value = await request(server, '/api/atualizacao', signal, 'GET', undefined, 20_000); }
  catch (error) {
    if ([404, 405].includes((error as { status?: number }).status ?? 0)) return null;
    throw error;
  }
  if (!object(value)) throw invalid();
  if ((value.modo !== 'pacote' && value.modo !== 'desenvolvedor') || typeof value.canal !== 'string') return null;
  return { developer: value.modo === 'desenvolvedor', channel: value.canal, checkout: typeof value.checkout === 'string' ? value.checkout : '' };
}

/** `POST /api/atualizacao/dev-mode`: o pid do executor a acompanhar, ou o `detalhe` quando a máquina já estava no modo pedido. */
export async function switchDevMode(server: Server, on: boolean, checkout: string, signal: AbortSignal): Promise<{ pid: number } | { already: string }> {
  // O teto do nativo: a rota confere o checkout antes de lançar o executor.
  const value = await request(server, '/api/atualizacao/dev-mode', signal, 'POST', on ? { on, checkout } : { on }, 30_000);
  if (object(value) && value.nada_a_fazer === true) return { already: typeof value.detalhe === 'string' ? value.detalhe : '' };
  if (!object(value) || value.ok !== true || typeof value.pid !== 'number' || !Number.isSafeInteger(value.pid) || value.pid <= 0) throw invalid();
  return { pid: value.pid };
}

function peers(value: unknown): Peer[] {
  if (!Array.isArray(value) || !value.every((p) => object(p) && typeof p.id === 'string' && validIdentifier(p.id) && !!p.id
    && typeof p.base_url === 'string' && (p.enabled === undefined || typeof p.enabled === 'boolean'))) throw invalid();
  // O token da listagem é uma máscara, nunca uma credencial utilizável.
  return value.map((p) => ({ id: p.id, base_url: normalizeUrl(p.base_url), enabled: p.enabled !== false }));
}
export async function readPeers(s: Server, signal: AbortSignal): Promise<Peer[]> { return peers(await request(s, '/api/peers', signal)); }
export async function enablePeer(s: Server, id: string, enabled: boolean, signal: AbortSignal): Promise<Peer[]> {
  return peers(await request(s, `/api/peers/${encodeURIComponent(id)}/enabled`, signal, 'PUT', { enabled }));
}
export async function removePeer(s: Server, id: string, signal: AbortSignal): Promise<Peer[]> {
  return peers(await request(s, `/api/peers/${encodeURIComponent(id)}`, signal, 'DELETE'));
}
export async function registerPeer(s: Server, id: string, url: string, token: string, signal: AbortSignal): Promise<Peer[]> {
  if (!id || !validIdentifier(id)) throw new Error(m.mobile_machines_identifier_hint());
  const base_url = normalizeUrl(url);
  if (isLoopback(base_url)) throw new Error(m.mobile_machines_no_reachable_address());
  if (!token.trim()) throw new Error(m.mobile_machines_token_required());
  return peers(await request(s, '/api/peers', signal, 'POST', { id, base_url, token }));
}
export async function checkPeer(s: Server, url: string, id: string, signal: AbortSignal): Promise<Check> {
  const value = await request(s, `/api/peers/check?url=${encodeURIComponent(normalizeUrl(url))}&id=${encodeURIComponent(id)}`, signal, 'GET', undefined, 30_000);
  if (!object(value) || !['ok', 'estranho', 'falhou', 'recusou', 'nao_configurado'].includes(String(value.estado))
    || typeof value.identificador !== 'string' || typeof value.motivo !== 'string' || !duration(value.tempo_ms)) throw invalid();
  const check = value as Check;
  return check.estado === 'ok' && check.identificador !== id
    ? { ...check, estado: 'estranho', motivo: check.motivo || `${id} ≠ ${check.identificador}` } : check;
}
export async function readReach(s: Server, signal: AbortSignal): Promise<Reach> {
  const value = await request(s, '/api/alcance', signal, 'GET', undefined, 30_000);
  if (!object(value) || typeof value.loopback !== 'boolean' || typeof value.bind !== 'string' || !Array.isArray(value.enderecos)
    || !value.enderecos.every((a) => object(a) && ['nesta_maquina', 'rede_local', 'tailscale', 'publico'].includes(String(a.tipo))
      && typeof a.url === 'string' && typeof a.estado === 'string' && typeof a.motivo === 'string' && duration(a.tempo_ms))) throw invalid();
  return value as Reach;
}
export async function discover(s: Server, signal: AbortSignal): Promise<Discovered[]> {
  const value = await request(s, '/api/peers/descobrir', signal, 'GET', undefined, 30_000);
  if (!Array.isArray(value) || !value.every((d) => object(d) && typeof d.nome === 'string' && typeof d.base_url === 'string'
    && (d.hosts === undefined || Array.isArray(d.hosts) && d.hosts.every((host: unknown) => typeof host === 'string')))) throw invalid();
  return value.map((d) => ({ nome: d.nome, base_url: normalizeUrl(d.base_url), ...(d.hosts ? { hosts: d.hosts } : {}) }));
}
export async function ownerAddress(s: Server, signal: AbortSignal): Promise<string> {
  const base = normalizeUrl(s.baseUrl);
  if (!isLoopback(base)) return base;
  const reach = await readReach(s, signal);
  const port = new URL(base).port;
  const address = reach.enderecos.filter((a) => {
    if (a.estado !== 'ok' || a.tipo === 'nesta_maquina' || !a.url) return false;
    try { const url = normalizeUrl(a.url); return !isLoopback(url) && (!new URL(url).port || new URL(url).port === port); }
    catch { return false; }
  }).sort((a, b) => (a.tempo_ms ?? Infinity) - (b.tempo_ms ?? Infinity))[0];
  if (!address) throw new Error(m.mobile_machines_no_reachable_address());
  return normalizeUrl(address.url);
}

export async function registerBoth(owner: Server, remote: Server, remoteId: string, signal: AbortSignal): Promise<{ directions: Directions; partial: string }> {
  const identity = await readIdentifier(owner, signal);
  if (!identity.identificador || !remoteId) throw new Error(m.mobile_machines_identifier_required());
  if (identity.identificador === remoteId) throw new Error(m.mobile_machines_identifier_distinct());
  const confirmed = await readIdentifier(remote, signal);
  if (confirmed.identificador !== remoteId) {
    throw new Error(m.mobile_machines_other_machine({ expected: remoteId, actual: confirmed.identificador || '—' }));
  }
  const url = await ownerAddress(owner, signal);
  await registerPeer(owner, remoteId, remote.baseUrl, remote.token, signal);
  let partial = '';
  try { await registerPeer(remote, identity.identificador, url, owner.token, signal); }
  catch (error) { ensureActive(signal); partial = reason(error); }
  const directions = await measureDirections(owner, { id: remoteId, base_url: remote.baseUrl }, remote, signal, { id: identity.identificador, url });
  if (!partial && (directions.forward?.estado !== 'ok' || directions.back?.estado !== 'ok')) {
    partial = directions.forwardError || directions.backError || [directions.forward?.estado, directions.back?.estado].filter(Boolean).join(' · ');
  }
  return { directions, partial };
}

export type Directions = { forward?: Check; back?: Check; forwardError?: string; backError?: string };
export async function measureDirections(owner: Server, peer: Pick<Peer, 'id' | 'base_url'>, remote: Server | null, signal: AbortSignal, ownerTarget?: { id: string; url: string }): Promise<Directions> {
  const [forward, back] = await Promise.allSettled([
    checkPeer(owner, peer.base_url, peer.id, signal),
    (async () => {
      if (!remote) return undefined;
      const target = ownerTarget ?? await (async () => {
        const identity = await readIdentifier(owner, signal);
        if (!identity.identificador) throw new Error(m.mobile_machines_identifier_required());
        return { id: identity.identificador, url: await ownerAddress(owner, signal) };
      })();
      return checkPeer(remote, target.url, target.id, signal);
    })(),
  ]);
  ensureActive(signal);
  return {
    ...(forward.status === 'fulfilled' ? { forward: forward.value } : { forwardError: reason(forward.reason) }),
    ...(back.status === 'fulfilled' && back.value ? { back: back.value } : { backError: back.status === 'rejected' ? reason(back.reason) : m.mobile_machines_back_unchecked() }),
  };
}
