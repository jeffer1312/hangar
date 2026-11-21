import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  adoptCliproxyForServer, getCliproxyInstallForServer, updateCliproxyForServer,
  type CliproxyInstall, type CliproxyJob, type Server,
} from '@hangar/core';
import { SectionCard } from '../config/SectionCard';
import { SettingsRow } from '../config/SettingsRow';
import { Pill } from '../config/PageHeader';
import { useSettingsColors } from '../config/colors';
import { InstallItems } from './InstallItems';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

// Quanto o "feito" fica na tela antes de sumir.
const DONE_FOR = 4000;
const UPDATE_AT: Record<string, number> = { download: 0, service: 1, start: 2 };
const ADOPT_AT: Record<string, number> = { download: 0, config: 1, service: 2, start: 3 };

/** As duas ações que reiniciam o proxy: trocar a versão do Hangar, ou passar o de fora para ele. */
export type Action = 'update' | 'adopt';
export type Outcome = { k: 'done'; action: Action; tag: string } | { k: 'failed'; action: Action; text: string } | { k: 'interrupted'; action: Action };

const actionJob = (install: CliproxyInstall | null): (CliproxyJob & { kind: Action }) | null =>
  install?.job && (install.job.kind === 'update' || install.job.kind === 'adopt') ? install.job as CliproxyJob & { kind: Action } : null;

/** O motivo de o proxy de fora não poder passar ao Hangar, pelo código do servidor. */
export function handoverText(code: string): string {
  switch (code) {
    case 'cliproxy_entrega_outro_config': return m.mobile_cliproxy_handover_other_config();
    case 'cliproxy_entrega_unit_de_sistema': return m.mobile_cliproxy_handover_system_unit();
    case 'cliproxy_entrega_pasta_das_contas': return m.mobile_cliproxy_handover_accounts_dir();
    case 'cliproxy_entrega_sem_servico': return m.mobile_cliproxy_handover_no_service();
    default: return m.mobile_cliproxy_handover_unknown({ code });
  }
}

/** O texto da falha pelo código da tarefa: só a volta que deu certo diz que o proxy segue funcionando. */
export function failureText(action: Action, error: { code: string; detail: string }, install: CliproxyInstall | null): string {
  const neu = install?.latest ?? '';
  const old = install?.version ?? '';
  if (error.code === 'cliproxy_tarefa_caiu') return m.mobile_cliproxy_wizard_err_task_crashed();
  if (action === 'adopt') {
    if (error.code === 'cliproxy_nao_subiu') return m.mobile_cliproxy_adopt_failed_back({ new: neu });
    if (error.code === 'cliproxy_entrega_sem_volta') return m.mobile_cliproxy_adopt_failed_lost();
    if (error.code.startsWith('cliproxy_entrega_')) return handoverText(error.code);
    return error.detail || error.code;
  }
  if (error.code === 'cliproxy_nao_subiu') return m.mobile_cliproxy_update_failed_back({ new: neu, old });
  if (error.code === 'cliproxy_atualizacao_sem_volta') return m.mobile_cliproxy_update_failed_lost({ old });
  return error.detail || error.code;
}

/** A versão que a linha do modelo mostra: só a do proxy que o Hangar instalou. */
export const managedVersion = (install: CliproxyInstall | null) => install?.state === 'managed' ? install.version : null;

/** Pergunta antes de reiniciar o proxy com sessões nele; `then` faz a ação confirmada. */
export function confirmRestart(action: Action, sessions: number, then: () => void) {
  const body = action === 'adopt'
    ? sessions === 1 ? m.mobile_cliproxy_adopt_confirm_one() : m.mobile_cliproxy_adopt_confirm_many({ n: String(sessions) })
    : sessions === 1 ? m.mobile_cliproxy_update_confirm_one() : m.mobile_cliproxy_update_confirm_many({ n: String(sessions) });
  Alert.alert(action === 'adopt' ? m.mobile_cliproxy_adopt_confirm_title() : m.mobile_cliproxy_update_confirm_title(), body, [
    { text: m.comum_cancelar(), style: 'cancel' },
    { text: action === 'adopt' ? m.mobile_cliproxy_adopt_confirm_ok() : m.mobile_cliproxy_update_confirm_ok(), style: 'destructive', onPress: then },
  ]);
}

/**
 * Estado do proxy lido ao abrir e recarregar a página; com a atualização ou a entrega em curso, relido a cada segundo.
 * Falha da leitura não vira aviso: só a ação que falha aparece.
 */
export function useProxyUpdate(server: Server | null) {
  const [install, setInstall] = useState<CliproxyInstall | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [posting, setPosting] = useState(false);
  const postingNow = useRef(false);
  const alive = useRef(true);
  const asked = useRef<Action | null>(null);
  const polling = useRef(false);
  const generation = useRef(0);
  const seq = useRef(0);
  const doneTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const read = useCallback(async (): Promise<CliproxyInstall | null> => {
    if (!server) return null;
    const turn = ++seq.current;
    try {
      const next = await getCliproxyInstallForServer(server);
      if (!alive.current || turn !== seq.current) return null;
      setInstall(next);
      return next;
    } catch (cause) {
      console.warn('cliproxy_install_read_failed', cause);
      return null;
    }
  }, [server?.id, server?.baseUrl, server?.token]);

  const done = useCallback((action: Action, tag: string) => {
    haptic.success();
    setOutcome({ k: 'done', action, tag });
    clearTimeout(doneTimer.current);
    doneTimer.current = setTimeout(() => { if (alive.current) setOutcome(o => o?.k === 'done' ? null : o); }, DONE_FOR);
  }, []);

  const finish = useCallback((next: CliproxyInstall) => {
    const action = asked.current;
    if (!action) return;
    const job = actionJob(next);
    // Servidor que reiniciou perde a tarefa: o estado relido diz se a troca entrou.
    if (!job) {
      asked.current = null;
      const entered = action === 'update' ? next.update_available === false : next.state === 'managed';
      if (entered && next.state === 'managed' && next.version) done(action, next.version);
      return;
    }
    if (!job.done || job.kind !== action) return;
    asked.current = null;
    if (!job.error) done(action, next.version ?? '');
    else if (job.error.code === 'erro_cliproxy_sessoes_ativas') setOutcome({ k: 'interrupted', action });
    else { haptic.error(); setOutcome({ k: 'failed', action, text: failureText(action, job.error, next) }); }
  }, [done]);

  const poll = useCallback(async () => {
    if (polling.current) return;
    polling.current = true;
    // Cada montagem do efeito tem a sua vez: o laço de um servidor anterior não segue no seguinte.
    const my = generation.current;
    const mine = () => alive.current && generation.current === my;
    try {
      while (mine()) {
        await new Promise(resolve => setTimeout(resolve, 1000));
        if (!mine()) return;
        // Leitura que falha (o servidor reinicia na troca) só espera a próxima volta.
        const next = await read();
        if (!next || !mine()) continue;
        const job = actionJob(next);
        if (!job || job.done) { finish(next); return; }
      }
    } finally { if (generation.current === my) polling.current = false; }
  }, [read, finish]);

  const reload = useCallback(async () => {
    if (polling.current) return;
    const next = await read();
    const job = actionJob(next);
    if (job && !job.done) void poll();
  }, [read, poll]);

  const start = useCallback(async (action: Action, confirm = false) => {
    // Trava síncrona: dois toques antes do primeiro voltar ainda veem o `posting` antigo.
    if (!server || postingNow.current) return;
    postingNow.current = true;
    setPosting(true);
    setOutcome(null);
    try {
      await (action === 'adopt' ? adoptCliproxyForServer : updateCliproxyForServer)(server, confirm);
      if (!alive.current) return;
      asked.current = action;
      void poll();
    } catch (cause) {
      if (!alive.current) return;
      const error = cause as { code?: string; envelope?: { params?: { sessions?: unknown } }; message?: string };
      if (error.code === 'erro_cliproxy_sessoes_ativas') {
        confirmRestart(action, Number(error.envelope?.params?.sessions) || 0, () => void start(action, true));
      } else if (error.code === 'erro_cliproxy_ocupado') {
        // Outra tarefa em curso: acompanha a que está rodando.
        asked.current = action;
        void poll();
      } else {
        haptic.error();
        setOutcome({ k: 'failed', action, text: error.message || m.erro_desconhecido() });
      }
    } finally {
      postingNow.current = false;
      if (alive.current) setPosting(false);
    }
  }, [server?.id, server?.baseUrl, server?.token, poll]);

  const update = useCallback((confirm = false) => start('update', confirm), [start]);
  const adopt = useCallback((confirm = false) => start('adopt', confirm), [start]);

  useEffect(() => {
    alive.current = true;
    void reload();
    return () => { alive.current = false; seq.current++; generation.current++; polling.current = false; clearTimeout(doneTimer.current); };
  }, [reload]);

  return { install, outcome, posting, reload, update, adopt };
}

export type ProxyUpdateState = ReturnType<typeof useProxyUpdate>;

const hostOf = (url: string) => url.replace(/^https?:\/\//, '').replace(/\/.*$/, '');

/**
 * A linha do CLIProxyAPI em Contas: versão, de quem é, se responde, contas e sessões, e as ações (Atualizar o do Hangar,
 * Deixar o Hangar cuidar do de fora, Conectar conta). Nada sem proxy nesta máquina ou antes da primeira leitura.
 */
export function ProxyCard({ state, onConnect }: {
  state: Pick<ProxyUpdateState, 'install' | 'outcome' | 'posting' | 'update' | 'adopt'>; onConnect: () => void;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const { install, outcome, posting, update, adopt } = state;
  if (install?.state !== 'managed' && install?.state !== 'external') return null;
  const external = install.state === 'external';
  const job = actionJob(install);
  const neu = install.latest ?? '';
  if (job && !job.done) {
    const adopting = job.kind === 'adopt';
    const labels = adopting
      ? [m.mobile_cliproxy_adopt_step_download({ new: neu }), m.mobile_cliproxy_adopt_step_config(), m.mobile_cliproxy_adopt_step_service(), m.mobile_cliproxy_adopt_step_start()]
      : [m.mobile_cliproxy_update_step_download(), m.mobile_cliproxy_update_step_service(), m.mobile_cliproxy_update_step_start()];
    return <SectionCard icon="Network" busy
      title={adopting ? m.mobile_cliproxy_adopt_running() : m.mobile_cliproxy_update_running({ new: neu })} subtitle={m.mobile_cliproxy_update_restarts()}>
      <View style={styles.body}><InstallItems at={(adopting ? ADOPT_AT : UPDATE_AT)[job.step] ?? 0} failed={false} labels={labels} /></View>
    </SectionCard>;
  }
  const count = (n: number, one: string, many: (p: { n: string }) => string) => n === 1 ? one : many({ n: String(n) });
  const subtitle = [
    install.running && install.base_url ? m.mobile_cliproxy_card_running({ url: hostOf(install.base_url) }) : m.mobile_cliproxy_card_stopped(),
    install.sessions ? count(install.sessions, m.mobile_cliproxy_card_sessions_one(), m.mobile_cliproxy_card_sessions_many) : null,
  ].filter(Boolean).join(' · ');
  const title = install.version ? `CLIProxyAPI ${install.version}` : 'CLIProxyAPI';
  const failed = outcome?.k === 'failed';
  const showUpdate = !external && install.update_available;
  const handover = external ? install.handover : null;
  const interrupted = (action: Action) => outcome?.k === 'interrupted' && outcome.action === action;
  return <SectionCard icon={failed ? 'CircleAlert' : 'Network'} tone={failed ? 'danger' : undefined} title={title} subtitle={subtitle}
    extra={showUpdate ? <Pill label={m.mobile_cliproxy_update_button()} onPress={() => void update(false)} disabled={posting} /> : undefined}>
    {outcome?.k === 'done'
      ? <View style={styles.body}><Text style={[styles.note, { color: c.text }]} accessibilityLiveRegion="polite">
        {outcome.action === 'adopt' ? m.mobile_cliproxy_adopt_done({ new: outcome.tag }) : m.mobile_cliproxy_update_done({ new: outcome.tag })}
      </Text></View>
      : null}
    {failed ? <View style={styles.body}><Text accessibilityRole="alert" style={[styles.note, { color: theme.tokens.status.error }]}>{outcome.text}</Text></View> : null}
    <SettingsRow title={m.mobile_cliproxy_card_origin()}
      right={<Text style={[styles.value, { color: c.muted }]}>{external ? m.mobile_cliproxy_card_external() : m.mobile_cliproxy_card_managed()}</Text>} />
    {showUpdate ? <SettingsRow title={m.mobile_cliproxy_card_new_version()}
      right={<Text style={[styles.value, { color: c.muted }]}>{neu}</Text>} /> : null}
    {install.accounts != null ? <SettingsRow title={m.mobile_cliproxy_card_accounts()}
      right={<Text style={[styles.value, { color: c.muted }]}>{String(install.accounts)}</Text>} /> : null}
    {handover?.ok ? <SettingsRow title={failed ? m.mobile_cliproxy_update_retry() : m.mobile_cliproxy_adopt_button()}
      description={interrupted('adopt') ? m.mobile_cliproxy_adopt_interrupted() : m.mobile_cliproxy_card_external_note()}
      onPress={posting ? undefined : () => void adopt(false)} /> : null}
    {handover && !handover.ok ? <View style={styles.body}><Text style={[styles.note, { color: theme.tokens.status.warning }]}>{handoverText(handover.code)}</Text></View> : null}
    {showUpdate && interrupted('update') ? <View style={styles.body}><Text style={[styles.note, { color: c.muted }]}>{m.mobile_cliproxy_update_interrupted()}</Text></View> : null}
    {failed && !external ? <SettingsRow title={m.mobile_cliproxy_update_retry()} onPress={posting ? undefined : () => void update(false)} /> : null}
    <SettingsRow title={m.mobile_cliproxy_card_connect()} onPress={posting ? undefined : onConnect} />
  </SectionCard>;
}

const styles = StyleSheet.create((theme) => ({
  body: { paddingHorizontal: theme.base.space[4], paddingBottom: theme.base.space[3] },
  note: { fontSize: theme.base.text.xs },
  value: { fontSize: theme.base.text.sm },
}));
