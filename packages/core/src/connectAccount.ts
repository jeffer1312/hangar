// Conta do Connect e entrada de aparelho. O `secret` vai direto à máquina e ao serviço só o hash
// dele: isso protege de um serviço honesto que vaze. Um serviço desonesto pode pré-aprovar bilhetes:
// limite aceito em docs/decisoes/plataforma.md ("Confiança na conta").
import { probeServerResponse } from './api';
import { apiEnv } from './apiEnv';
import { registrar as registrarDiag } from './diag';
import { SERVIDOR_CANDIDATO } from './esfriamento';
import { sha256Hex } from './hmac';
import { isLocalMachineUrl, isOwnMachine, normalizeMachineUrl, readMachineIdentity } from './machineImport';
import { baseOf, readConnectUrl } from './rota';
import type { Server } from './servers';

const REQUEST_TIMEOUT_MS = 15_000;
const APPROVAL_POLL_MS = 3000;
// O pedido pendente expira neste prazo na máquina (`PENDING_TTL`).
const APPROVAL_WINDOW_MS = 10 * 60_000;

/** Credencial deste aparelho na conta (`POST /v1/devices`); vai ao serviço, nunca à máquina. */
export interface AccountDeviceKey { id: string; secret: string }
export interface AccountSession { serviceUrl: string; token: string; device?: AccountDeviceKey }
export interface ConnectMachine { machine_id: string; name: string; host: string; online: boolean }
export type AccountDeviceStatus = 'pending' | 'approved' | 'revoked';
/** Linha de `GET /v1/devices`; tempos em segundos. */
export interface AccountDevice {
  device_id: string; name: string; status: AccountDeviceStatus; created_at: number; approved_at: number | null; revoked_at: number | null;
}

const fail = (code: string, status?: number) => Object.assign(new Error(code), { code, status });
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
    const reason = await response.json().then((body: unknown) => record(body).reason, () => undefined);
    throw fail(REASONS.includes(reason) ? `connect_${String(reason)}` : 'connect_request_failed', response.status);
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
      && (row.approved_at === null || when(row.approved_at)) && (row.revoked_at === null || when(row.revoked_at));
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
  const name = `Hangar ${apiEnv().deviceIdentity?.()?.kind ?? 'app'}`;
  const response = await probeServerResponse(server, '/api/connect/devices/self', {
    method: 'POST', body: JSON.stringify({ device_name: name }), redirect: 'error',
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
