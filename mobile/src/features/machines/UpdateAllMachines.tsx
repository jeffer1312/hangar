import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { InteractionManager, Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { estaDesligado, isServerParked, ownUpdatableServers, prepareUpdateAll, updateAll, updateReportLine } from '@hangar/core';
import type { Server, UpdateAllResult, UpdateAllState } from '@hangar/core';
import { SectionCard } from '../config/SectionCard';
import { useSettingsColors } from '../config/colors';
import { MachineSheet, Notice } from './parts';
import { MachineRow } from './MachineRows';
import { UpdateRow, UpdateSummary, type RowTone } from './UpdateAllRows';
import { haptic } from '../../ui/haptics';
import { useRouter } from 'expo-router';
import { useServers } from '../../stores/servers';
import { createMachineChecks, loadArrivedNews, requestUpdateConfirm, whatsNewHref } from '../updates/pending';
import * as m from '../../paraglide/messages';

function statusText(value: UpdateAllState): string {
  switch (value.status) {
    case 'consultando': return m.update_all_checking();
    case 'iniciando': return m.update_all_starting();
    case 'rodando': return m.update_all_running({ texto: value.state?.texto ?? '', passo: String(value.state?.passo ?? 0), total: String(value.state?.total ?? 0) });
    case 'esperando': return m.update_all_waiting();
    case 'em_dia': return m.update_all_up_to_date();
    case 'travado':
      switch (value.reason) {
        case 'branch': return m.update_all_held_branch();
        case 'mudancas': return m.update_all_held_changes();
        case 'rodando': return m.update_all_held_running();
        default: return m.update_all_held_missing();
      }
    case 'fora_do_ar': return m.update_all_offline();
    case 'atualizado': return m.update_all_updated();
    case 'atualizado_manual': return m.update_all_updated_manual();
    case 'falhou': {
      const failed = m.update_all_failed({ erro: value.errorCode === 'silent' ? m.update_all_silent()
        : value.errorCode === 'timeout' ? m.update_all_timeout() : value.error || m.erro_desconhecido() });
      const report = updateReportLine(value.state);
      return report ? `${failed} · ${report}` : failed;
    }
  }
}

const needsAttention = (value: UpdateAllState) => ['falhou', 'fora_do_ar', 'travado'].includes(value.status);
const running = (value: UpdateAllState) => ['consultando', 'iniciando', 'rodando', 'esperando'].includes(value.status);

function rowTone(value: UpdateAllState): RowTone {
  if (running(value)) return 'busy';
  if (value.status === 'falhou') return 'error';
  return needsAttention(value) || value.status === 'atualizado_manual' ? 'warn' : 'ok';
}

const plural = (n: number, one: () => string, other: (p: { n: string }) => string) => n === 1 ? one() : other({ n: String(n) });

/** "Tudo pronto · 2 atualizadas, 1 fora do ar": a máquina deixada de fora antes não conta como problema. */
function doneSummary(rows: readonly UpdateAllResult[], skipped: number): string {
  const count = (statuses: string[]) => rows.filter((row) => statuses.includes(row.state.status)).length;
  const updated = count(['atualizado', 'atualizado_manual']);
  const current = count(['em_dia']);
  const offline = count(['fora_do_ar']) + skipped;
  const issues = count(['falhou', 'travado']);
  const parts = [
    updated ? plural(updated, m.update_all_count_updated_one, m.update_all_count_updated_other) : '',
    current ? plural(current, m.update_all_count_current_one, m.update_all_count_current_other) : '',
    offline ? plural(offline, m.update_all_count_offline_one, m.update_all_count_offline_other) : '',
    issues ? plural(issues, m.update_all_count_issue_one, m.update_all_count_issue_other) : '',
  ].filter(Boolean).join(', ');
  const head = issues || count(['fora_do_ar']) ? m.update_all_done_issues() : m.update_all_done_ok();
  return parts ? `${head} · ${parts}` : head;
}

// Já sabida fora do ar: fica de fora sem esperar o prazo da identidade.
const knownOffline = (id: string) => estaDesligado(id) || isServerParked(id);

// A rodada vive fora do componente: sair da tela não a para, e voltar mostra a mesma em vez de abrir outra.
// `left`: as que falharam antes da folha (sem a busca ou sem os avisos) e ficaram de fora; contam como problema.
// `skipped`: as fora do ar deixadas de fora antes da folha. As duas aparecem no fim da lista.
type Round = { busy: boolean; count: number; rows: UpdateAllResult[]; left: UpdateAllResult[]; skipped: Server[]; error: string };
let round: Round = { busy: false, count: 0, rows: [], left: [], skipped: [], error: '' };
const listeners = new Set<() => void>();
const setRound = (patch: Partial<Round>) => { round = { ...round, ...patch }; listeners.forEach((listener) => listener()); };
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };

export function UpdateAllMachines({ servers, compact = false }: { servers: readonly Server[]; compact?: boolean }) {
  const c = useSettingsColors();
  const candidates = ownUpdatableServers(servers);
  const [open, setOpen] = useState(false);
  const newsAfterClose = useRef<string | null>(null);
  const openRef = useRef(open);
  useEffect(() => { openRef.current = open; }, [open]);
  const router = useRouter();
  const activeId = useServers((s) => s.activeId);
  const { busy, count, rows, left, skipped, error } = useSyncExternalStore(subscribe, () => round);

  const start = async () => {
    if (round.busy) { setOpen(true); return; }
    // As novidades são as do servidor ativo (a versão nova é a mesma); os avisos, os de cada máquina.
    const notesServer = candidates.find((s) => s.id === activeId) ?? candidates[0];
    if (!notesServer) return;
    // A folha abre na hora com uma linha por máquina; cada uma se preenche quando a máquina responde.
    const checks = createMachineChecks(candidates);
    const prepared = prepareUpdateAll(candidates, { knownOffline, onCheck: checks.set,
      onTargets: (targets) => checks.keep(targets) });
    const accept = await requestUpdateConfirm(notesServer, () => router.push('/update-confirm' as never), candidates.length, prepared, checks);
    if (!accept) return;
    const ready = await prepared;
    // Quem não mostrou os avisos fica fora da rodada: atualizá-la passaria por cima do aceite.
    const targets = ready.run.map((entry) => entry.server);
    // A folha já trava o Atualizar sem máquina para rodar; isto só cobre a corrida.
    if (!targets.length) return;
    setOpen(true);
    if (round.busy) return;
    setRound({ busy: true, count: targets.length, skipped: ready.offline, error: '',
      left: ready.failed.filter((server) => !ready.offline.includes(server)).map((server) => ({ server,
        state: { status: 'falhou', error: ready.reasons?.[server.id] || m.update_notes_failed(), warnings: [] } })),
      rows: targets.map((server) => ({ server, state: { status: 'consultando', warnings: [] } })) });
    try {
      // A identidade e a busca do alvo já foram feitas antes da folha: a rodada não as repete.
      const results = await updateAll(targets, (id, state) => {
        const server = candidates.find((entry) => entry.id === id);
        if (!server) return;
        setRound({ rows: round.rows.some((row) => row.server.id === id)
          ? round.rows.map((row) => row.server.id === id ? { server, state } : row)
          : [...round.rows, { server, state }] });
      }, { accept, prepared: ready });
      setRound({ rows: results });
      if (!results.length || round.left.length || results.some((row) => needsAttention(row.state))) haptic.error();
      else haptic.success();
      const done = results.find((row) => row.server.id === notesServer.id);
      // Abre ao fechar a folha de resultados: por cima dela, no iOS, ficaria escondida e a lista se perderia.
      if (done && (done.state.status === 'atualizado' || done.state.status === 'atualizado_manual')) void loadArrivedNews(notesServer).then((show) => {
        if (!show) return;
        if (openRef.current) newsAfterClose.current = notesServer.id;
        else router.push(whatsNewHref(notesServer.id));
      });
    } catch (cause) {
      // A rodada caiu: a linha que ainda esperava não vai terminar, e leva o motivo da queda.
      const reason = cause instanceof Error ? cause.message : String(cause);
      setRound({ error: m.update_all_failed({ erro: reason }), rows: round.rows.map((row) => running(row.state)
        ? { ...row, state: { ...row.state, status: 'falhou', error: reason, errorCode: undefined } } : row) });
      haptic.error();
    } finally {
      setRound({ busy: false });
    }
  };

  if (candidates.length < 2 && !busy && !open) return null;
  const buttonLabel = m.update_all_button({ n: String(busy ? count : candidates.length) });
  const finished = rows.filter((row) => !running(row.state)).length;
  const done = !busy && !error && rows.length > 0;
  const closeSheet = () => {
    setOpen(false);
    const id = newsAfterClose.current;
    newsAfterClose.current = null;
    if (id) InteractionManager.runAfterInteractions(() => router.push(whatsNewHref(id)));
  };
  return <>
    {candidates.length >= 2 || busy ? compact ? <MachineRow title={buttonLabel} onPress={() => void start()} /> : <Pressable accessibilityRole="button" accessibilityLabel={buttonLabel}
      onPress={() => void start()} style={({ pressed }) => [styles.button, { borderColor: c.borderStrong }, pressed && { backgroundColor: c.hover }]}>
      <Text style={[styles.label, { color: c.text }]}>{buttonLabel}</Text>
    </Pressable> : null}
    {open ? <MachineSheet title={done ? m.update_all_done_title() : m.update_all_title()} onClose={closeSheet}>
      {busy && rows.length === 0 ? <Notice text={m.update_all_checking()} /> : null}
      <Notice text={error} error />
      {!busy && !error && rows.length === 0 ? <Notice text={m.maquinas_vazio()} /> : null}
      {rows.length ? <UpdateSummary progress={done ? 1 : finished / rows.length}
        text={done ? doneSummary([...rows, ...left], skipped.length) : m.update_all_summary_progress({ done: String(finished), total: String(rows.length) })} /> : null}
      {rows.length || left.length || skipped.length ? <SectionCard>
        {rows.map(({ server, state }) => <View key={server.id} style={styles.row}>
          <UpdateRow label={server.label} text={statusText(state)} tone={rowTone(state)}
            detail={state.status === 'travado' && state.reason === 'falta' && state.preflight?.faltando.length
              ? state.preflight.faltando.join(', ') : undefined} />
          {state.warnings.length ? <View style={styles.warnings}>
            <Notice text={m.update_all_warnings()} />
            {state.warnings.map((warning, index) => <Notice key={index} text={warning} />)}
          </View> : null}
        </View>)}
        {left.map(({ server, state }) => <View key={server.id} style={styles.row}>
          <UpdateRow label={server.label} text={statusText(state)} tone="error" />
        </View>)}
        {skipped.map((server) => <View key={server.id} style={styles.row}>
          <UpdateRow label={server.label} text={m.update_all_offline_skipped()} tone="warn" />
        </View>)}
      </SectionCard> : null}
      {rows.length ? <Pressable accessibilityRole="button" onPress={closeSheet}
        style={({ pressed }) => [styles.close, done ? { backgroundColor: c.accent } : { borderWidth: 1, borderColor: c.borderStrong },
          pressed && { opacity: 0.8 }]}>
        <Text style={[styles.label, { color: done ? c.accentText : c.text }]}>
          {done ? m.update_all_close() : m.update_all_close_background()}
        </Text>
      </Pressable> : null}
    </MachineSheet> : null}
  </>;
}

const styles = StyleSheet.create((theme) => ({
  button: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderWidth: 1, borderRadius: theme.base.radius.md },
  label: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  warnings: { gap: theme.base.space[2], marginTop: theme.base.space[2] },
  row: { paddingHorizontal: theme.base.space[3] },
  close: { minHeight: 44, alignItems: 'center', justifyContent: 'center', borderRadius: theme.base.radius.md,
    paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[2] },
}));
