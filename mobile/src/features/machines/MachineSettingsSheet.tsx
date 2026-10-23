import { useEffect, useRef, useState } from 'react';
import { Alert } from 'react-native';
import { baseOf, getConfigForServer, type Server } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { Notice, useMachineAction } from './parts';
import { confirmMachineRoute, normalizeUrl } from './api';
import { MachineField, MachineGroup, MachineRow, type MachineView } from './MachineRows';
import { MachinePage } from './MachinePage';
import { MachineConnection } from './MachineConnection';
import { ServerSettings } from './ServerSettings';
import { MachineOperations } from './MachineOperations';
import { PeerSettings } from './PeerSettings';
import * as m from '../../paraglide/messages';

export function useForgetMachine(onForgot?: () => void) {
  const alive = useRef(true);
  const pending = useRef(false);
  useEffect(() => { alive.current = true; return () => { alive.current = false; pending.current = false; }; }, []);
  return (server: Server) => {
    if (!alive.current || pending.current) return;
    pending.current = true;
    const cancel = () => { pending.current = false; };
    Alert.alert(m.config_servidores_remover({ nome: server.label }), m.mobile_machines_leave_body({ name: server.label }), [
      { text: m.comum_cancelar(), style: 'cancel', onPress: cancel },
      { text: m.mobile_machines_leave(), style: 'destructive', onPress: () => {
        if (!alive.current || !pending.current) return;
        pending.current = false;
        useServers.getState().remove(server.id); onForgot?.();
      } },
    ], { cancelable: true, onDismiss: cancel });
  };
}

export function MachineSettings({ server, view, onNavigate, onClose }: {
  server: Server; view: MachineView; onNavigate: (view: MachineView) => void; onClose: () => void;
}) {
  if (view === 'connection' || view === 'discover' || view === 'remote') return <MachineConnection server={server} view={view} onNavigate={onNavigate} />;
  if (view === 'identity' || view === 'terminal' || view === 'diagnostics') return <ServerSettings server={server} view={view} />;
  if (view === 'peers') return <MachinePage title={m.mobile_machines_peers_title()}><PeerSettings server={server} /></MachinePage>;
  if (view === 'maintenance') return <MachinePage title={m.mobile_machines_maintenance()}><MachineOperations server={server} /></MachinePage>;
  if (view === 'advanced') return <MachinePage title={m.mobile_machines_advanced_title()}>
    <MachineGroup>
      <MachineRow title={m.mobile_machines_identity_title()} onPress={() => onNavigate('identity')} />
      <MachineRow title={m.mobile_machines_terminal_title()} onPress={() => onNavigate('terminal')} />
      <MachineRow title={m.mobile_machines_diagnostics_title()} onPress={() => onNavigate('diagnostics')} />
      <MachineRow title={m.mobile_machines_peers_title()} onPress={() => onNavigate('peers')} />
      <MachineRow title={m.mobile_machines_maintenance()} onPress={() => onNavigate('maintenance')} />
    </MachineGroup>
  </MachinePage>;
  return <MachineEditor server={server} onNavigate={onNavigate} onClose={onClose} />;
}

function MachineEditor({ server, onNavigate, onClose }: {
  server: Server; onNavigate: (view: MachineView) => void; onClose: () => void;
}) {
  const [label, setLabel] = useState(server.label);
  const [address, setAddress] = useState(server.baseUrl);
  const connection = useSessions((state) => state.byServer.find((entry) => entry.server.id === server.id));
  const route = baseOf(server);
  const status = connection?.error ? connection.error === 'offline' ? m.mobile_machines_offline() : connection.error : !connection?.loaded ? m.mobile_machines_connection_pending()
    : route === server.lan?.url ? m.mobile_machines_local_address()
    : route === server.remote?.url || /\.ts\.net(?::|\/|$)/.test(route) ? m.mobile_machines_remote_address()
    : m.mobile_machines_saved_address();
  const local = useMachineAction();
  const forget = useForgetMachine(onClose);
  const saveLocal = () => local.run(async (signal) => {
    const baseUrl = normalizeUrl(address);
    const confirmedIdentity = baseUrl !== server.baseUrl ? (await confirmMachineRoute(server, baseUrl, signal)).id : undefined;
    return { label: label.trim() || server.label, baseUrl, confirmedIdentity };
  }, (patch) => {
    const current = useServers.getState().servers.find((s) => s.id === server.id);
    if (!current || current.baseUrl !== server.baseUrl || current.token !== server.token) throw new Error(m.mobile_machines_entry_changed());
    useServers.getState().update(server.id, { label: patch.label, baseUrl: patch.baseUrl });
    if (patch.confirmedIdentity) useServers.getState().rememberLan(server.id, { url: '', id: patch.confirmedIdentity }, { ...server, baseUrl: patch.baseUrl });
    if (patch.baseUrl !== server.baseUrl) onClose();
  }, m.mobile_machines_saved());
  const test = useMachineAction();
  return <MachinePage title={m.mobile_machines_edit()} save={{ onPress: saveLocal, disabled: local.busy || label === server.label && address === server.baseUrl, busy: local.busy }}>
    <MachineGroup>
      <MachineField label={m.mobile_machines_label()} value={label} onChangeText={setLabel} editable={!local.busy} />
      <MachineField label={m.mobile_machines_endpoint()} value={address} onChangeText={setAddress} editable={!local.busy} keyboardType="url" stacked />
    </MachineGroup>
    <MachineGroup>
      <MachineRow title={m.mobile_machines_connection_title()} value={status} disabled={local.busy} onPress={() => onNavigate('connection')} />
      <MachineRow title={test.busy ? m.mobile_machines_busy() : m.config_maquinas_testar()} action disabled={test.busy || local.busy}
        onPress={() => test.run(() => getConfigForServer(server, 15_000), () => {}, m.config_maquinas_conexao_ok())} />
      <MachineRow title={m.mobile_machines_advanced_title()} disabled={local.busy} onPress={() => onNavigate('advanced')} />
      <MachineRow title={m.mobile_machines_leave()} danger action disabled={local.busy} onPress={() => forget(server)} />
    </MachineGroup>
    <Notice text={local.error || test.error} error /><Notice text={local.message || test.message} />
  </MachinePage>;
}
