// Conta do Connect e entrada de aparelho. O `secret` vai direto à máquina e ao serviço só o hash
// dele: isso protege de um serviço honesto que vaze. Um serviço desonesto pode pré-aprovar bilhetes:
// limite aceito em docs/decisoes/plataforma.md ("Confiança na conta").
import { probeServerResponse } from './api';
import { apiEnv } from './apiEnv';
import { registrar as registrarDiag } from './diag';
import { SERVIDOR_CANDIDATO } from './esfriamento';
import { normalizeGuestUser } from './guests';
import { sha256Hex } from './hmac';
import { isLocalMachineUrl, isOwnMachine, normalizeMachineUrl, readMachineIdentity } from './machineImport';
import { baseOf, readConnectUrl } from './rota';
import type { Server } from './servers';
import { syncSignIn, type SyncCrypto } from './syncLogin';

export const CONNECT_SERVICE_URL = 'https://connect.hangar.dev.br';

const REQUEST_TIMEOUT_MS = 15_000;
const APPROVAL_POLL_MS = 3000;
// O pedido pendente expira neste prazo na máquina (`PENDING_TTL`).
const APPROVAL_WINDOW_MS = 10 * 60_000;

/** Credencial deste aparelho na conta (`POST /v1/devices`); vai ao serviço, nunca à máquina. */
export interface AccountDeviceKey { id: string; secret: string }
export interface AccountSession { serviceUrl: string; token: string; device?: AccountDeviceKey }
export interface ConnectMachine { machine_id: string; name: string; host: string; online: boolean }
export type AccountDeviceStatus = 'pending' | 'approved' | 'revoked';
/** Linha de `GET /v1/devices`; tempos em segundos. `install_code`: o mesmo código que o celular mostra (`installCode`),
 *  pronto do serviço, que nunca manda o `install_id`; ausente em serviço antigo ou aparelho sem identificador. */
export interface AccountDevice {
  device_id: string; name: string; status: AccountDeviceStatus; created_at: number; approved_at: number | null; revoked_at: number | null;
  install_code?: string | null;
}

const fail = (code: string, status?: number, fromService = false) => Object.assign(new Error(code), { code, status }, fromService ? { fromService } : {});
const record = (value: unknown): Record<string, unknown> => (value && typeof value === 'object' && !Array.isArray(value)
  ? value as Record<string, unknown> : {});
const filled = (value: unknown): value is string => typeof value === 'string' && !!value;
const STATUSES: readonly unknown[] = ['pending', 'approved', 'revoked'];
// Motivos do 403 do serviço que a tela explica; qualquer outro vira o genérico.
const REASONS: readonly unknown[] = ['device_required', 'device_pending', 'device_revoked'];

async function request(base: string, path: string, init: RequestInit): Promise<unknown> {
  // A senha da conta passa por aqui: http só em loopback, que é o caso dos testes.
  if (new URL(base).protocol !== 'https:' && !isLocalMachineUrl(base)) throw fail('connect_service_insecure');
  const response = await fetch(`${base.replace(/\/+$/, '')}${path}`, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...init.headers },
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS), redirect: 'error', credentials: 'omit',
  });
  if (!response.ok) {
    // Do corpo do erro só sai o motivo conhecido, nunca o texto: ele pode ecoar o que recebeu.
    const body = record(await response.json().catch(() => undefined));
    // `fromService`: o corpo é o `{"error": …}` do serviço, não o de um proxy no caminho.
    throw fail(REASONS.includes(body.reason) ? `connect_${String(body.reason)}` : 'connect_request_failed', response.status,
      typeof body.error === 'string');
  }
  return response.status === 204 ? null : await response.json() as unknown;
}

const post = (base: string, path: string, body: unknown, headers?: Record<string, string>) =>
  request(base, path, { method: 'POST', body: JSON.stringify(body), headers });

const auth = ({ token, device }: AccountSession): Record<string, string> => ({
  Authorization: `Bearer ${token}`, ...(device ? { 'X-Hangar-Device': device.id, 'X-Hangar-Device-Secret': device.secret } : {}),
});

/** Registra este aparelho na conta: nasce pendente e espera a liberação de um aparelho ou PC já liberado. */
export async function registerDevice(session: AccountSession, name: string, installId?: string): Promise<AccountDeviceKey> {
  const { device_id: id, device_secret: secret } = record(await post(session.serviceUrl, '/v1/devices',
    { name, ...(installId ? { install_id: installId } : {}) }, auth(session)));
  if (!filled(id) || !filled(secret)) throw fail('connect_response_invalid');
  return { id, secret };
}

/** Status deste aparelho na conta. 404 = pedido vencido ou desconhecido: registrar de novo. */
export async function deviceStatus(session: AccountSession & { device: AccountDeviceKey }): Promise<AccountDeviceStatus> {
  const { status } = record(await request(session.serviceUrl, `/v1/devices/${encodeURIComponent(session.device.id)}`, { headers: auth(session) }));
  if (!STATUSES.includes(status)) throw fail('connect_response_invalid');
  return status as AccountDeviceStatus;
}

const when = (value: unknown) => typeof value === 'number' && Number.isFinite(value);

/** Aparelhos da conta; só responde a aparelho liberado. */
export async function listAccountDevices(session: AccountSession): Promise<AccountDevice[]> {
  const rows = await request(session.serviceUrl, '/v1/devices', { headers: auth(session) });
  const valid = (item: unknown): item is AccountDevice => {
    const row = record(item);
    return filled(row.device_id) && typeof row.name === 'string' && STATUSES.includes(row.status) && when(row.created_at)
      && (row.approved_at === null || when(row.approved_at)) && (row.revoked_at === null || when(row.revoked_at))
      && (row.install_code === undefined || row.install_code === null || typeof row.install_code === 'string');
  };
  if (!Array.isArray(rows) || !rows.every(valid)) throw fail('connect_response_invalid');
  return rows;
}

export async function approveAccountDevice(session: AccountSession, id: string): Promise<void> {
  await post(session.serviceUrl, `/v1/devices/${encodeURIComponent(id)}/approve`, {}, auth(session));
}

/** Recusa o pendente ou tira o acesso do liberado, em todas as máquinas da conta. */
export async function revokeAccountDevice(session: AccountSession, id: string): Promise<void> {
  await request(session.serviceUrl, `/v1/devices/${encodeURIComponent(id)}`, { method: 'DELETE', headers: auth(session) });
}

/** Tira o computador da conta: a chave dele deixa de valer e ele some da lista de todos os aparelhos. Já fora da conta
 *  conta como feito; erro `connect_machine_remove_unavailable` quando o 404 não veio do serviço. */
export async function removeAccountMachine(session: AccountSession, machineId: string): Promise<void> {
  try {
    await request(session.serviceUrl, `/v1/machines/${encodeURIComponent(machineId)}`, { method: 'DELETE', headers: auth(session) });
  } catch (error) {
    // Só o 404 do próprio serviço diz que a máquina já saiu; o de quem está no caminho não diz nada sobre a conta.
    const { status, fromService } = error as { status?: number; fromService?: boolean };
    if (status !== 404) throw error;
    if (!fromService) throw fail('connect_machine_remove_unavailable', 404);
  }
}

async function accountSession(serviceUrl: string, path: string, name: string, password: string): Promise<AccountSession> {
  const { token } = record(await post(serviceUrl, path, { name, password }));
  if (!filled(token)) throw fail('connect_response_invalid');
  return { serviceUrl, token };
}

export function signUp(serviceUrl: string, name: string, password: string): Promise<AccountSession> {
  return accountSession(serviceUrl, '/v1/accounts', name, password);
}

export function signIn(serviceUrl: string, name: string, password: string): Promise<AccountSession> {
  return accountSession(serviceUrl, '/v1/sessions', name, password);
}

export type NameLookup = { kind: 'account' } | { kind: 'guest'; hosts: string[] } | { kind: 'none' };
export type SignInResult = { kind: 'account'; account: AccountSession } | { kind: 'guest'; host: string; servers: Server[] };
export interface SignInDeps { crypto: SyncCrypto; serviceUrl?: string }

const CONNECT_HOST_SUFFIX = '.hangar.dev.br';
const HOSTNAME = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/;

// Pedido que nem chegou a ter resposta é rede, e a tela não pode tratá-lo como senha errada.
async function overNetwork<T>(call: () => Promise<T>): Promise<T> {
  try {
    return await call();
  } catch (error) {
    if (typeof (error as { code?: unknown }).code === 'string') throw error;
    throw fail('network_failed');
  }
}

/** Pergunta ao serviço só o nome: diz se é conta, convidado (com as máquinas que guardam o cofre dele) ou nada. */
export async function lookupName(name: string, serviceUrl = CONNECT_SERVICE_URL): Promise<NameLookup> {
  let answer: Record<string, unknown>;
  try {
    answer = record(await overNetwork(() => post(serviceUrl, '/v1/lookup', { name: normalizeGuestUser(name) })));
  } catch (error) {
    // Serviço anterior ao diretório de convidados: só conhece conta.
    if ((error as { status?: number }).status === 404) return { kind: 'account' };
    if ((error as { status?: number }).status === 429) throw fail('connect_too_many_attempts', 429);
    throw error;
  }
  const { kind, hosts } = answer;
  if (kind === 'account' || kind === 'none') return { kind };
  if (kind !== 'guest' || !Array.isArray(hosts) || !hosts.length
    || !hosts.every((host) => typeof host === 'string' && HOSTNAME.test(host))) throw fail('connect_response_invalid');
  return { kind: 'guest', hosts };
}

/**
 * Entrada pelo campo único "conta ou usuário". A senha da convidada nunca vai ao serviço: ele só recebe o nome, e o
 * login dela roda numa máquina de cada vez, parando na primeira que aceita.
 * Erros com `code`: `credentials_mismatch` (nome desconhecido ou senha recusada em todas), `network_failed`,
 * `connect_too_many_attempts`/`sync_too_many_attempts` e os demais de `signIn`/`syncSignIn`.
 */
export async function signInAnywhere(name: string, password: string, deps: SignInDeps): Promise<SignInResult> {
  const serviceUrl = deps.serviceUrl ?? CONNECT_SERVICE_URL;
  const user = normalizeGuestUser(name);
  const found = await lookupName(user, serviceUrl);
  if (found.kind === 'none') throw fail('credentials_mismatch');
  if (found.kind === 'account') {
    try {
      return { kind: 'account', account: await overNetwork(() => signIn(serviceUrl, user, password)) };
    } catch (error) {
      if ((error as { status?: number }).status === 401) throw fail('credentials_mismatch', 401);
      throw error;
    }
  }
  // A senha derivada da convidada vai a cada máquina: o serviço não pode apontar uma de fora do Connect.
  if (!found.hosts.every((host) => host.endsWith(CONNECT_HOST_SUFFIX))) throw fail('connect_response_invalid');
  const failures: Error[] = [];
  for (const host of found.hosts) {
    try {
      return { kind: 'guest', host, servers: await syncSignIn(`https://${host}`, user, password, deps.crypto) };
    } catch (error) {
      failures.push(error as Error);
    }
  }
  const codes = failures.map((error) => (error as { code?: unknown }).code);
  // Máquina fora do ar pode ser justamente a que aceitaria a senha: só "não confere" quando todas recusaram.
  const other = failures.find((_, i) => codes[i] !== 'credentials_mismatch' && codes[i] !== 'sync_too_many_attempts');
  if (other) throw other;
  if (codes.includes('sync_too_many_attempts')) throw fail('sync_too_many_attempts', 429);
  throw fail('credentials_mismatch', 401);
}

export async function listMachines(session: AccountSession): Promise<ConnectMachine[]> {
  const machines = await request(session.serviceUrl, '/v1/machines', { headers: { Authorization: `Bearer ${session.token}` } });
  const valid = (item: unknown): item is ConnectMachine => {
    const { machine_id: id, name, host, online } = record(item);
    return filled(id) && filled(name) && filled(host) && typeof online === 'boolean';
  };
  if (!Array.isArray(machines) || !machines.every(valid)) throw fail('connect_response_invalid');
  return machines;
}

/** Encerra todas as sessões da conta, inclusive a deste aparelho. */
export async function endAccountSessions(session: AccountSession): Promise<void> {
  await request(session.serviceUrl, '/v1/sessions', { method: 'DELETE', headers: { Authorization: `Bearer ${session.token}` } });
}

/** Troca a senha. As outras sessões acabam, e a deste aparelho continua valendo. */
export async function changeAccountPassword(session: AccountSession, oldPassword: string, newPassword: string): Promise<void> {
  await post(session.serviceUrl, '/v1/accounts/password', { old_password: oldPassword, new_password: newPassword },
    { Authorization: `Bearer ${session.token}` });
}

// Sem gerador seguro falha: com `Math.random` o serviço, que tem o hash, conseguiria achar o segredo.
function newSecret(): string {
  if (globalThis.crypto?.getRandomValues) {
    const bytes = globalThis.crypto.getRandomValues(new Uint8Array(32));
    return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  }
  const secret = apiEnv().secureRandom?.();
  if (!secret) throw fail('connect_secure_random_unavailable');
  return secret;
}

function machineUrl(raw: unknown, host: string): string {
  const url = filled(raw) ? normalizeMachineUrl(raw) : '';
  if (!url.startsWith('https:') || new URL(url).hostname !== host) throw fail('connect_response_invalid');
  return url;
}

async function waitForApproval(base: string, deviceId: string, secret: string, onPending: () => void): Promise<string> {
  const deadline = Date.now() + APPROVAL_WINDOW_MS;
  // A primeira consulta sai na hora: bilhete pré-aprovado pela conta já entrega o token, sem espera nem aviso.
  for (let polls = 0; Date.now() < deadline; polls++) {
    if (polls === 1) onPending();
    if (polls > 0) await new Promise((resolve) => setTimeout(resolve, APPROVAL_POLL_MS));
    let answer: Record<string, unknown>;
    try {
      answer = record(await post(base, '/api/connect/redeem/status', { device_id: deviceId, secret }));
    } catch (error) {
      // Rede ou máquina instável não é recusa: a aprovação pode chegar logo depois.
      const status = (error as { status?: number }).status;
      if (status !== undefined && status < 500 && status !== 429) throw error;
      continue;
    }
    if (answer.status === 'denied') throw fail('connect_device_denied');
    if (answer.status === 'approved') {
      // O token sai uma vez só: aprovado sem ele é entrega perdida, e esperar não o traz de volta.
      if (!filled(answer.token)) throw fail('connect_token_lost');
      return answer.token;
    }
    if (answer.status !== 'pending') throw fail('connect_response_invalid');
  }
  throw fail('connect_approval_expired');
}

/** Pede a entrada do aparelho na máquina. Liberado na conta, o bilhete já sai aprovado; senão (máquina antiga) espera
 *  a aprovação nela, e `onPending` avisa uma vez que ela foi pedida. */
export async function addMachine(session: AccountSession, machine: ConnectMachine, deviceName: string, onPending: () => void,
  installId?: string): Promise<Server> {
  const secret = newSecret();
  const verifier = sha256Hex(new TextEncoder().encode(secret));
  const ticket = record(await post(session.serviceUrl, `/v1/machines/${encodeURIComponent(machine.machine_id)}/devices`,
    { device_name: deviceName, verifier }, auth(session)));
  const url = machineUrl(ticket.url, machine.host);
  const base = baseOf({ id: SERVIDOR_CANDIDATO, label: machine.name, baseUrl: url, token: '' });
  const { device_id: deviceId } = record(await post(base, '/api/connect/redeem',
    { ticket: ticket.ticket, signature: ticket.signature, secret, device_name: deviceName, ...(installId ? { install_id: installId } : {}) }));
  if (!filled(deviceId)) throw fail('connect_response_invalid');
  const token = await waitForApproval(base, deviceId, secret, onPending);
  const server: Server = { id: machine.machine_id, label: machine.name, baseUrl: url, token, credential: 'device', deviceId };
  try {
    const lan = await readMachineIdentity({ ...server, id: SERVIDOR_CANDIDATO }, { redirect: 'error' });
    return { ...server, lan, ...(lan.id ? { connect: { url, id: lan.id } } : {}) };
  } catch (error) {
    // O token já foi entregue e não sai de novo: sem a identidade a entrada ainda vale, e a rota a aprende depois.
    registrarDiag({ evento: 'connect.identidade_falhou', nivel: 'aviso', codigo: error instanceof Error ? error.name : 'erro' }, url);
    return server;
  }
}

async function enroll(server: Server): Promise<Server> {
  if (!await readConnectUrl(server)) return server;
  const identity = apiEnv().deviceIdentity?.();
  const name = (apiEnv().deviceName?.()?.trim() || `Hangar ${identity?.kind ?? 'app'}`).slice(0, 64);
  // A mesma instalação pedindo de novo volta à entrada dela na máquina, em vez de somar outra.
  const response = await probeServerResponse(server, '/api/connect/devices/self', {
    method: 'POST', body: JSON.stringify({ device_name: name, ...(identity ? { install_id: identity.id } : {}) }), redirect: 'error',
  });
  if (!response.ok) throw fail('connect_enroll_failed', response.status);
  const { device_id: deviceId, token } = record(await response.json());
  if (!filled(deviceId) || !filled(token)) throw fail('connect_response_invalid');
  // A senha fica guardada: 401 temporário do aparelho volta a ela em vez de perder a máquina.
  return { ...server, ownerToken: server.token, token, credential: 'device', deviceId };
}

const upgrades = new Map<string, Promise<Server>>();

/**
 * Troca a senha do dono por credencial de aparelho quando a máquina tem Connect. O token novo vale
 * também na rede local e no Tailscale. Nunca rejeita: falha mantém a senha, e sem Connect nada muda.
 */
export function upgradeToDevice(server: Server): Promise<Server> {
  if (server.credential === 'device' || server.connectOff || !isOwnMachine(server)) return Promise.resolve(server);
  const pending = upgrades.get(server.id);
  if (pending) return pending;
  const upgrade = enroll(server).catch((error: unknown) => {
    const code = (error as { code?: unknown }).code;
    registrarDiag({ evento: 'connect.aparelho_falhou', nivel: 'aviso', codigo: typeof code === 'string' ? code : 'erro' }, server.baseUrl);
    return server;
  }).finally(() => { upgrades.delete(server.id); });
  upgrades.set(server.id, upgrade);
  return upgrade;
}
