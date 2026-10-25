import { useEffect, useState } from 'react';
import type { Server } from '@hangar/core';
import { webUi } from './api';
import { Notice, useMachineAction } from './parts';
import { MachineGroup, MachineRow } from './MachineRows';
import * as m from '../../paraglide/messages';

// Só no app e no nativo: desligar pelo PWA desligaria a própria tela que a pessoa está usando.
export function WebUiSetting({ server }: { server: Server }) {
  // `undefined` = ainda sem leitura boa; `null` = servidor sem a rota.
  const [on, setOn] = useState<boolean | null | undefined>(undefined);
  const read = useMachineAction();
  const save = useMachineAction();
  const load = () => read.run((signal) => webUi(server, signal), setOn);
  useEffect(() => { void load(); }, [server.id, server.baseUrl, server.token]);
  if (on === null) return null;
  const value = save.busy ? m.native_server_saving() : on === undefined ? read.error ? undefined : m.native_server_loading()
    : on ? m.native_web_ui_on() : m.native_web_ui_off();
  return <>
    <MachineGroup>
      <MachineRow title={m.native_web_ui_title()} description={m.native_web_ui_help()} value={value} checked={on === undefined || save.busy ? 'mixed' : on}
        disabled={on === undefined || read.busy || save.busy}
        onPress={() => { if (on !== undefined) void save.run((signal) => webUi(server, signal, !on), setOn); }} />
      {read.error ? <MachineRow title={m.mobile_machines_retry()} action onPress={load} disabled={read.busy} /> : null}
    </MachineGroup>
    <Notice text={read.error || save.error} error />
  </>;
}
