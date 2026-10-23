import { useEffect, useState } from 'react';
import { baseOf, comTeto, decidirRota, esquecerRota, type LanInfo, type Server } from '@hangar/core';
import { ownServers, useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { confirmMachineRoute, discover, ensureActive, normalizeUrl, readReach, reason, type Discovered } from './api';
import { MachineField, MachineGroup, MachineHelp, MachineRow, type MachineView } from './MachineRows';
import { MachinePage } from './MachinePage';
import { Notice, useMachineAction } from './parts';
import * as m from '../../paraglide/messages';

function saveRemote(server: Server, remote: LanInfo) {
  const store = useServers.getState();
  const current = store.servers.find((s) => s.id === server.id);
  if (!current || current.invite || current.baseUrl !== server.baseUrl || current.token !== server.token
    || current.lan?.id && current.lan.id !== remote.id) throw new Error(m.mobile_machines_entry_changed());
  if (ownServers(store.servers).some((s) => s.id !== server.id && (s.baseUrl === remote.url || s.remote?.url === remote.url))) {
    throw new Error(m.mobile_machines_duplicate_address());
  }
  if (!current.lan?.id) store.rememberLan(server.id, { id: remote.id, url: current.lan?.url || '' }, server);
  store.rememberRemote(server.id, remote, server);
  const saved = useServers.getState().servers.find((s) => s.id === server.id)?.remote;
  if (saved?.url !== remote.url || saved.id !== remote.id) throw new Error(m.mobile_machines_entry_changed());
  esquecerRota(server.id);
  const linked = useServers.getState().servers.find((s) => s.id === server.id);
  if (linked) void decidirRota(linked).then(() => useSessions.getState().reconnect());
}

export function MachineConnection({ server, view, onNavigate }: {
  server: Server;
  view: 'connection' | 'discover' | 'remote';
  onNavigate: (view: MachineView) => void;
}) {
  if (view === 'discover') return <MachineDiscovery server={server} />;
  if (view === 'remote') return <RemoteAddress server={server} />;
  return <ConnectionOverview server={server} onNavigate={onNavigate} />;
}

function ConnectionOverview({ server, onNavigate }: { server: Server; onNavigate: (view: MachineView) => void }) {
  const bucket = useSessions((state) => state.byServer.find((entry) => entry.server.id === server.id));
  const route = baseOf(server);
  const remote = server.remote?.url || (/\.ts\.net(?::|\/|$)/.test(server.baseUrl) ? server.baseUrl : '');
  const local = server.lan?.url || '';
  const live = !!bucket?.loaded && !bucket.error;
  return <MachinePage title={m.mobile_machines_connection_title()}>
    <MachineGroup>
      <MachineRow title={m.mobile_machines_local_address()} description={local || m.mobile_machines_not_discovered()}
        value={live && route === local ? m.mobile_machines_in_use() : undefined} />
      <MachineRow title={m.mobile_machines_remote_address()} description={remote || m.mobile_machines_not_discovered()}
        value={live && route === remote ? m.mobile_machines_in_use() : undefined} />
      {server.baseUrl !== local && server.baseUrl !== remote ? <MachineRow title={m.mobile_machines_saved_address()}
        description={server.baseUrl} value={live && route === server.baseUrl ? m.mobile_machines_in_use() : undefined} /> : null}
      <MachineRow title={m.mobile_machines_discover()} action onPress={() => onNavigate('discover')} />
      <MachineRow title={m.mobile_machines_remote_edit()} onPress={() => onNavigate('remote')} />
    </MachineGroup>
    <MachineHelp>{m.mobile_machines_network_help()}</MachineHelp>
  </MachinePage>;
}

function MachineDiscovery({ server }: { server: Server }) {
  const [found, setFound] = useState<Discovered[] | null>(null);
  const [source, setSource] = useState('');
  const [partial, setPartial] = useState('');
  const search = useMachineAction();
  const link = useMachineAction();
  const remote = useServers((state) => state.servers.find((s) => s.id === server.id)?.remote);
  const start = () => search.run(async (signal) => {
    const store = useServers.getState();
    const buckets = useSessions.getState().byServer;
    const candidates = ownServers(store.servers).filter((s) => !s.disabled && !s.ephemeral && buckets.some((b) => b.server.id === s.id
      && b.server.baseUrl === s.baseUrl && b.server.token === s.token && b.loaded && !b.error))
      .sort((a, b) => Number(b.id === server.id) - Number(a.id === server.id) || Number(b.id === store.activeId) - Number(a.id === store.activeId));
    if (!candidates.length) throw new Error(m.mobile_machines_discovery_no_server());
    const deadline = comTeto(signal, 30_000);
    const searches = candidates.map((candidate) => ({ name: candidate.label, run: () => discover(candidate, deadline) }));
    if (candidates.some((candidate) => candidate.id === server.id)) {
      searches.push({ name: server.label, run: async () => {
        const reach = await readReach(server, deadline);
        return reach.enderecos.filter((address) => address.tipo === 'tailscale' && address.url)
          .map((address) => ({ nome: server.label, base_url: normalizeUrl(address.url) }));
      } });
    }
    const results = await Promise.allSettled(searches.map((search) => search.run()));
    ensureActive(signal);
    const machines = new Map<string, Discovered>();
    const names = new Set<string>();
    const failures: string[] = [];
    results.forEach((result, index) => {
      if (result.status === 'rejected') failures.push(`${searches[index].name}: ${reason(result.reason)}`);
      else {
        names.add(searches[index].name);
        result.value.forEach((machine) => {
          const hosts = new Set([new URL(machine.base_url).hostname, ...machine.hosts ?? []].map((host) => host.toLowerCase().replace(/\.$/, '')));
          const exists = [...machines.values()].some((saved) => [new URL(saved.base_url).hostname, ...saved.hosts ?? []]
            .some((host) => hosts.has(host.toLowerCase().replace(/\.$/, ''))));
          if (!exists) machines.set(machine.base_url, machine);
        });
      }
    });
    if (!names.size) throw new Error(failures.join('\n'));
    return { machines: [...machines.values()], name: [...names].join(', '), failures: failures.join('\n') };
  }, ({ machines, name, failures }) => { setFound(machines); setSource(name); setPartial(failures); });
  useEffect(() => { void start(); }, []);
  const busy = search.busy || link.busy;
  return <MachinePage title={m.mobile_machines_discover()}>
    {search.busy ? <Notice text={m.mobile_machines_searching()} /> : null}
    <Notice text={search.error || link.error} error /><Notice text={link.message} />
    {partial ? <Notice text={m.mobile_machines_search_partial({ reason: partial })} error /> : null}
    {found?.length ? <MachineGroup>
      {found.map((machine) => <MachineRow key={machine.base_url} title={machine.nome} description={machine.base_url}
        disabled={busy} selected={remote?.url === machine.base_url}
        onPress={() => link.run((signal) => confirmMachineRoute(server, machine.base_url, signal),
          (target) => saveRemote(server, target), m.mobile_machines_route_linked({ name: server.label }))} />)}
    </MachineGroup> : found ? <Notice text={m.mobile_machines_discover_empty()} /> : null}
    {source ? <MachineHelp>{m.mobile_machines_discovery_source({ name: source })}</MachineHelp> : null}
    <MachineHelp>{m.mobile_machines_discovery_link_help({ name: server.label })}</MachineHelp>
    <MachineGroup><MachineRow title={m.mobile_machines_retry()} action disabled={busy} onPress={start} /></MachineGroup>
  </MachinePage>;
}

function RemoteAddress({ server }: { server: Server }) {
  const initial = server.remote?.url || '';
  const [address, setAddress] = useState(initial);
  const action = useMachineAction();
  const save = () => action.run((signal) => confirmMachineRoute(server, normalizeUrl(address), signal),
    (target) => saveRemote(server, target), m.mobile_machines_route_linked({ name: server.label }));
  return <MachinePage title={m.mobile_machines_remote_address()} save={{ onPress: save, disabled: action.busy || !address.trim() || address === initial, busy: action.busy }}>
    <MachineGroup><MachineField label={m.mobile_machines_endpoint()} value={address} onChangeText={setAddress} keyboardType="url" stacked editable={!action.busy} /></MachineGroup>
    <MachineHelp>{m.mobile_machines_discovery_link_help({ name: server.label })}</MachineHelp>
    <Notice text={action.error} error /><Notice text={action.message} />
  </MachinePage>;
}
