// Conta do Connect e entrada de aparelho. O serviço só apresenta aparelho e máquina: o `secret` vai
// direto à máquina, e ao serviço só o hash dele.
import { probeServerResponse } from './api';
import { apiEnv } from './apiEnv';
import { registrar as registrarDiag } from './diag';
import { SERVIDOR_CANDIDATO } from './esfriamento';
import { sha256Hex } from './hmac';
import { isOwnMachine, normalizeMachineUrl, readMachineIdentity } from './machineImport';
import { baseOf, readConnectUrl } from './rota';
import type { Server } from './servers';

const REQUEST_TIMEOUT_MS = 15_000;
const APPROVAL_POLL_MS = 3000;
// O pedido pendente expira neste prazo na máquina (`PENDING_TTL`).
const APPROVAL_WINDOW_MS = 10 * 60_000;

export interface AccountSession { serviceUrl: string; token: string }
export interface ConnectMachine { machine_id: string; name: string; host: string; online: boolean }

const fail = (code: string, status?: number) => Object.assign(new Error(code), { code, status });
const record = (value: unknown): Record<string, unknown> => (value && typeof value === 'object' && !Array.isArray(value)
  ? value as Record<string, unknown> : {});
const filled = (value: unknown): value is string => typeof value === 'string' && !!value;

async function request(base: string, path: string, init: RequestInit): Promise<unknown> {
  const response = await fetch(`${base.replace(/\/+$/, '')}${path}`, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...init.headers },
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS), redirect: 'error', credentials: 'omit',
  });
  // Não incorpora o corpo do erro: ele pode ecoar o que recebeu.
  if (!response.ok) throw fail('connect_request_failed', response.status);
  return await response.json() as unknown;
}

const post = (base: string, path: string, body: unknown, headers?: Record<string, string>) =>
  request(base, path, { method: 'POST', body: JSON.stringify(body), headers });

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
  if (!Array.isArray(machines)) throw fail('connect_response_invalid');
  return machines as ConnectMachine[];
}

// Sem gerador seguro falha: com `Math.random` o serviço, que tem o hash, conseguiria achar o segredo.
function newSecret(): string {
  if (!globalThis.crypto?.getRandomValues) throw fail('connect_secure_random_unavailable');
  const bytes = globalThis.crypto.getRandomValues(new Uint8Array(32));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

function machineUrl(raw: unknown): string {
  const url = filled(raw) ? normalizeMachineUrl(raw) : '';
  if (!url.startsWith('https:')) throw fail('connect_response_invalid');
  return url;
}

async function waitForApproval(base: string, deviceId: string, secret: string): Promise<string> {
  const deadline = Date.now() + APPROVAL_WINDOW_MS;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, APPROVAL_POLL_MS));
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
  }
  throw fail('connect_approval_expired');
}

/** Pede a entrada do aparelho e espera a aprovação na máquina. `onPending` avisa uma vez que ela foi pedida. */
export async function addMachine(session: AccountSession, machine: ConnectMachine, deviceName: string, onPending: () => void): Promise<Server> {
  const secret = newSecret();
  const verifier = sha256Hex(new TextEncoder().encode(secret));
  const ticket = record(await post(session.serviceUrl, `/v1/machines/${encodeURIComponent(machine.machine_id)}/devices`,
    { device_name: deviceName, verifier }, { Authorization: `Bearer ${session.token}` }));
  const url = machineUrl(ticket.url);
  const base = baseOf({ id: SERVIDOR_CANDIDATO, label: machine.name, baseUrl: url, token: '' });
  const { device_id: deviceId } = record(await post(base, '/api/connect/redeem',
    { ticket: ticket.ticket, signature: ticket.signature, secret, device_name: deviceName }));
  if (!filled(deviceId)) throw fail('connect_response_invalid');
  onPending();
  const token = await waitForApproval(base, deviceId, secret);
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
  return { ...server, token, credential: 'device', deviceId };
}

const upgrades = new Map<string, Promise<Server>>();

/**
 * Troca a senha do dono por credencial de aparelho quando a máquina tem Connect. O token novo vale
 * também na rede local e no Tailscale. Nunca rejeita: falha mantém a senha, e sem Connect nada muda.
 */
export function upgradeToDevice(server: Server): Promise<Server> {
  if (server.credential === 'device' || !isOwnMachine(server)) return Promise.resolve(server);
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
