import { Alert, Linking } from 'react-native';
import { putAutoReportEm, putTelemetry, registrarDiag, type Server } from '@hangar/core';
import { toast } from '../../ui/Toast';
import * as m from '../../paraglide/messages';

// As perguntas da abertura ficam fora das linhas das Configurações: o `_layout` não carrega a folha nativa.

export const TELEMETRY_DOC_URL = 'https://github.com/jeffer1312/hangar/blob/main/docs/telemetria.md';

export function openTelemetryDoc(): void {
  Linking.openURL(TELEMETRY_DOC_URL).catch(() => {
    registrarDiag({ evento: 'telemetry.doc_open_failed', nivel: 'erro', codigo: 'telemetry_doc_open_failed' });
    toast.erro(m.telemetry_doc_open_failed());
  });
}

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

/** O aviso da abertura: só "Pode enviar" e "Não enviar" gravam, em todas as máquinas de `pending`.
 *  Fechar sem escolher deixa sem resposta, e a pergunta volta na próxima abertura. */
export function askTelemetry(pending: readonly Server[]): Promise<void> {
  const machines = pending.map((s) => s.label).join(', ');
  const answer = (value: boolean) => saveAll(pending, (server) => putTelemetry(server, value));
  return new Promise((done) => {
    const show = () => Alert.alert(m.telemetry_question_title(), m.telemetry_question_body({ machines }), [
      // Abrir a lista fecha o aviso; ele volta para a resposta.
      { text: m.telemetry_what_is_sent(), onPress: () => { openTelemetryDoc(); show(); } },
      { text: m.telemetry_no(), style: 'cancel', onPress: () => { answer(false); done(); } },
      { text: m.telemetry_yes(), onPress: () => { answer(true); done(); } },
    ], { cancelable: true, onDismiss: () => done() });
    show();
  });
}
