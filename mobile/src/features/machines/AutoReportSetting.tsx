import { useEffect, useState } from 'react';
import { Alert } from 'react-native';
import { getAutoReportEm, putAutoReportEm, type Server } from '@hangar/core';
import { Notice, useMachineAction } from './parts';
import { MachineGroup, MachineRow } from './MachineRows';
import { toast } from '../../ui/Toast';
import * as m from '../../paraglide/messages';

/** Grava a resposta de um aviso em todas as máquinas de `pending`; as que recusarem saem num toast. */
export function saveAll(pending: readonly Server[], put: (server: Server) => Promise<unknown>): void {
  void Promise.allSettled(pending.map(put)).then((results) => {
    const failed = pending.filter((_, i) => results[i].status === 'rejected');
    if (failed.length) toast.erro(m.auto_report_save_failed({ machines: failed.map((s) => s.label).join(', ') }));
  });
}

/** O aviso da abertura do relatório automático; resolve quando a pessoa responde. */
export function askAutoReport(pending: readonly Server[]): Promise<void> {
  return new Promise((done) => {
    const answer = (value: boolean) => { done(); saveAll(pending, (server) => putAutoReportEm(server, value)); };
    Alert.alert(m.auto_report_question_title(), m.auto_report_question_body({ machines: pending.map((s) => s.label).join(', ') }), [
      { text: m.auto_report_no(), style: 'cancel', onPress: () => answer(false) },
      { text: m.auto_report_yes(), onPress: () => answer(true) },
    ]);
  });
}

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
