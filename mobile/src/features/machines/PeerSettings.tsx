import { useEffect, useRef, useState } from 'react';
import { Alert, View } from 'react-native';
import type { Server } from '@hangar/core';
import { ownServers, useServers } from '../../stores/servers';
import { MachineGroup, MachineRow, MachineField } from './MachineRows';
import { InfoNotice } from '../config/InfoNotice';
import { Pill } from '../config/PageHeader';
import { enablePeer, ensureActive, measureDirections, normalizeUrl, readIdentifier, readPeers, reason, registerPeer, removePeer, type Directions, type Peer } from './api';
import { DirectionsNotice, Notice, styles, useMachineAction } from './parts';
import * as m from '../../paraglide/messages';

export function PeerSettings({ server }: { server: Server }) {
  const saved = useServers((s) => s.servers);
  const candidates = ownServers(saved).filter((s) => s.id !== server.id);
  const [peers, setPeers] = useState<Peer[] | null>(null);
  const [identifier, setIdentifier] = useState('');
  const [address, setAddress] = useState('');
  const [token, setToken] = useState('');
  const [checks, setChecks] = useState<Record<string, Directions>>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const read = useMachineAction();
  const write = useMachineAction();
  const alive = useRef(true);
  const confirming = useRef(false);
  const [asking, setAsking] = useState(false);
  const load = () => read.run((signal) => readPeers(server, signal), setPeers);
  const applyPeers = (next: Peer[]) => { setPeers(next); setChecks({}); };
  useEffect(() => {
    alive.current = true; void load();
    return () => { alive.current = false; confirming.current = false; };
  }, [server.id, server.baseUrl, server.token]);

  const check = (peer: Peer) => write.run(async (signal) => {
    let remote = candidates.find((s) => s.baseUrl.replace(/\/+$/, '') === peer.base_url) ?? null;
    const failures: string[] = [];
    // Um token salvo em outro endereço só serve se aquela entrada disser o mesmo identificador.
    if (!remote && candidates.length) {
      const identities = await Promise.allSettled(candidates.map((s) => readIdentifier(s, signal)));
      ensureActive(signal);
      identities.forEach((identity, index) => {
        if (identity.status === 'fulfilled' && identity.value.identificador === peer.id && !remote) remote = candidates[index];
        else if (identity.status === 'rejected') failures.push(`${candidates[index].label}: ${reason(identity.reason)}`);
      });
    }
    const result = await measureDirections(server, peer, remote, signal);
    if (failures.length) result.backError = [result.backError, ...failures].filter(Boolean).join(' · ');
    return result;
  }, (result) => setChecks((old) => ({ ...old, [peer.id]: result })));

  const remove = (peer: Peer) => {
    if (confirming.current || write.busy || !alive.current) return;
    confirming.current = true; setAsking(true);
    const cancel = () => { if (alive.current) { confirming.current = false; setAsking(false); } };
    Alert.alert(m.mobile_machines_peer_remove({ name: peer.id }), m.mobile_machines_peer_remove_body({ name: peer.id }), [
      { text: m.comum_cancelar(), style: 'cancel', onPress: cancel },
      { text: m.lista_remover(), style: 'destructive', onPress: () => {
        if (!alive.current || !confirming.current) return;
        confirming.current = false; setAsking(false);
        void write.run((signal) => removePeer(server, peer.id, signal), applyPeers, m.mobile_machines_peer_removed());
      } },
    ], { cancelable: true, onDismiss: cancel });
  };
  const busy = read.busy || write.busy || asking;
  return <>
      {read.busy ? <InfoNotice text={m.native_server_loading()} /> : null}
      <Notice text={read.error || write.error} error /><Notice text={write.message} />
      {read.error ? <Pill label={m.mobile_machines_retry()} onPress={load} disabled={busy} /> : null}
      {peers?.length === 0 ? <InfoNotice text={m.mobile_machines_no_peers()} /> : null}
      {peers?.length ? <MachineGroup>{peers.map((peer) => <View key={peer.id}>
        <MachineRow title={peer.id} value={peer.enabled ? m.mobile_machines_peer_enable_state() : m.mobile_machines_peer_disable_state()}
          expanded={selected === peer.id}
          description={selected === peer.id ? peer.base_url : undefined}
          onPress={() => setSelected((value) => value === peer.id ? null : peer.id)} />
        {selected === peer.id ? <View style={styles.content}>
        <View style={styles.actions}>
          <Pill label={m.mobile_machines_check()} onPress={() => check(peer)} disabled={busy} />
          <Pill label={peer.enabled ? m.mobile_machines_peer_disable() : m.mobile_machines_peer_enable()} disabled={busy}
            onPress={() => write.run((signal) => enablePeer(server, peer.id, !peer.enabled, signal), applyPeers, m.mobile_machines_saved())} />
          <Pill label={m.lista_remover()} onPress={() => remove(peer)} disabled={busy} />
        </View>
        {checks[peer.id] ? <DirectionsNotice value={checks[peer.id]} /> : null}
        </View> : null}
      </View>)}</MachineGroup> : null}
      <MachineGroup><MachineRow title={m.mobile_machines_add_peer()} action onPress={() => setAdding((value) => !value)} disabled={busy} /></MachineGroup>
      {adding ? <>
      <View style={styles.actions}>
      {candidates.map((candidate) => <Pill key={candidate.id} label={m.mobile_machines_use_saved({ name: candidate.label })} disabled={busy}
        onPress={() => write.run((signal) => readIdentifier(candidate, signal), (identity) => {
          setIdentifier(identity.identificador); setAddress(candidate.baseUrl); setToken(candidate.token);
        })} />)}
      </View>
      <MachineGroup>
      <MachineField label={m.mobile_machines_peer_id()} value={identifier} onChangeText={setIdentifier} editable={!busy} stacked />
      <MachineField label={m.mobile_machines_peer_url()} value={address} onChangeText={setAddress} editable={!busy} keyboardType="url" stacked />
      <MachineField label={m.mobile_machines_token()} value={token} onChangeText={setToken} editable={!busy} secureTextEntry autoComplete="off" stacked />
      </MachineGroup>
      <View style={styles.actions}><Pill label={write.busy ? m.mobile_machines_busy() : m.mobile_machines_save()} disabled={busy}
        onPress={() => write.run((signal) => {
          const base = normalizeUrl(address);
          const credential = token.trim();
          return registerPeer(server, identifier, base, credential, signal);
        }, applyPeers, m.mobile_machines_saved())} /></View>
      </> : null}
  </>;
}
