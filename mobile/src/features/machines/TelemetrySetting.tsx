import { useEffect, useState } from 'react';
import { getTelemetry, putTelemetry, type Server, type TelemetrySetting as Telemetry } from '@hangar/core';
import { Notice, useMachineAction } from './parts';
import { MachineGroup, MachineRow } from './MachineRows';
import { openTelemetryDoc } from './startupQuestions';
import * as m from '../../paraglide/messages';

// Contagens anônimas desta máquina. Servidor sem telemetria (404/503) não mostra a linha.
export function TelemetrySetting({ server }: { server: Server }) {
  // `undefined` = ainda sem leitura boa.
  const [state, setState] = useState<Telemetry | undefined>(undefined);
  const [missing, setMissing] = useState(false);
  const read = useMachineAction();
  const save = useMachineAction();
  const load = () => read.run(() => getTelemetry(server), (value) => { setMissing(value === undefined); if (value) setState(value); });
  useEffect(() => { void load(); }, [server.id, server.baseUrl, server.token]);
  if (missing) return null;
  const off = !!state?.disabled_by;
  const consent = state?.consent;
  const value = save.busy ? m.native_server_saving() : !state ? read.error ? undefined : m.native_server_loading()
    : off || consent === false ? m.auto_report_off() : consent === null ? m.auto_report_value_unanswered() : m.auto_report_on();
  return <>
    <MachineGroup>
      <MachineRow title={m.telemetry_setting()} description={off ? m.telemetry_disabled_env() : m.telemetry_setting_hint()} value={value}
        checked={!state || save.busy || (!off && consent === null) ? 'mixed' : !off && consent === true}
        disabled={!state || off || read.busy || save.busy}
        onPress={() => { if (state && !off) void save.run(() => putTelemetry(server, consent !== true), setState); }} />
      <MachineRow title={m.telemetry_what_is_sent()} action onPress={openTelemetryDoc} />
      {read.error ? <MachineRow title={m.mobile_machines_retry()} action onPress={load} disabled={read.busy} /> : null}
    </MachineGroup>
    <Notice text={read.error || save.error} error />
  </>;
}
