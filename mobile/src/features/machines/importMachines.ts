import {
  confirmImportedMachine, discoverMachineCandidates, isOwnMachine, machineImportIssue, mergeImportedMachines, parseMachineImportPayload, readMachineIdentity,
  registrarDiag, type MachineImportIssue, type MachineImportResult, type Server,
} from '@hangar/core';
import { useServers } from '../../stores/servers';
import { prefs } from '../../stores/prefs';
import { toast } from '../../ui/Toast';
import * as m from '../../paraglide/messages';

const warningKey = 'cp_machine_discovery_warned';

export function machineImportIssueMessage(issue: MachineImportIssue): string {
  const messages = {
    source_peers: m.machine_import_source_peers, source_app_list: m.machine_import_source_app_list,
    peer_token: m.machine_import_peer_token, saved_identity: m.machine_import_saved_identity,
    candidate: m.machine_import_candidate, invalid_item: m.machine_import_invalid_item,
    device_source: m.machine_import_device_source,
  };
  let reason = `${messages[issue.stage]()}${issue.status ? ` (HTTP ${issue.status})` : ''}`;
  if (issue.machine) reason = m.machine_import_issue_machine({ machine: issue.machine, reason });
  if (issue.source) reason = m.machine_import_issue_source({ source: issue.source, reason });
  return reason;
}

async function ensureHydrated(): Promise<void> {
  if (!useServers.getState().vaultReadable) await useServers.getState().load();
  if (!useServers.getState().vaultReadable) throw new Error('machine_import_vault_unread');
}

export async function importMachines(candidates: Server[]): Promise<MachineImportResult> {
  await ensureHydrated();
  const issues: MachineImportIssue[] = [];
  const valid = candidates.flatMap((server) => {
    try { return parseMachineImportPayload(JSON.stringify([server])); }
    catch { issues.push(machineImportIssue('invalid_item', undefined, server.baseUrl)); return []; }
  });
  const newUrls = new Set(mergeImportedMachines(useServers.getState(), valid).added.map((s) => s.baseUrl));
  const pending = new Map<string, Server[]>();
  for (const server of valid) {
    if (!newUrls.has(server.baseUrl)) continue;
    const alternatives = pending.get(server.baseUrl) ?? [];
    if (!alternatives.some((s) => s.token === server.token)) alternatives.push(server);
    pending.set(server.baseUrl, alternatives);
  }
  if (!pending.size) return { added: [], issues };
  const saved = useServers.getState().servers.filter(isOwnMachine);
  await Promise.all(saved.map(async (captured) => {
    try {
      const lan = await readMachineIdentity(captured);
      if (!lan.id) throw new Error('machine_import_identity_empty');
      await ensureHydrated();
      const current = useServers.getState().servers.find((server) => server.id === captured.id);
      if (current && isOwnMachine(current) && current.baseUrl === captured.baseUrl && current.token === captured.token
        && (current.lan?.id !== lan.id || current.lan?.url !== lan.url)) {
        useServers.getState().rememberLan(current.id, lan);
      }
    } catch (error) { issues.push(machineImportIssue('saved_identity', error, captured.baseUrl)); }
  }));
  const results = await Promise.all([...pending.values()].map(async (alternatives) => {
    for (const server of alternatives) {
      try { return await confirmImportedMachine(server); }
      catch (error) { issues.push(machineImportIssue('candidate', error, server.baseUrl)); }
    }
    return null;
  }));
  // Revalida após a rede e conserva a ordem da lista, não a velocidade das respostas.
  await ensureHydrated();
  const confirmed = results.filter((server): server is Server => server !== null);
  return { added: confirmed.length ? useServers.getState().appendImported(confirmed) : [], issues };
}

function shortHost(host: string | undefined): string {
  if (!host) return '';
  let name = host;
  try { name = new URL(host.includes('://') ? host : `http://${host}`).hostname; } catch { /* fica o texto recebido */ }
  return name.replace(/\.tail[0-9a-z]+\.ts\.net$/i, '');
}

export async function discoverSavedMachines(sources?: Server[]): Promise<MachineImportResult> {
  let result: MachineImportResult;
  try {
    await ensureHydrated();
    const snapshot = [...(sources ?? useServers.getState().servers)];
    const discovered = await discoverMachineCandidates(snapshot, () => useServers.getState().servers);
    const imported = await importMachines(discovered.candidates);
    result = { added: imported.added, issues: [...discovered.issues, ...imported.issues] };
  } catch {
    toast.passive(useServers.getState().vaultReadable ? m.mobile_machines_invalid_response() : m.servers_warning_vault_unread());
    return { added: [], issues: [{ stage: 'invalid_item' }] };
  }
  const savedHosts = new Set(useServers.getState().servers.map((server) => new URL(server.baseUrl).host));
  let warned: string[] = [];
  try {
    const stored: unknown = JSON.parse(prefs.getString(warningKey) ?? '[]');
    if (Array.isArray(stored)) warned = stored.filter((host): host is string => typeof host === 'string');
  } catch { registrarDiag({ evento: 'machines.warning_unreadable', nivel: 'aviso' }); }
  // Falha da fonte não prova queda da máquina; outra credencial pode ter confirmado a candidata.
  const hosts = [...new Set(result.issues.filter((issue) => issue.stage === 'candidate' || issue.stage === 'peer_token')
    .map((issue) => issue.machine).filter((host): host is string => !!host && !savedHosts.has(host) && !warned.includes(host)))];
  if (hosts.length) {
    prefs.set(warningKey, JSON.stringify([...warned, ...hosts]));
    toast.passive(m.machine_discovery_unconfirmed({ hosts: hosts.map(shortHost).join(', ') }));
  }
  return result;
}
