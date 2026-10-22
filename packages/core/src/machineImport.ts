import { comTeto, getSessions, probeServerResponse } from './api';
import { SERVIDOR_CANDIDATO } from './esfriamento';
import type { LanInfo, Server } from './servers';

const MAX_MACHINES = 100;
const MAX_PAYLOAD = 262_144;
const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const invalid = () => new Error('machine_import_invalid_payload');
const text = (value: unknown, max: number): value is string => typeof value === 'string'
  && !!value.trim() && value.length <= max && !/[\u0000-\u001f\u007f]/.test(value);

export const isOwnMachine = (server: Pick<Server, 'invite' | 'ephemeral' | 'removed'>): boolean =>
  !server.invite && !server.ephemeral && !server.removed;

export function normalizeMachineUrl(raw: string): string {
  const value = raw.trim();
  const url = new URL(value);
  if (!['http:', 'https:'].includes(url.protocol) || !url.hostname || url.username || url.password
    || /[?#\s\\]/.test(value) || value.length > 2048) throw invalid();
  return url.toString().replace(/\/+$/, '');
}

export function isLocalMachineUrl(raw: string): boolean {
  const host = new URL(raw).hostname.toLowerCase().replace(/^\[|\]$/g, '').replace(/\.$/, '');
  return host === 'localhost' || host.endsWith('.localhost') || host === '::1' || host === '::'
    || host === '0.0.0.0' || host === '*' || host === '+' || /^127\./.test(host)
    || /^::(?:ffff:)?(127\.|7f[0-9a-f]{2}:)/.test(host) || host === '::ffff:0:0';
}

function candidate(value: unknown): Server | null {
  if (!object(value)) throw invalid();
  for (const flag of ['invite', 'ephemeral', 'removed', 'disabled', 'guest']) {
    if (value[flag] !== undefined && typeof value[flag] !== 'boolean') throw invalid();
  }
  if (value.invite || value.ephemeral || value.removed || value.guest) return null;
  if (!text(value.id, 128) || /\s/.test(value.id) || !text(value.label, 256)
    || !text(value.token, 8192) || /\s/.test(value.token) || typeof value.baseUrl !== 'string') throw invalid();
  const baseUrl = normalizeMachineUrl(value.baseUrl);
  if (isLocalMachineUrl(baseUrl)) return null;
  return { id: value.id, label: value.label, baseUrl, token: value.token,
    ...(typeof value.disabled === 'boolean' ? { disabled: value.disabled } : {}) };
}

/** Recebe o JSON já decodificado pelo roteador; o token pode conter percentuais literais. */
export function parseMachineImportPayload(raw: string): Server[] {
  try {
    if (typeof raw !== 'string' || raw.length > MAX_PAYLOAD) throw invalid();
    const value: unknown = JSON.parse(raw);
    if (!Array.isArray(value) || value.length > MAX_MACHINES) throw invalid();
    const seen = new Set<string>();
    return value.map(candidate).filter((server): server is Server => {
      if (!server || seen.has(server.baseUrl)) return false;
      seen.add(server.baseUrl);
      return true;
    });
  } catch { throw invalid(); }
}

export function createMachineImportLink(servers: Server[]): string {
  const payload = servers.filter(isOwnMachine).map(({ id, label, baseUrl, token }) => ({ id, label, baseUrl, token }));
  const validated = parseMachineImportPayload(JSON.stringify(payload));
  return `hangar://importar#${encodeURIComponent(JSON.stringify(validated))}`;
}

export type MachineImportIssue = {
  stage: 'source_peers' | 'source_app_list' | 'peer_token' | 'saved_identity' | 'candidate' | 'invalid_item';
  status?: number; machine?: string; source?: string;
};
export type MachineImportResult = { added: Server[]; issues: MachineImportIssue[] };

export function machineImportIssue(stage: MachineImportIssue['stage'], error: unknown, baseUrl?: string, sourceUrl?: string): MachineImportIssue {
  const host = (url?: string) => {
    try { return url ? new URL(normalizeMachineUrl(url)).host : undefined; }
    catch { return undefined; }
  };
  const status = object(error) ? error.status : undefined;
  const machine = host(baseUrl);
  const source = host(sourceUrl);
  return { stage,
    ...(typeof status === 'number' && Number.isInteger(status) && status >= 100 && status <= 599 ? { status } : {}),
    ...(machine ? { machine } : {}), ...(source ? { source } : {}),
  };
}

async function readImportJson(server: Server, path: string, init?: RequestInit): Promise<unknown> {
  const response = await probeServerResponse(server, path, init);
  // Não incorpora o corpo do erro: ele pode ecoar o token que recebeu.
  if (!response.ok) throw Object.assign(new Error('machine_import_request_failed'), { status: response.status });
  return response.json();
}

export async function readMachineIdentity(server: Server, init?: RequestInit): Promise<LanInfo> {
  const identity = await readImportJson(server, '/api/peers/identificador', init);
  if (!object(identity) || typeof identity.identificador !== 'string' || identity.identificador.length > 256
    || /[\u0000-\u001f\u007f]/.test(identity.identificador)
    || (identity.lan_url !== undefined && typeof identity.lan_url !== 'string')) throw invalid();
  return { id: identity.identificador.trim(), url: identity.lan_url ? normalizeMachineUrl(identity.lan_url as string) : '' };
}

/** Intervalo para reler o identificador de agrupamento depois de uma falha. */
export const GROUP_IDENTITY_RETRY_MS = 30_000;

/** Identificador da máquina para agrupar entre máquinas. Vazio (máquina sem CP_SERVER_ID) é falha, com `name`
 * `vazio`: quem chama relê no mesmo intervalo de qualquer falha, para o conserto aparecer sem reconectar. */
export async function readGroupIdentity(server: Server, signal?: AbortSignal): Promise<string> {
  const { id } = await readMachineIdentity(server, signal ? { signal: comTeto(signal, 8000) } : undefined);
  if (!id) throw Object.assign(new Error('machine_identity_empty'), { name: 'vazio' });
  return id;
}

export async function confirmImportedMachine(server: Server): Promise<Server> {
  const target = { ...server, id: SERVIDOR_CANDIDATO };
  const sessions: unknown = await getSessions(target, { redirect: 'error' });
  if (!Array.isArray(sessions)) throw invalid();
  const lan = await readMachineIdentity(target, { redirect: 'error' });
  return { ...server, label: lan.id || server.label, lan };
}

export async function discoverMachineCandidates(sources: Server[], current: () => Server[]): Promise<{ candidates: Server[]; issues: MachineImportIssue[] }> {
  const candidates: Server[] = [];
  const issues: MachineImportIssue[] = [];
  const known = (baseUrl: string) => current().some((s) => {
    try { return normalizeMachineUrl(s.baseUrl) === baseUrl; } catch { return false; }
  });
  const include = (server: Server | null) => {
    if (server && !known(server.baseUrl) && !candidates.some((s) => s.baseUrl === server.baseUrl && s.token === server.token)) candidates.push(server);
  };
  const peerJobs: { source: Server; id: string; baseUrl: string }[] = [];
  // Só estas fontes participam: candidatas novas nunca entram nesta rodada.
  await Promise.all(sources.filter(isOwnMachine).map(async (source) => {
    await Promise.all([
      (async () => {
        try {
          const peers = await readImportJson(source, '/api/peers');
          if (!Array.isArray(peers) || peers.length > MAX_MACHINES) throw invalid();
          for (const peer of peers) {
            try {
              if (!object(peer)) throw invalid();
              if (peer.enabled === false || peer.invite === true || peer.ephemeral === true || peer.removed === true) continue;
              if (!text(peer.id, 128) || /\s/.test(peer.id) || typeof peer.base_url !== 'string'
                || (peer.enabled !== undefined && typeof peer.enabled !== 'boolean')) throw invalid();
              const baseUrl = normalizeMachineUrl(peer.base_url);
              if (!isLocalMachineUrl(baseUrl)) peerJobs.push({ source, id: peer.id, baseUrl });
            } catch { issues.push(machineImportIssue('invalid_item', undefined, source.baseUrl)); }
          }
        } catch (error) { issues.push(machineImportIssue('source_peers', error, source.baseUrl)); }
      })(),
      (async () => {
        try {
          const list = await readImportJson(source, '/api/peers/app-list');
          if (!object(list) || typeof list.rev !== 'string' || !Array.isArray(list.servers) || list.servers.length > MAX_MACHINES) throw invalid();
          for (const item of list.servers) {
            try {
              if (!object(item)) throw invalid();
              const server = candidate({ ...item, baseUrl: item.address });
              include(server);
            } catch { issues.push(machineImportIssue('invalid_item', undefined, source.baseUrl)); }
          }
        } catch (error) { issues.push(machineImportIssue('source_app_list', error, source.baseUrl)); }
      })(),
    ]);
  }));
  await Promise.all(peerJobs.map(async ({ source, id, baseUrl }) => {
    if (known(baseUrl)) return;
    try {
      const response = await readImportJson(source, `/api/peers/${encodeURIComponent(id)}/token`);
      const server = candidate({ id, label: id, baseUrl, token: object(response) ? response.token : undefined });
      include(server);
    } catch (error) { issues.push(machineImportIssue('peer_token', error, baseUrl, source.baseUrl)); }
  }));
  return { candidates, issues };
}

export function mergeImportedMachines(state: { servers: Server[]; activeId: string | null }, incoming: Server[]) {
  const key = (raw: string) => { try { return normalizeMachineUrl(raw); } catch { return raw.replace(/\/+$/, ''); } };
  const seen = new Set(state.servers.map((s) => key(s.baseUrl)));
  const identities = new Set(state.servers.filter(isOwnMachine).map((s) => s.lan?.id.trim()).filter((id): id is string => !!id));
  const ids = new Set([SERVIDOR_CANDIDATO, ...state.servers.map((s) => s.id)]);
  const added: Server[] = [];
  const updated = new Map<string, Server>();
  for (const server of incoming.filter(isOwnMachine)) {
    const baseUrl = key(server.baseUrl);
    const identity = server.lan?.id.trim();
    if (seen.has(baseUrl)) continue;
    if (identity && identities.has(identity)) {
      const existing = [...state.servers, ...added].find((s) => isOwnMachine(s) && s.lan?.id === identity);
      const host = new URL(baseUrl).hostname;
      const tailnetIp = /^100\.(\d+)\.\d+\.\d+$/.exec(host);
      const remoteAddress = host.endsWith('.ts.net') || !!tailnetIp && Number(tailnetIp[1]) >= 64 && Number(tailnetIp[1]) <= 127;
      if (existing && existing.token === server.token && !existing.remote
        && remoteAddress) {
        const replacement = { ...existing, remote: { url: baseUrl, id: identity } };
        const index = added.findIndex((s) => s.id === existing.id);
        if (index >= 0) added[index] = replacement;
        else updated.set(existing.id, replacement);
      }
      continue;
    }
    let id = server.id;
    for (let suffix = 2; ids.has(id); suffix++) id = `${server.id}_${suffix}`;
    added.push({ ...server, id, baseUrl });
    seen.add(baseUrl);
    if (identity) identities.add(identity);
    ids.add(id);
  }
  return {
    servers: added.length || updated.size ? [...state.servers.map((s) => updated.get(s.id) ?? s), ...added] : state.servers,
    activeId: state.servers.some((s) => s.id === state.activeId) ? state.activeId : added[0]?.id ?? state.activeId,
    added,
  };
}
