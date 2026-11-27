import { useEffect, useState } from 'react';
import { TELEMETRY_CONSENT_SCOPE, getTelemetry, knowsTelemetryScope, putTelemetry, type Server, type TelemetrySetting as Telemetry } from '@hangar/core';
import { Notice, useMachineAction } from './parts';
import { MachineGroup, MachineRow } from './MachineRows';
import { confirmTelemetry, openTelemetryDoc } from './startupQuestions';
import * as m from '../../paraglide/messages';

// Contagens anônimas desta máquina. Servidor sem telemetria (404/503) não mostra a linha.
export function TelemetrySetting({ server }: { server: Server }) {
  // `undefined` = ainda sem leitura boa.
  const [loaded, setLoaded] = useState<{ server: Server; value: Telemetry | undefined }>();
  const read = useMachineAction();
  const save = useMachineAction();
  const apply = (value: Telemetry | undefined) => setLoaded({ server, value });
  const load = () => read.run(() => getTelemetry(server), apply);
  useEffect(() => { void load(); }, [server.id, server.baseUrl, server.token]);
  const current = loaded?.server.id === server.id && loaded.server.baseUrl === server.baseUrl && loaded.server.token === server.token;
  const state = current ? loaded.value : undefined;
  if (current && !state) return null;
  const off = !!state?.disabled_by;
  const consent = state?.consent;
  const scope = state?.consent_scope;
  const known = state !== undefined && knowsTelemetryScope(scope);
  const canChange = state && (consent === true || (!off && known));
  const value = save.busy ? m.native_server_saving() : !state ? read.error ? undefined : m.native_server_loading()
    : consent === false ? m.auto_report_off() : consent === null ? m.auto_report_value_unanswered() : m.auto_report_on();
  const hint = !state ? undefined : off ? m.telemetry_disabled_env() : !known ? m.telemetry_scope_unknown()
    : scope === TELEMETRY_CONSENT_SCOPE ? m.telemetry_b_setting_hint() : m.telemetry_setting_hint();
  const change = () => {
    if (!state || !canChange) return;
    if (state.consent === true) { void save.run(() => putTelemetry(server, false, state), apply); return; }
    const label = `${server.label} (${new URL(server.baseUrl).host})`;
    void confirmTelemetry(label, scope, (yes) => { if (yes) void save.run(() => putTelemetry(server, true, state), apply); });
  };
  return <>
    <MachineGroup>
      <MachineRow title={m.telemetry_setting()} description={hint} value={value}
        checked={!state || save.busy || consent === null ? 'mixed' : consent === true}
        disabled={!canChange || read.busy || save.busy}
        onPress={change} />
      {known ? <MachineRow title={m.telemetry_what_is_sent()} action onPress={() => openTelemetryDoc(scope)} /> : null}
      {read.error ? <MachineRow title={m.mobile_machines_retry()} action onPress={load} disabled={read.busy} /> : null}
    </MachineGroup>
    <Notice text={read.error || save.error} error />
  </>;
}
