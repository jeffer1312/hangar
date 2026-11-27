import { Alert, Linking } from 'react-native';
import { putAutoReportEm, putTelemetry, registrarDiag, SETTINGS_CONSENT_SCOPE, type Server } from '@hangar/core';
import { onTelemetrySaved } from '../../net/telemetrySaved';
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
export function saveAll(pending: readonly Server[], put: (server: Server) => Promise<unknown>, name = (server: Server) => server.label): void {
  void Promise.allSettled(pending.map(put)).then((results) => {
    const failed = pending.filter((_, i) => results[i].status === 'rejected');
    if (failed.length) toast.erro(m.auto_report_save_failed({ machines: failed.map(name).join(', ') }));
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

export async function saveTelemetryChoice(server: Server, value: boolean, scope: number | undefined) {
  const saved = await putTelemetry(server, value, scope);
  onTelemetrySaved(server, saved);
  return saved;
}

export function telemetryTarget(server: Server, registered: readonly Server[]): string {
  return registered.some((other) => other.id !== server.id && other.label === server.label)
    ? `${server.label} (${server.baseUrl})` : server.label;
}

export function showTelemetryNotice(machines: string): Promise<boolean | undefined> {
  const body = m.telemetry_question_body({ machines });
  return new Promise((done) => {
    const show = () => Alert.alert(m.telemetry_question_title(), body, [
      // Abrir a lista fecha o aviso; ele volta para a resposta.
      { text: m.telemetry_what_is_sent(), onPress: () => { openTelemetryDoc(); show(); } },
      { text: m.telemetry_no(), style: 'cancel', onPress: () => done(false) },
      { text: m.telemetry_yes(), onPress: () => done(true) },
    ], { cancelable: true, onDismiss: () => done(undefined) });
    show();
  });
}

/** Só recebe máquinas cujo GET anunciou o aviso do escopo 2. Fechar sem escolher não grava. */
export async function askTelemetry(servers: readonly Server[], registered: readonly Server[]): Promise<void> {
  const name = (server: Server) => telemetryTarget(server, registered);
  const answer = await showTelemetryNotice(servers.map(name).join(', '));
  if (answer !== undefined) saveAll(servers, (server) => saveTelemetryChoice(server, answer, SETTINGS_CONSENT_SCOPE), name);
}
