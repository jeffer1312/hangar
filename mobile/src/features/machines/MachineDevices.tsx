import { useEffect, useState } from 'react';
import { Alert } from 'react-native';
import { relativeTime, type Server } from '@hangar/core';
import { SectionCard } from '../config/SectionCard';
import { approveDevice, readDevices, removeDevice, type LocalDevice } from './api';
import { MachineGroup, MachineHelp, MachineRow } from './MachineRows';
import { Notice, useMachineAction } from './parts';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

/**
 * Aparelhos que entraram só nesta máquina: aprovados nela sem a conta do Connect e pedidos de app antigo. Os que
 * vieram pela conta saem e entram pela lista da conta, então ficam fora daqui.
 */
export function MachineDevices({ server }: { server: Server }) {
  const [rows, setRows] = useState<LocalDevice[] | null>(null);
  const read = useMachineAction();
  const answer = useMachineAction();
  const load = () => read.run((signal) => readDevices(server, signal), setRows);
  useEffect(() => { void load(); }, []);

  const run = (row: LocalDevice, approve: boolean) => {
    haptic.selection();
    void answer.run(async (signal) => {
      try {
        await (approve ? approveDevice : removeDevice)(server, row.device_id, signal);
      } catch (error) { haptic.error(); throw error; }
      haptic.success();
      return readDevices(server, signal);
    }, setRows);
  };
  const askApprove = (row: LocalDevice) => Alert.alert(m.machine_devices_approve_title({ name: row.name }), m.machine_devices_approve_body(), [
    { text: m.comum_cancelar(), style: 'cancel' },
    { text: m.connect_device_deny(), style: 'destructive', onPress: () => run(row, false) },
    { text: m.connect_device_approve(), onPress: () => run(row, true) },
  ]);
  const askRevoke = (row: LocalDevice) => Alert.alert(m.connect_device_revoke_title({ name: row.name }), m.machine_devices_revoke_body(), [
    { text: m.comum_cancelar(), style: 'cancel' },
    { text: m.connect_device_revoke(), style: 'destructive', onPress: () => run(row, false) },
  ]);

  // Recusado só fica até o prazo para o próprio aparelho ler a recusa.
  const local = rows?.filter((row) => !row.account_device && row.status !== 'denied') ?? [];
  const pending = local.filter((row) => row.status === 'pending');
  const approved = local.filter((row) => row.status === 'approved');
  const busy = read.busy || answer.busy;
  const asked = (row: LocalDevice) => {
    const when = relativeTime(row.created_at);
    return row.install_code ? m.machine_devices_asked_code({ code: row.install_code, when }) : m.machine_devices_asked({ when });
  };

  return <>
    {rows === null && !read.error ? <MachineGroup><MachineRow title={m.comum_carregando()} /></MachineGroup> : null}
    {rows !== null && !local.length ? <MachineGroup><MachineRow title={m.machine_devices_empty()} /></MachineGroup> : null}
    {pending.length ? <SectionCard icon="Hourglass" title={m.machine_devices_pending()}>
      {pending.map((row) => <MachineRow key={row.device_id} title={row.name} description={asked(row)}
        value={m.connect_device_approve()} disabled={busy} onPress={() => askApprove(row)} />)}
    </SectionCard> : null}
    {approved.length ? <SectionCard icon="Smartphone" title={m.machine_devices_approved()}>
      {approved.map((row) => (row.device_id === server.deviceId
        ? <MachineRow key={row.device_id} title={row.name} value={m.connect_device_this()} />
        : <MachineRow key={row.device_id} title={row.name}
          description={row.last_seen ? m.machine_devices_seen({ when: relativeTime(row.last_seen) }) : undefined}
          value={m.connect_device_revoke()} disabled={busy} onPress={() => askRevoke(row)} />))}
    </SectionCard> : null}
    {/* Com a lista antiga na tela, o erro avisa que ela pode estar velha. */}
    <Notice text={read.error || answer.error} error />
    {read.error ? <MachineGroup>
      <MachineRow title={m.mobile_machines_retry()} action disabled={busy} onPress={() => { void load(); }} />
    </MachineGroup> : null}
    <MachineHelp>{m.machine_devices_help()}</MachineHelp>
  </>;
}
