import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Alert, AppState, Text, View } from 'react-native';
import Constants from 'expo-constants';
import { useFocusEffect, useRouter } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { relativeTime, type AccountDevice, type ConnectMachine, type Server } from '@hangar/core';
import { CheckRow } from '../config/CheckRow';
import { InfoNotice } from '../config/InfoNotice';
import { Pill } from '../config/PageHeader';
import { SectionCard } from '../config/SectionCard';
import { useSettingsColors, withAlpha } from '../config/colors';
import { MachineGroup, MachineHelp, MachineRow } from '../machines/MachineRows';
import { Field, MachineSheet, Notice, styles as machineStyles, useMachineAction } from '../machines/parts';
import { ownServers, useServers } from '../../stores/servers';
import { missingMachines, needsEntry, readInstallCode, sameMachine, useConnect } from './connectStore';
import { ConnectMark } from './ConnectHero';
import { ComputerDetail } from './ComputerDetail';
import { LoginSubmit } from '../login/LoginFields';
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
  // Ligada, fora deste aparelho e sem pedido: vem marcada, e a pessoa tira a que não quer. Guardar só as desmarcadas
  // deixa marcada a máquina que liga depois.
  const [unpicked, setUnpicked] = useState<ReadonlySet<string>>(() => new Set());
  const choosable = missingMachines(machines, servers).filter((machine) => !entries[machine.machine_id]);
  const picked = choosable.filter((machine) => !unpicked.has(machine.machine_id));
  const toggle = (id: string) => setUnpicked((before) => {
    const next = new Set(before);
    if (!next.delete(id)) next.add(id);
    return next;
  });
  const enterPicked = () => {
    haptic.selection();
    const ids = picked.map((machine) => machine.machine_id);
    void useConnect.getState().enterAll(ids).then((saved) => {
      if (!mounted.current) return;
      const failed = ids.some((id) => useConnect.getState().entries[id]?.phase === 'failed');
      if (failed) haptic.error();
      // Entrou em todas as escolhidas: segue para o app. Alguma falhou: fica, e a linha dela diz o motivo.
      if (saved.length && !failed) { haptic.success(); onDone(); }
      else refresh();
    });
  };
  const enterLabel = picked.length === 1 ? m.login_guest_enter_one() : m.login_guest_enter_many({ n: String(picked.length) });
  const account = useConnect((s) => s.account?.name ?? '');
  // Pelo id: a lista relida troca os objetos, e o computador que saiu da conta fecha o detalhe sozinho.
  const [detail, setDetail] = useState<string | null>(null);
  const opened = detail ? machines?.find((machine) => machine.machine_id === detail) : undefined;
  useEffect(() => { if (detail && machines && !opened) setDetail(null); }, [detail, machines, opened]);
  return <>
    <View style={[styles.header, { borderColor: c.border }]}>
      <ConnectMark size={40} />
      <View style={styles.headerText}>
        <Text accessibilityRole="header" style={[styles.headerTitle, { color: c.text }]}>{m.connect_title()}</Text>
        <Text style={[styles.headerSub, { color: c.muted }]}>{m.connect_signed_in_as({ name: account })}</Text>
      </View>
    </View>
    {/* O pedido esperando vem antes de tudo: quem pediu está com o celular na mão agora. */}
    {status === 'approved' ? <PendingRequests /> : null}
    <DeviceState />
    {/* Máquina sem a versão nova ainda pede a aprovação nela mesma. */}
    {waiting.map((machine) => <View key={machine.machine_id} style={styles.waiting}>
      <ActivityIndicator color={c.accent} />
      <View style={styles.waitingText}><Notice text={m.connect_waiting_notice({ name: machine.name })} /></View>
    </View>)}
    <SectionCard icon="Laptop" title={m.connect_computers_title()} subtitle={m.connect_computers_help({ account })}>
      {machines === null && !error ? <MachineRow title={loading ? m.mobile_machines_busy() : m.comum_carregando()} /> : null}
      {machines?.length === 0 ? <MachineRow title={m.connect_machines_empty()} /> : null}
      {machines?.map((machine) => {
        const id = machine.machine_id;
        // O toque marca para entrar; as opções, como Remover da conta, ficam no toque longo.
        if (choosable.includes(machine)) {
          return <CheckRow key={id} title={machine.name} dot="on" checked={!unpicked.has(id)} disabled={blocked} onToggle={() => toggle(id)}
            onLongPress={() => setDetail(id)} longPressLabel={m.connect_computer_more({ name: machine.name })} />;
        }
        if (!machine.online && !entries[id] && !own.some((server) => sameMachine(server, machine))) {
          return <MachineRow key={id} title={machine.name} description={m.connect_machine_off_now()} onPress={() => setDetail(id)} />;
        }
        return <MachineItem key={id} machine={machine} onOpen={() => setDetail(id)} />;
      })}
    </SectionCard>
    {choosable.length > 0 && !blocked ? <LoginSubmit label={enterLabel} blocked={!picked.length} onPress={enterPicked} /> : null}
    <Notice text={error} error />
    {/* Puxar para atualizar não se vê: o erro traz o caminho à mostra. */}
    {error ? <MachineGroup>
      <MachineRow title={m.mobile_machines_retry()} action disabled={loading} onPress={() => { void useConnect.getState().refreshMachines(); }} />
    </MachineGroup> : null}
    {machines?.length ? <MachineHelp>{choosable.length ? m.connect_computers_tip_checks() : m.connect_computers_tip()}</MachineHelp> : null}
    <ConnectRecovery servers={enabled} />
    {status === 'approved' ? <AccountDevices /> : null}
    <AccountRows />
    {opened ? <ComputerSheet key={opened.machine_id} machine={opened} blocked={blocked} onEntered={entered} onDone={onDone}
      onClose={() => setDetail(null)} /> : null}
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

/** Linha de um computador da conta: diz o estado, e o toque abre o detalhe com as ações dele. */
function MachineItem({ machine, onOpen }: { machine: ConnectMachine; onOpen: () => void }) {
  const entry = useConnect((s) => s.entries[machine.machine_id]);
  const server = useServers((s) => ownServers(s.servers).find((candidate) => sameMachine(candidate, machine)));
  const title = machine.name;
  if (entry?.phase === 'asking') return <MachineRow title={title} value={m.mobile_machines_busy()} />;
  if (entry?.phase === 'waiting') return <MachineRow title={title} value={m.connect_waiting()} />;
  const refused = server?.disabled ? m.connect_refused_help() : undefined;
  if (!machine.online) return <MachineRow title={title} value={m.mobile_machines_offline()} valueTone="error" description={entry?.error ?? refused} onPress={onOpen} />;
  if (entry?.phase === 'failed') return <MachineRow title={title} description={entry.error} value={m.mobile_machines_retry()} onPress={onOpen} />;
  if (refused) return <MachineRow title={title} description={refused} value={m.connect_reenter()} onPress={onOpen} />;
  if (server) return <MachineRow title={title} value={m.connect_machine_added()} valueTone="success" onPress={onOpen} />;
  return <MachineRow title={title} value={m.login_entrar()} onPress={onOpen} />;
}

/** Detalhe de um computador: estado, a ação que cabe agora e Remover da conta. */
function ComputerSheet({ machine, blocked, onEntered, onDone, onClose }: {
  machine: ConnectMachine; blocked: boolean; onEntered: () => void; onDone: () => void; onClose: () => void;
}) {
  const entry = useConnect((s) => s.entries[machine.machine_id]);
  const server = useServers((s) => ownServers(s.servers).find((candidate) => sameMachine(candidate, machine)));
  const remove = useMachineAction();
  const entering = entry?.phase === 'asking' || entry?.phase === 'waiting';
  const enter = () => {
    haptic.selection();
    // A espera aparece na linha da lista: o detalhe fecha para ela ficar à vista.
    onClose();
    void useConnect.getState().enter(machine).then((saved) => {
      if (saved) { haptic.success(); onEntered(); }
      else if (useConnect.getState().entries[machine.machine_id]?.phase === 'failed') haptic.error();
    });
  };
  const askRemove = () => confirm(m.connect_computer_remove_title({ name: machine.name }), m.connect_computer_remove_body(),
    m.connect_computer_remove(), () => {
      void remove.run(async () => {
        try { await useConnect.getState().removeMachine(machine); }
        catch (error) { haptic.error(); throw error; }
        haptic.success();
      }, onClose);
    }, true);
  const refused = !!server?.disabled;
  const canEnter = machine.online && !blocked && !entering && !remove.busy;
  let next: { title: string; description?: string } | null = null;
  if (entry?.phase === 'failed') next = { title: m.mobile_machines_retry(), description: entry.error };
  else if (refused) next = { title: m.connect_reenter(), description: m.connect_refused_help() };
  else if (!server && machine.online) next = { title: m.connect_computer_add() };
  return <MachineSheet title={machine.name} onClose={onClose}>
    <ComputerDetail online={machine.online} inApp={!!server && !refused} next={next} busy={remove.busy} canEnter={canEnter}
      removeDisabled={blocked || entering || remove.busy} onOpen={() => { onClose(); onDone(); }} onEnter={enter} onRemove={askRemove} />
    <Notice text={remove.error} error />
  </MachineSheet>;
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

/** Liberar e Recusar um aparelho da conta, com a mesma confirmação nas duas listas. */
function useAnswerDevice() {
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
    { text: m.connect_account_device_release(), onPress: () => answer(row, true) },
  ]);
  return { action, answer, release };
}

/** Os pedidos esperando liberação na conta, um grupo em destaque cada, no topo da tela. */
function PendingRequests() {
  const rows = useConnect((s) => s.devices.rows);
  const self = useConnect((s) => s.device?.id);
  const { action, answer, release } = useAnswerDevice();
  const pending = (rows ?? []).filter((row) => row.status === 'pending' && row.device_id !== self);
  return <>
    {pending.map((row) => {
      const asked = m.connect_account_device_asked({ when: relativeTime(row.created_at) });
      return <SectionCard key={row.device_id} icon="Smartphone" tone="warning" title={m.connect_request_title({ name: row.name })}
        subtitle={m.connect_request_help()}>
        {/* O código é o que o celular que pediu mostra: quem libera confere que é ele. */}
        {row.install_code ? <MachineRow title={m.connect_request_code({ code: row.install_code })} description={asked} />
          : <MachineRow title={asked} />}
        <MachineRow title={m.connect_account_device_release()} accessibilityLabel={m.connect_request_release_label({ name: row.name })}
          action disabled={action.busy} onPress={() => release(row)} />
        <MachineRow title={m.connect_device_deny()} accessibilityLabel={m.connect_request_deny_label({ name: row.name })}
          danger action disabled={action.busy} onPress={() => answer(row, false)} />
      </SectionCard>;
    })}
    <Notice text={action.error} error />
  </>;
}

/** Celulares com acesso: liberados na conta, entram em todos os computadores dela; o liberado perde o acesso daqui. */
function AccountDevices() {
  const { rows, error } = useConnect((s) => s.devices);
  const self = useConnect((s) => s.device?.id);
  const { action, answer } = useAnswerDevice();
  // Pendente sobe para o topo da tela; revogado fica no serviço só para as máquinas o tirarem.
  const released = (rows ?? []).filter((row) => row.status === 'approved');
  return <>
    <SectionCard icon="Smartphone" title={m.connect_devices_all()} subtitle={m.connect_phones_help()}>
      {rows === null && !error ? <MachineRow title={m.comum_carregando()} /> : null}
      {rows && !released.length ? <MachineRow title={m.connect_phones_empty()} /> : null}
      {released.map((row) => (row.device_id === self
        ? <MachineRow key={row.device_id} title={Constants.deviceName || row.name} value={m.connect_device_this()} />
        : <MachineRow key={row.device_id} title={row.name}
          description={row.approved_at ? m.connect_phone_released({ when: relativeTime(row.approved_at) }) : undefined}
          value={m.connect_device_revoke()} disabled={action.busy}
          onPress={() => confirm(m.connect_device_revoke_title({ name: row.name }), m.connect_account_device_revoke_body(),
            m.connect_device_revoke(), () => answer(row, false), true)} />))}
    </SectionCard>
    {/* Com a lista antiga na tela, o erro avisa que ela pode estar velha. */}
    <Notice text={error} error />
    {error ? <MachineGroup>
      <MachineRow title={m.mobile_machines_retry()} action onPress={() => { void useConnect.getState().refreshDevices(); }} />
    </MachineGroup> : null}
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
