import { useEffect, useState } from 'react';
import { getAutoReportEm, putAutoReportEm, type Server } from '@hangar/core';
import { Notice, useMachineAction } from './parts';
import { MachineGroup, MachineRow } from './MachineRows';
import * as m from '../../paraglide/messages';

// Relatório automático da atualização desta máquina. `null` = ainda não respondido.
export function AutoReportSetting({ server }: { server: Server }) {
  // `undefined` = ainda sem leitura boa; `false` vindo de servidor sem a rota vira `missing`.
  const [answer, setAnswer] = useState<boolean | null | undefined>(undefined);
  const [missing, setMissing] = useState(false);
  const read = useMachineAction();
  const save = useMachineAction();
  const load = () => read.run(() => getAutoReportEm(server), (value) => { setMissing(value === undefined); setAnswer(value ?? null); });
  useEffect(() => { void load(); }, [server.id, server.baseUrl, server.token]);
  if (missing) return null;
  const value = save.busy ? m.native_server_saving() : answer === undefined ? read.error ? undefined : m.native_server_loading()
    : answer === null ? m.auto_report_value_unanswered() : answer ? m.auto_report_on() : m.auto_report_off();
  return <>
    <MachineGroup>
      <MachineRow title={m.auto_report_setting()} description={m.auto_report_setting_hint()} value={value}
        checked={answer === undefined || answer === null || save.busy ? 'mixed' : answer}
        disabled={answer === undefined || read.busy || save.busy}
        onPress={() => { if (answer !== undefined) void save.run(() => putAutoReportEm(server, answer !== true), setAnswer); }} />
      {read.error ? <MachineRow title={m.mobile_machines_retry()} action onPress={load} disabled={read.busy} /> : null}
    </MachineGroup>
    <Notice text={read.error || save.error} error />
  </>;
}
