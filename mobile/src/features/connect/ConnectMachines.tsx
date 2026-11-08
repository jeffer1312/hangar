import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Alert, AppState, Text, View } from 'react-native';
import Constants from 'expo-constants';
import { useFocusEffect, useRouter } from 'expo-router';
import { StyleSheet } from 'react-native-unistyles';
import type { ConnectMachine, Server } from '@hangar/core';
import { InfoNotice } from '../config/InfoNotice';
import { Pill } from '../config/PageHeader';
import { SectionCard } from '../config/SectionCard';
import { useSettingsColors } from '../config/colors';
import { MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { Field, MachineSheet, Notice, styles as machineStyles, useMachineAction } from '../machines/parts';
import { ownServers, useServers } from '../../stores/servers';
import { connectServers, needsEntry, sameMachine, useConnect } from './connectStore';
import { ConnectMark } from './ConnectHero';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

const DEVICES_REFRESH_MS = 10_000;

function confirm(title: string, body: string, label: string, onYes: () => void, destructive = false) {
  Alert.alert(title, body, [
    { text: m.comum_cancelar(), style: 'cancel' },
    { text: label, style: destructive ? 'destructive' : 'default', onPress: onYes },
  ]);
}

export function ConnectMachines({ onDone }: { onDone: () => void }) {
  const c = useSettingsColors();
  const machines = useConnect((s) => s.machines);
  const error = useConnect((s) => s.machinesError);
  const loading = useConnect((s) => s.loadingMachines);
  const entries = useConnect((s) => s.entries);
  const servers = useServers((s) => s.servers);
  const enabled = useMemo(() => ownServers(servers).filter((s) => !s.disabled), [servers]);
  const withDevices = useMemo(() => connectServers(servers), [servers]);
  const mounted = useRef(true);
  const refresh = () => {
    void useConnect.getState().refreshMachines();
    void useConnect.getState().refreshDevices();
  };
  useEffect(() => {
    mounted.current = true;
    refresh();
    return () => { mounted.current = false; };
  }, []);
  // O pedido de outro aparelho chega enquanto a pessoa olha esta tela: fora dela, ou com o app no fundo, não consulta.
  useFocusEffect(useCallback(() => {
    const timer = setInterval(() => {
      if (AppState.currentState === 'active') void useConnect.getState().refreshDevices();
    }, DEVICES_REFRESH_MS);
    return () => clearInterval(timer);
  }, []));
  const entered = () => { if (mounted.current) onDone(); };
  const waiting = machines?.filter((machine) => entries[machine.machine_id]?.phase === 'waiting') ?? [];
  const account = useConnect((s) => s.account?.name ?? '');
  return <>
    <View style={[styles.header, { borderColor: c.border }]}>
      <ConnectMark size={40} />
      <View style={styles.headerText}>
        <Text accessibilityRole="header" style={[styles.headerTitle, { color: c.text }]}>{m.connect_title()}</Text>
        <Text style={[styles.headerSub, { color: c.muted }]}>{m.connect_signed_in_as({ name: account })}</Text>
      </View>
    </View>
    {waiting.map((machine) => <View key={machine.machine_id} style={styles.waiting}>
      <ActivityIndicator color={c.accent} />
      <View style={styles.waitingText}><Notice text={m.connect_waiting_notice({ name: machine.name })} /></View>
    </View>)}
    <SectionCard icon="Laptop" title={m.connect_your_machines()} subtitle={m.connect_machines_pull()}>
      {machines === null && !error ? <MachineRow title={loading ? m.mobile_machines_busy() : m.comum_carregando()} /> : null}
      {machines?.length === 0 ? <MachineRow title={m.connect_machines_empty()} /> : null}
      {machines?.map((machine) => <MachineItem key={machine.machine_id} machine={machine} onEntered={entered} onOpen={onDone} />)}
    </SectionCard>
    <Notice text={error} error />
    {/* Puxar para atualizar não se vê: o erro traz o caminho à mostra. */}
    {error ? <MachineGroup>
      <MachineRow title={m.mobile_machines_retry()} action disabled={loading} onPress={() => { void useConnect.getState().refreshMachines(); }} />
    </MachineGroup> : null}
    <ConnectRecovery servers={enabled} />
    <AllDevices servers={withDevices} />
    <AccountRows />
  </>;
}

function MachineItem({ machine, onEntered, onOpen }: { machine: ConnectMachine; onEntered: () => void; onOpen: () => void }) {
  const entry = useConnect((s) => s.entries[machine.machine_id]);
  const server = useServers((s) => ownServers(s.servers).find((candidate) => sameMachine(candidate, machine)));
  const enter = () => {
    void useConnect.getState().enter(machine).then((saved) => {
      if (saved) { haptic.success(); onEntered(); }
      else if (useConnect.getState().entries[machine.machine_id]?.phase === 'failed') haptic.error();
    });
  };
  const title = machine.name;
  if (entry?.phase === 'asking') return <MachineRow title={title} value={m.mobile_machines_busy()} />;
  if (entry?.phase === 'waiting') return <MachineRow title={title} value={m.connect_waiting()} />;
  const refused = server?.disabled ? m.connect_refused_help() : undefined;
  if (!machine.online) return <MachineRow title={title} value={m.mobile_machines_offline()} valueTone="error" description={entry?.error ?? refused} disabled />;
  if (entry?.phase === 'failed') return <MachineRow title={title} description={entry.error} value={m.mobile_machines_retry()} onPress={enter} />;
  if (refused) return <MachineRow title={title} description={refused} value={m.connect_reenter()} onPress={enter} />;
  if (server) return <MachineRow title={title} value={m.connect_machine_added()} valueTone="success" onPress={onOpen} />;
  return <MachineRow title={title} value={m.login_entrar()} onPress={enter} />;
}

/** O caminho de volta de quem foi recusado: pedir a entrada de novo, ou religar o Connect sobre a senha guardada. */
export function ConnectRecovery({ servers }: { servers: Server[] }) {
  const router = useRouter();
  const action = useMachineAction();
  useEffect(() => { void useConnect.getState().load(); }, []);
  const refused = servers.filter(needsEntry);
  const off = servers.filter((s) => s.connectOff && !s.disabled);
  const reenter = (server: Server) => {
    void useConnect.getState().reenter(server);
    router.push('/connect' as never);
  };
  const turnOn = (server: Server) => action.run(() => useConnect.getState().turnOnConnect(server), () => {},
    (on) => (on ? m.connect_turned_on() : m.connect_turn_on_pending()));
  return <>
    {refused.length || off.length ? <MachineGroup>
      {refused.map((server) => <MachineRow key={server.id} title={server.label} description={m.connect_refused_help()}
        value={m.connect_reenter()} onPress={() => reenter(server)} />)}
      {off.map((server) => <MachineRow key={server.id} title={server.label} description={m.connect_off_help()}
        value={m.connect_turn_on()} disabled={action.busy} onPress={() => { void turnOn(server); }} />)}
    </MachineGroup> : null}
    <Notice text={action.error} error /><Notice text={action.message} />
  </>;
}

/** Os aparelhos de todas as máquinas num lugar só: pedidos esperando em cima, quem já entrou embaixo. */
function AllDevices({ servers }: { servers: Server[] }) {
  const devices = useConnect((s) => s.devices);
  const action = useMachineAction();
  if (!servers.length) return null;
  const answer = (server: Server, id: string, approve: boolean) => {
    void action.run((signal) => useConnect.getState().answerDevice(server, id, approve, signal), () => {});
  };
  const rows = servers.flatMap((server) => (devices[server.id]?.rows ?? []).map((row) => ({ server, row })));
  const pending = rows.filter(({ row }) => row.status === 'pending');
  const approved = rows.filter(({ row }) => row.status === 'approved');
  // "Nenhum aparelho" só com todas as listas na mão: uma ainda a caminho pode trazer algum.
  const loading = servers.some((server) => !devices[server.id]);
  // Uma linha por máquina que falhou: com lista antiga na tela, avisa que ela pode estar velha.
  const failures = servers.filter((server) => devices[server.id]?.error)
    .map((server) => `${server.label}: ${devices[server.id]?.error ?? ''}`);
  return <>
    {pending.map(({ server, row }) => <SectionCard key={`${server.id}:${row.device_id}`} icon="Smartphone" title={row.name}
      subtitle={m.connect_device_pending({ machine: server.label })}>
      <MachineRow title={m.connect_device_approve()} action disabled={action.busy}
        onPress={() => confirm(m.connect_device_approve_title({ name: row.name }), m.connect_device_approve_body({ machine: server.label }),
          m.connect_device_approve(), () => answer(server, row.device_id, true))} />
      <MachineRow title={m.connect_device_deny()} danger action disabled={action.busy} onPress={() => answer(server, row.device_id, false)} />
    </SectionCard>)}
    <SectionCard icon="Smartphone" title={m.connect_devices_all()}>
      {loading ? <MachineRow title={m.comum_carregando()} /> : null}
      {!loading && !approved.length && !failures.length ? <MachineRow title={m.connect_devices_empty()} /> : null}
      {approved.map(({ server, row }) => (row.device_id === server.deviceId
        ? <MachineRow key={`${server.id}:${row.device_id}`} title={Constants.deviceName || row.name}
          description={m.connect_device_on({ machine: server.label })} value={m.connect_device_this()} />
        : <MachineRow key={`${server.id}:${row.device_id}`} title={row.name} description={m.connect_device_on({ machine: server.label })}
          value={m.connect_device_revoke()} disabled={action.busy}
          onPress={() => confirm(m.connect_device_revoke_title({ name: row.name }), m.connect_device_revoke_body({ machine: server.label }),
            m.connect_device_revoke(), () => answer(server, row.device_id, false), true)} />))}
    </SectionCard>
    {failures.map((text) => <Notice key={text} text={text} error />)}
    <Notice text={action.error} error />
    <MachineHelp>{m.connect_devices_help()}</MachineHelp>
  </>;
}

function AccountRows() {
  const name = useConnect((s) => s.account?.name);
  const [changing, setChanging] = useState(false);
  const action = useMachineAction();
  const endSessions = () => confirm(m.connect_end_sessions(), m.connect_end_sessions_body(), m.connect_end_sessions(),
    () => { void action.run(() => useConnect.getState().endSessions(), () => {}); }, true);
  return <>
    <MachineGroup>
      <MachineRow title={m.connect_account()} value={name} />
      <MachineRow title={m.connect_password_title()} disabled={action.busy} onPress={() => setChanging(true)} />
      <MachineRow title={m.connect_sign_out()} action disabled={action.busy} onPress={() => { void action.run(() => useConnect.getState().signOut(), () => {}); }} />
      <MachineRow title={m.connect_end_sessions()} danger action disabled={action.busy} onPress={endSessions} />
    </MachineGroup>
    <MachineHelp>{m.connect_sign_out_help()}</MachineHelp>
    <Notice text={action.error} error />
    {changing ? <PasswordSheet onClose={() => setChanging(false)} /> : null}
  </>;
}

function PasswordSheet({ onClose }: { onClose: () => void }) {
  const [current, setCurrent] = useState('');
  const [next, setNext] = useState('');
  const action = useMachineAction();
  const ready = !action.busy && !!current && !!next;
  const submit = () => {
    if (!ready) return;
    // Os campos só esvaziam quando a troca deu certo: senha atual errada se corrige sem redigitar as duas.
    void action.run(async () => {
      try { await useConnect.getState().changePassword(current, next); }
      catch (error) { haptic.error(); throw error; }
      haptic.success();
    }, () => { setCurrent(''); setNext(''); }, m.connect_password_changed());
  };
  return <MachineSheet title={m.connect_password_title()} onClose={onClose}>
    <View style={machineStyles.content}>
      <Field label={m.connect_password_current()} value={current} onChangeText={setCurrent} editable={!action.busy} secureTextEntry
        textContentType="password" autoComplete="current-password" returnKeyType="next" />
      <Field label={m.connect_password_new()} value={next} onChangeText={setNext} editable={!action.busy} secureTextEntry
        textContentType="newPassword" autoComplete="new-password" returnKeyType="go" onSubmitEditing={submit} />
      <InfoNotice text={m.connect_password_help()} />
      <Notice text={action.error} error /><Notice text={action.message} />
      <View style={machineStyles.actions}>
        <Pill label={action.busy ? m.mobile_machines_busy() : m.connect_password_title()} disabled={!ready} onPress={submit} />
      </View>
    </View>
  </MachineSheet>;
}

const styles = StyleSheet.create((theme) => ({
  header: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], padding: theme.base.space[4], borderRadius: theme.base.radius.md, borderWidth: 1 },
  headerText: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  headerTitle: { fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold },
  headerSub: { fontSize: theme.base.text.sm },
  waiting: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[4] },
  waitingText: { flex: 1, minWidth: 0 },
}));
