import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { Alert, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { baseOf, comTeto, getConfigForServer, verifyServerIdentity, type Server } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { INK } from '../../theme/scale';
import { Icon } from '../../ui/Icon';
import { SettingsRow } from '../config/SettingsRow';
import { useSettingsColors } from '../config/colors';
import { Field, MachineSheet, Notice, useMachineAction } from './parts';
import { ensureActive, normalizeUrl, readIdentifier } from './api';
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

export function MachineSettingsSheet({ server, onClose }: { server: Server; onClose: () => void }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const [label, setLabel] = useState(server.label);
  const [address, setAddress] = useState(server.baseUrl);
  const [networkOpen, setNetworkOpen] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [advanced, setAdvanced] = useState<'technical' | 'peers' | 'maintenance' | null>(null);
  const connection = useSessions((state) => state.byServer.find((entry) => entry.server.id === server.id));
  const route = baseOf(server);
  const status = connection?.error ? connection.error === 'offline' ? m.mobile_machines_offline() : connection.error : !connection?.loaded ? m.mobile_machines_connection_pending()
    : route === server.lan?.url ? m.mobile_machines_connection_local()
    : route === server.remote?.url || /\.ts\.net(?::|\/|$)/.test(route) ? m.mobile_machines_connection_remote()
    : m.mobile_machines_connection_main();
  const local = useMachineAction();
  const forget = useForgetMachine(onClose);
  const saveLocal = () => local.run(async (signal) => {
    const baseUrl = normalizeUrl(address);
    let confirmedIdentity: string | undefined;
    if (baseUrl !== server.baseUrl) {
      const identifier = server.lan?.id || server.remote?.id || (await readIdentifier(server, signal)).identificador;
      if (!identifier) throw new Error(m.mobile_machines_known_identifier_required());
      const verified = await verifyServerIdentity(server, { url: baseUrl, id: identifier }, comTeto(signal, 15_000));
      ensureActive(signal);
      if (!verified) throw new Error(m.mobile_machines_identity_unconfirmed());
      // O id salvo pode ter rota LAN em cache; este teste precisa atingir o endereço digitado.
      const candidate = { id: `machine-probe:${baseUrl}`, label: server.label, baseUrl, token: server.token };
      await getConfigForServer(candidate, 15_000);
      ensureActive(signal);
      confirmedIdentity = identifier;
    }
    return { label: label.trim() || server.label, baseUrl, confirmedIdentity };
  }, (patch) => {
    useServers.getState().update(server.id, { label: patch.label, baseUrl: patch.baseUrl });
    if (patch.confirmedIdentity) useServers.getState().rememberLan(server.id, { url: '', id: patch.confirmedIdentity }, { ...server, baseUrl: patch.baseUrl });
    if (patch.baseUrl !== server.baseUrl) onClose();
  }, m.mobile_machines_saved());
  return <MachineSheet title={m.mobile_machines_edit()} onClose={onClose}>
    <View style={editorStyles.form}>
      <Field label={m.mobile_machines_label()} value={label} onChangeText={setLabel} editable={!local.busy} />
      <Field label={m.mobile_machines_endpoint()} value={address} onChangeText={setAddress} editable={!local.busy} keyboardType="url" />
      <View style={editorStyles.status} accessibilityLiveRegion="polite">
        <View style={[editorStyles.dot, { backgroundColor: connection?.loaded && !connection.error ? theme.tokens.status.success : c.faint }]} />
        <Text style={[editorStyles.caption, { color: c.muted }]}>{status}</Text>
      </View>
      <Disclosure label={m.mobile_machines_connection()} open={networkOpen} onPress={() => setNetworkOpen((value) => !value)}>
        <SettingsRow title={m.mobile_machines_local_address()} description={server.lan?.url || m.mobile_machines_not_discovered()} />
        <SettingsRow title={m.mobile_machines_remote_address()} description={server.remote?.url || (/\.ts\.net(?::|\/|$)/.test(server.baseUrl) ? server.baseUrl : m.mobile_machines_not_discovered())} />
      </Disclosure>
      <Text style={[editorStyles.caption, { color: c.muted }]}>{m.mobile_machines_connection_help()}</Text>
      <Notice text={local.error} error /><Notice text={local.message} />
      <Pressable accessibilityRole="button" accessibilityLabel={m.mobile_machines_save()} accessibilityState={{ disabled: local.busy }}
        disabled={local.busy} onPress={saveLocal} style={[editorStyles.save, { backgroundColor: c.accent, opacity: local.busy ? 0.5 : 1 }]}>
        <Text style={[editorStyles.saveText, { color: INK.onAccent }]}>{local.busy ? m.mobile_machines_busy() : m.mobile_machines_save()}</Text>
      </Pressable>
      <Pressable accessibilityRole="button" accessibilityLabel={m.comum_cancelar()} onPress={onClose} style={editorStyles.secondary}>
        <Text style={[editorStyles.caption, { color: c.muted }]}>{m.comum_cancelar()}</Text>
      </Pressable>
    </View>
    <Disclosure label={m.mobile_machines_more()} open={moreOpen} onPress={() => setMoreOpen((value) => !value)}>
      <Disclosure label={m.mobile_machines_technical()} open={advanced === 'technical'} onPress={() => setAdvanced((value) => value === 'technical' ? null : 'technical')}>
        <ServerSettings server={server} />
      </Disclosure>
      <Disclosure label={m.mobile_machines_peer_controls()} open={advanced === 'peers'} onPress={() => setAdvanced((value) => value === 'peers' ? null : 'peers')}>
        <PeerSettings server={server} />
      </Disclosure>
      <Disclosure label={m.mobile_machines_maintenance()} open={advanced === 'maintenance'} onPress={() => setAdvanced((value) => value === 'maintenance' ? null : 'maintenance')}>
        <MachineOperations server={server} />
      </Disclosure>
      <Pressable accessibilityRole="button" accessibilityLabel={m.mobile_machines_leave()} disabled={local.busy} onPress={() => forget(server)} style={editorStyles.secondary}>
        <Text style={[editorStyles.caption, { color: theme.tokens.status.error }]}>{m.mobile_machines_leave()}</Text>
      </Pressable>
    </Disclosure>
  </MachineSheet>;
}

function Disclosure({ label, open, onPress, children }: { label: string; open: boolean; onPress: () => void; children: ReactNode }) {
  const c = useSettingsColors();
  return <View>
    <Pressable accessibilityRole="button" accessibilityLabel={label} accessibilityState={{ expanded: open }} onPress={onPress}
      style={[editorStyles.disclosure, { borderBottomColor: c.border }]}>
      <Text style={[editorStyles.disclosureText, { color: c.text }]}>{label}</Text>
      <Icon name={open ? 'ChevronDown' : 'ChevronRight'} size={17} color={c.muted} />
    </Pressable>
    {open ? <View style={editorStyles.expanded}>{children}</View> : null}
  </View>;
}

const editorStyles = StyleSheet.create((theme) => ({
  form: { gap: theme.base.space[3] },
  status: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingVertical: theme.base.space[2] },
  dot: { width: 7, height: 7, borderRadius: theme.base.radius.full },
  caption: { fontSize: theme.base.text.xs, lineHeight: 19 },
  save: { minHeight: 48, borderRadius: theme.base.radius.md, alignItems: 'center', justifyContent: 'center', paddingHorizontal: theme.base.space[4] },
  saveText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  secondary: { minHeight: 44, alignItems: 'center', justifyContent: 'center', padding: theme.base.space[2] },
  disclosure: { minHeight: 52, borderBottomWidth: 1, flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: theme.base.space[3] },
  disclosureText: { fontSize: theme.base.text.sm, flex: 1 },
  expanded: { paddingVertical: theme.base.space[3], gap: theme.base.space[3] },
}));
