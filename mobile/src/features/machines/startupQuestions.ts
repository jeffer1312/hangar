import { Alert, Linking } from 'react-native';
import { SETTINGS_CONSENT_SCOPE, TELEMETRY_CONSENT_SCOPE, putAutoReportEm, putTelemetry, registrarDiag, type PendingTelemetry, type Server } from '@hangar/core';
import { onTelemetrySaved } from '../../net/telemetrySaved';
import { toast } from '../../ui/Toast';
import * as m from '../../paraglide/messages';

// As perguntas da abertura ficam fora das linhas das Configurações: o `_layout` não carrega a folha nativa.

export const TELEMETRY_DOC_URL = 'https://github.com/jeffer1312/hangar/blob/main/docs/telemetria.md';
export const LEGACY_TELEMETRY_DOC_URL = `${TELEMETRY_DOC_URL}#servidores-legados`;

/** `scope` é o `consent_scope` lido da máquina: fora do 3, a máquina é anterior à opção B. */
export function openTelemetryDoc(scope?: number): void {
  Linking.openURL(scope === TELEMETRY_CONSENT_SCOPE ? TELEMETRY_DOC_URL : LEGACY_TELEMETRY_DOC_URL).catch(() => {
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

/** Grava a escolha e, aceita na máquina ativa, já manda o retrato das preferências do aparelho. */
export async function saveTelemetryChoice(server: Server, value: boolean, scope: number | undefined) {
  const saved = await putTelemetry(server, value, scope);
  onTelemetrySaved(server, saved);
  return saved;
}

/** O aviso da abertura: só "Pode enviar" e "Não enviar" gravam, em todas as máquinas de `pending`.
 *  Fechar sem escolher deixa sem resposta, e a pergunta volta na próxima abertura. */
export async function askTelemetry(pending: readonly PendingTelemetry[]): Promise<void> {
  // Um aviso por texto: as máquinas da opção B primeiro, depois as anteriores a ela.
  for (const scope of [TELEMETRY_CONSENT_SCOPE, SETTINGS_CONSENT_SCOPE, undefined]) {
    const group = pending.filter(({ shown }) => shown.consent_scope === scope && !shown.disabled_by);
    if (!group.length) continue;
    // O nome repetido em outro aviso também confunde: o endereço desambigua contra todas as máquinas.
    const labels = group.map(({ server }) => pending.filter((item) => item.server.label === server.label).length > 1
      ? `${server.label} (${new URL(server.baseUrl).host})` : server.label);
    await confirmTelemetry(labels.join(', '), scope, (value) => {
      void Promise.allSettled(group.map(({ server, shown }) => saveTelemetryChoice(server, value, shown.consent_scope))).then((results) => {
        const failed = labels.filter((_, i) => results[i].status === 'rejected');
        if (failed.length) toast.erro(m.auto_report_save_failed({ machines: failed.join(', ') }));
      });
    });
  }
}

/** Mesma explicação na abertura e no ajuste; fechar sem escolher não grava. */
export function confirmTelemetry(machines: string, scope: number | undefined, answer: (value: boolean) => void): Promise<void> {
  const body = scope === TELEMETRY_CONSENT_SCOPE ? m.telemetry_b_question_body({ machines })
    : scope === SETTINGS_CONSENT_SCOPE ? m.telemetry_question_body({ machines }) : m.telemetry_legacy_question_body({ machines });
  return new Promise((done) => {
    const show = () => Alert.alert(m.telemetry_question_title(), body, [
      { text: m.telemetry_what_is_sent(), onPress: () => { openTelemetryDoc(scope); show(); } },
      { text: m.telemetry_no(), style: 'cancel', onPress: () => { answer(false); done(); } },
      { text: m.telemetry_yes(), onPress: () => { answer(true); done(); } },
    ], { cancelable: true, onDismiss: () => done() });
    show();
  });
}
