import { useEffect, useState } from 'react';
import { getTelemetry, knowsTelemetryScope, type Server, type TelemetrySetting as Telemetry } from '@hangar/core';
import { Notice, useMachineAction } from './parts';
import { MachineGroup, MachineRow } from './MachineRows';
import { openTelemetryDoc, saveTelemetryChoice } from './startupQuestions';
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
  const scope = state?.consent_scope;
  const known = !!state && knowsTelemetryScope(scope);
  const consent = state?.consent;
  const blocked = !!state && (!known || !!state.disabled_by || (consent === true && state.enabled !== true));
  const hint = !state ? undefined : !known ? m.telemetry_update_required() : state.disabled_by ? m.telemetry_disabled_env()
    : blocked ? m.telemetry_not_active() : m.telemetry_setting_hint();
  const value = save.busy ? m.native_server_saving() : !state ? read.error ? undefined : m.native_server_loading()
    : (blocked && known) || consent === false ? m.auto_report_off()
      : !known || consent === null ? m.auto_report_value_unanswered() : m.auto_report_on();
  return <>
    <MachineGroup>
      <MachineRow title={m.telemetry_setting()} description={hint} value={value}
        checked={!state || save.busy || (!blocked && consent === null) ? 'mixed' : !blocked && consent === true}
        disabled={!state || blocked || read.busy || save.busy}
        onPress={() => { if (state && !blocked) void save.run(() => saveTelemetryChoice(server, consent !== true, scope), setState); }} />
      {blocked && consent === true ? <MachineRow title={m.telemetry_revoke()} action disabled={read.busy || save.busy}
        onPress={() => { void save.run(() => saveTelemetryChoice(server, false, scope), setState); }} /> : null}
      {known ? <MachineRow title={m.telemetry_what_is_sent()} action onPress={openTelemetryDoc} /> : null}
      {read.error ? <MachineRow title={m.mobile_machines_retry()} action onPress={load} disabled={read.busy} /> : null}
    </MachineGroup>
    <Notice text={read.error || save.error} error />
  </>;
}
