import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Alert, AppState, Text, View } from 'react-native';
import Constants from 'expo-constants';
import { useFocusEffect, useRouter } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { relativeTime, type AccountDevice, type ConnectMachine, type Server } from '@hangar/core';
import { InfoNotice } from '../config/InfoNotice';
import { Pill } from '../config/PageHeader';
import { SectionCard } from '../config/SectionCard';
import { useSettingsColors, withAlpha } from '../config/colors';
import { MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { Field, MachineSheet, Notice, styles as machineStyles, useMachineAction } from '../machines/parts';
import { ownServers, useServers } from '../../stores/servers';
import { missingMachines, needsEntry, readInstallCode, sameMachine, useConnect } from './connectStore';
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
  const status = useConnect((s) => s.device?.status);
  // Pendente ou sem acesso, o serviço recusa o bilhete: entrar fica para depois da liberação.
  const blocked = status === 'pending' || status === 'revoked';
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
  // A liberação deste aparelho e o pedido de outro chegam enquanto a pessoa olha esta tela: fora dela, ou com o app no
  // fundo, não consulta.
  useFocusEffect(useCallback(() => {
    const timer = setInterval(() => {
      if (AppState.currentState !== 'active') return;
      void useConnect.getState().refreshDevices();
    }, DEVICES_REFRESH_MS);
    return () => clearInterval(timer);
  }, []));
  const entered = () => { if (mounted.current) onDone(); };
  const waiting = machines?.filter((machine) => entries[machine.machine_id]?.phase === 'waiting') ?? [];
  const own = ownServers(servers);
  const missing = missingMachines(machines, servers);
  // Uma máquina esperando aprovação não trava as outras: "Entrar em todas" pula quem já está pedindo.
  const idle = missing.filter((machine) => !['asking', 'waiting'].includes(entries[machine.machine_id]?.phase ?? ''));
  const later = machines?.filter((machine) => !machine.online && !own.some((server) => sameMachine(server, machine))) ?? [];
  const offerAll = missing.length > 1 && !blocked;
  const allHelp = later.length ? m.connect_enter_all_offline({ machines: later.map((machine) => machine.name).join(', ') }) : undefined;
  const enterAll = () => {
    haptic.selection();
    void useConnect.getState().enterAll().then(() => { if (mounted.current) refresh(); });
  };
  const account = useConnect((s) => s.account?.name ?? '');
  return <>
    <View style={[styles.header, { borderColor: c.border }]}>
      <ConnectMark size={40} />
      <View style={styles.headerText}>
        <Text accessibilityRole="header" style={[styles.headerTitle, { color: c.text }]}>{m.connect_title()}</Text>
        <Text style={[styles.headerSub, { color: c.muted }]}>{m.connect_signed_in_as({ name: account })}</Text>
      </View>
    </View>
    <DeviceState />
    {/* Máquina sem a versão nova ainda pede a aprovação nela mesma. */}
    {waiting.map((machine) => <View key={machine.machine_id} style={styles.waiting}>
      <ActivityIndicator color={c.accent} />
      <View style={styles.waitingText}><Notice text={m.connect_waiting_notice({ name: machine.name })} /></View>
    </View>)}
    <SectionCard icon="Laptop" title={m.connect_your_machines()} subtitle={m.connect_machines_pull()}>
      {machines === null && !error ? <MachineRow title={loading ? m.mobile_machines_busy() : m.comum_carregando()} /> : null}
      {machines?.length === 0 ? <MachineRow title={m.connect_machines_empty()} /> : null}
      {machines?.map((machine) => <MachineItem key={machine.machine_id} machine={machine} blocked={blocked} onEntered={entered} onOpen={onDone} />)}
      {offerAll ? <MachineRow title={m.connect_enter_all()} description={allHelp} action disabled={!idle.length} onPress={enterAll} /> : null}
    </SectionCard>
    <Notice text={error} error />
    {/* Puxar para atualizar não se vê: o erro traz o caminho à mostra. */}
    {error ? <MachineGroup>
      <MachineRow title={m.mobile_machines_retry()} action disabled={loading} onPress={() => { void useConnect.getState().refreshMachines(); }} />
    </MachineGroup> : null}
    <ConnectRecovery servers={enabled} />
    {status === 'approved' ? <AccountDevices /> : null}
    <AccountRows />
  </>;
}

/** Este aparelho na conta: esperando a liberação, sem acesso, ou o erro que impediu saber. */
function DeviceState() {
  const c = useSettingsColors();
  const status = useConnect((s) => s.device?.status);
  const error = useConnect((s) => s.deviceError);
  const tap = (work: () => Promise<void>) => () => { haptic.selection(); void work(); };
  if (error) return <>
    <Notice text={error} error />
    <MachineGroup><MachineRow title={m.mobile_machines_retry()} action onPress={tap(() => useConnect.getState().refreshDevices())} /></MachineGroup>
  </>;
  if (status === 'pending') return <ReleaseCard />;
  if (status === 'revoked') return <>
    <Notice text={m.connect_device_revoked_notice()} error />
    <MachineGroup><MachineRow title={m.connect_device_request_again()} action onPress={tap(() => useConnect.getState().requestDeviceAgain())} /></MachineGroup>
  </>;
  return null;
}

/** A liberação vale uma vez para a conta toda; o código deixa quem libera conferir que é este celular. */
function ReleaseCard() {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [code, setCode] = useState('');
  useEffect(() => {
    let alive = true;
    void readInstallCode().then((value) => { if (alive) setCode(value); });
    return () => { alive = false; };
  }, []);
  const warn = theme.tokens.status.warning;
  return <View style={styles.release}>
    <Text accessibilityRole="header" style={[styles.releaseTitle, { color: c.text }]}>{m.connect_release_title()}</Text>
    <View style={[styles.releaseBox, { borderColor: warn, backgroundColor: withAlpha(warn, 0.12) }]}>
      <View style={styles.releaseHead}>
        <ActivityIndicator color={warn} />
        <Text style={[styles.releaseStrong, { color: warn }]}>{m.connect_release_waiting()}</Text>
      </View>
      <Text style={[styles.releaseText, { color: warn }]}>{m.connect_release_body()}</Text>
    </View>
    {code ? <>
      <Text style={[styles.releaseHint, { color: c.muted }]}>{m.connect_release_code()}</Text>
      {/* Lido letra a letra: o leitor de tela juntaria o código numa palavra. */}
      <Text selectable accessibilityLabel={code.replace('-', '').split('').join(' ')} style={[styles.code, { color: c.text }]}>{code}</Text>
    </> : null}
  </View>;
}

function MachineItem({ machine, blocked, onEntered, onOpen }: { machine: ConnectMachine; blocked: boolean; onEntered: () => void; onOpen: () => void }) {
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
  if (entry?.phase === 'failed') return <MachineRow title={title} description={entry.error} value={m.mobile_machines_retry()} disabled={blocked} onPress={enter} />;
  if (refused) return <MachineRow title={title} description={refused} value={m.connect_reenter()} disabled={blocked} onPress={enter} />;
  if (server) return <MachineRow title={title} value={m.connect_machine_added()} valueTone="success" onPress={onOpen} />;
  return <MachineRow title={title} value={m.login_entrar()} disabled={blocked} onPress={enter} />;
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

/** Aparelhos da conta: o pendente se libera ou recusa num toque, valendo para todas as máquinas; o liberado perde o acesso. */
function AccountDevices() {
  const { rows, error } = useConnect((s) => s.devices);
  const self = useConnect((s) => s.device?.id);
  const action = useMachineAction();
  const answer = (row: AccountDevice, approve: boolean) => {
    void action.run(async () => {
      try { await useConnect.getState().answerDevice(row.device_id, approve); }
      catch (failed) { haptic.error(); throw failed; }
      haptic.success();
    }, () => {});
  };
  const release = (row: AccountDevice) => Alert.alert(m.connect_account_device_release_title({ name: row.name }), m.connect_account_device_release_body(), [
    { text: m.comum_cancelar(), style: 'cancel' },
    { text: m.connect_device_deny(), style: 'destructive', onPress: () => answer(row, false) },
    { text: m.connect_account_device_release(), onPress: () => answer(row, true) },
  ]);
  // Revogado fica no serviço só para as máquinas o tirarem; pedidos esperando vêm em cima.
  const visible = (rows ?? []).filter((row) => row.status !== 'revoked')
    .sort((a, b) => Number(b.status === 'pending') - Number(a.status === 'pending'));
  return <>
    <SectionCard icon="Smartphone" title={m.connect_account_devices()}>
      {rows === null && !error ? <MachineRow title={m.comum_carregando()} /> : null}
      {rows && !visible.length ? <MachineRow title={m.connect_account_devices_empty()} /> : null}
      {visible.map((row) => (row.device_id === self
        ? <MachineRow key={row.device_id} title={Constants.deviceName || row.name} value={m.connect_device_this()} />
        : row.status === 'pending'
          ? <MachineRow key={row.device_id} title={row.name} description={m.connect_account_device_asked({ when: relativeTime(row.created_at) })}
            value={m.connect_account_device_release()} disabled={action.busy} onPress={() => release(row)} />
          : <MachineRow key={row.device_id} title={row.name} value={m.connect_device_revoke()} disabled={action.busy}
            onPress={() => confirm(m.connect_device_revoke_title({ name: row.name }), m.connect_account_device_revoke_body(),
              m.connect_device_revoke(), () => answer(row, false), true)} />))}
    </SectionCard>
    {/* Com a lista antiga na tela, o erro avisa que ela pode estar velha. */}
    <Notice text={error} error />
    <Notice text={action.error} error />
    <MachineHelp>{m.connect_account_devices_help()}</MachineHelp>
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
  release: { gap: theme.base.space[3] },
  releaseTitle: { ...theme.type.heading },
  releaseBox: { gap: theme.base.space[1.5], padding: theme.base.space[4], borderRadius: theme.base.radius.md, borderWidth: 1 },
  releaseHead: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  releaseStrong: { ...theme.type.control },
  releaseText: { ...theme.type.meta },
  releaseHint: { ...theme.type.meta, textAlign: 'center' },
  code: { ...theme.type.title, fontFamily: theme.type.mono.fontFamily, letterSpacing: 4, textAlign: 'center' },
}));
