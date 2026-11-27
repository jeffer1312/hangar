import { useEffect, useState } from 'react';
import { Alert, Linking } from 'react-native';
import { getTelemetry, putTelemetry, registrarDiag, type Server, type TelemetrySetting as Telemetry } from '@hangar/core';
import { Notice, useMachineAction } from './parts';
import { MachineGroup, MachineRow } from './MachineRows';
import { saveAll } from './AutoReportSetting';
import { toast } from '../../ui/Toast';
import * as m from '../../paraglide/messages';

export const TELEMETRY_DOC_URL = 'https://github.com/jeffer1312/hangar/blob/main/docs/telemetria.md';

const openDoc = () => {
  Linking.openURL(TELEMETRY_DOC_URL).catch(() => {
    registrarDiag({ evento: 'telemetry.doc_open_failed', nivel: 'erro', codigo: 'telemetry_doc_open_failed' });
    toast.erro(m.telemetry_doc_open_failed());
  });
};

/** O aviso da abertura: só "Pode enviar" e "Não enviar" gravam, em todas as máquinas de `pending`.
 *  Fechar sem escolher deixa sem resposta, e a pergunta volta na próxima abertura. */
export function askTelemetry(pending: readonly Server[]): Promise<void> {
  const machines = pending.map((s) => s.label).join(', ');
  const answer = (value: boolean) => saveAll(pending, (server) => putTelemetry(server, value));
  return new Promise((done) => {
    const show = () => Alert.alert(m.telemetry_question_title(), m.telemetry_question_body({ machines }), [
      // Abrir a lista fecha o aviso; ele volta para a resposta.
      { text: m.telemetry_what_is_sent(), onPress: () => { openDoc(); show(); } },
      { text: m.telemetry_no(), style: 'cancel', onPress: () => { answer(false); done(); } },
      { text: m.telemetry_yes(), onPress: () => { answer(true); done(); } },
    ], { cancelable: true, onDismiss: () => done() });
    show();
  });
}

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
      <MachineRow title={m.telemetry_what_is_sent()} action onPress={openDoc} />
      {read.error ? <MachineRow title={m.mobile_machines_retry()} action onPress={load} disabled={read.busy} /> : null}
    </MachineGroup>
    <Notice text={read.error || save.error} error />
  </>;
}
