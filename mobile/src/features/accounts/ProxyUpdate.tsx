import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { getCliproxyInstallForServer, updateCliproxyForServer, type CliproxyInstall, type CliproxyJob, type Server } from '@hangar/core';
import { SectionCard } from '../config/SectionCard';
import { Pill } from '../config/PageHeader';
import { useSettingsColors } from '../config/colors';
import { InstallItems } from './InstallItems';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

// Quanto o "atualizado" fica na tela antes de sumir.
const DONE_FOR = 4000;
const STEP_AT: Record<string, number> = { download: 0, service: 1, start: 2 };

export type Outcome = { k: 'done'; tag: string } | { k: 'failed'; text: string } | { k: 'interrupted' };

const updateJob = (install: CliproxyInstall | null): CliproxyJob | null =>
  install?.job?.kind === 'update' ? install.job : null;

/** O texto da falha pelo código da tarefa: só a volta que deu certo diz que o proxy segue funcionando. */
export function failureText(error: { code: string; detail: string }, install: CliproxyInstall | null): string {
  const neu = install?.latest ?? '';
  const old = install?.version ?? '';
  if (error.code === 'cliproxy_nao_subiu') return m.mobile_cliproxy_update_failed_back({ new: neu, old });
  if (error.code === 'cliproxy_atualizacao_sem_volta') return m.mobile_cliproxy_update_failed_lost({ old });
  return error.detail || error.code;
}

/** A versão que a linha do modelo mostra: só a do proxy que o Hangar instalou. */
export const managedVersion = (install: CliproxyInstall | null) => install?.state === 'managed' ? install.version : null;

/**
 * Estado da instalação lido ao abrir e recarregar a página; com a atualização em curso, relido a cada segundo.
 * Falha da leitura não vira aviso: só a atualização que falha aparece.
 */
export function useProxyUpdate(server: Server | null) {
  const [install, setInstall] = useState<CliproxyInstall | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [posting, setPosting] = useState(false);
  const postingNow = useRef(false);
  const alive = useRef(true);
  const asked = useRef(false);
  const polling = useRef(false);
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
      console.warn('[contas] estado do CLIProxyAPI não veio', cause);
      return null;
    }
  }, [server?.id, server?.baseUrl, server?.token]);

  const finish = useCallback((next: CliproxyInstall) => {
    const job = updateJob(next);
    if (!job?.done || !asked.current) return;
    asked.current = false;
    if (!job.error) {
      haptic.success();
      setOutcome({ k: 'done', tag: next.version ?? '' });
      clearTimeout(doneTimer.current);
      doneTimer.current = setTimeout(() => { if (alive.current) setOutcome(o => o?.k === 'done' ? null : o); }, DONE_FOR);
    } else if (job.error.code === 'erro_cliproxy_sessoes_ativas') setOutcome({ k: 'interrupted' });
    else { haptic.error(); setOutcome({ k: 'failed', text: failureText(job.error, next) }); }
  }, []);

  const poll = useCallback(async () => {
    if (polling.current) return;
    polling.current = true;
    try {
      while (alive.current) {
        await new Promise(resolve => setTimeout(resolve, 1000));
        if (!alive.current) return;
        const next = await read();
        if (!next) return;
        if (!updateJob(next) || updateJob(next)!.done) { finish(next); return; }
      }
    } finally { polling.current = false; }
  }, [read, finish]);

  const reload = useCallback(async () => {
    if (polling.current) return;
    const next = await read();
    if (next && updateJob(next) && !updateJob(next)!.done) void poll();
  }, [read, poll]);

  const update = useCallback(async (confirm = false) => {
    // Trava síncrona: dois toques antes do primeiro voltar ainda veem o `posting` antigo.
    if (!server || postingNow.current) return;
    postingNow.current = true;
    setPosting(true);
    setOutcome(null);
    try {
      await updateCliproxyForServer(server, confirm);
      if (!alive.current) return;
      asked.current = true;
      void poll();
    } catch (cause) {
      if (!alive.current) return;
      const error = cause as { code?: string; envelope?: { params?: { sessions?: unknown } }; message?: string };
      if (error.code === 'erro_cliproxy_sessoes_ativas') {
        const sessions = Number(error.envelope?.params?.sessions) || 0;
        const body = sessions === 1 ? m.mobile_cliproxy_update_confirm_one() : m.mobile_cliproxy_update_confirm_many({ n: String(sessions) });
        Alert.alert(m.mobile_cliproxy_update_confirm_title(), body, [
          { text: m.comum_cancelar(), style: 'cancel' },
          { text: m.mobile_cliproxy_update_confirm_ok(), style: 'destructive', onPress: () => void update(true) },
        ]);
      } else if (error.code === 'erro_cliproxy_ocupado') {
        // Outra instalação ou atualização em curso: acompanha a que está rodando.
        asked.current = true;
        void poll();
      } else {
        haptic.error();
        setOutcome({ k: 'failed', text: error.message || m.erro_desconhecido() });
      }
    } finally {
      postingNow.current = false;
      if (alive.current) setPosting(false);
    }
  }, [server?.id, server?.baseUrl, server?.token, poll]);

  useEffect(() => {
    alive.current = true;
    void reload();
    return () => { alive.current = false; seq.current++; clearTimeout(doneTimer.current); };
  }, [reload]);

  return { install, outcome, posting, reload, update };
}

export type ProxyUpdateState = ReturnType<typeof useProxyUpdate>;

/** O cartão de Contas: versão nova com o Atualizar, andamento, concluído ou a falha com Tentar de novo. */
export function ProxyUpdateCard({ state }: { state: Pick<ProxyUpdateState, 'install' | 'outcome' | 'posting' | 'update'> }) {
  const c = useSettingsColors();
  const { install, outcome, posting, update } = state;
  const job = updateJob(install);
  const neu = install?.latest ?? '';
  const old = install?.version ?? '';
  if (job && !job.done) {
    const labels = [m.mobile_cliproxy_update_step_download(), m.mobile_cliproxy_update_step_service(), m.mobile_cliproxy_update_step_start()];
    return <SectionCard icon="ArrowUp" busy title={m.mobile_cliproxy_update_running({ new: neu })} subtitle={m.mobile_cliproxy_update_restarts()}>
      <View style={styles.body}><InstallItems at={STEP_AT[job.step] ?? 0} failed={false} labels={labels} /></View>
    </SectionCard>;
  }
  if (outcome?.k === 'done') return <SectionCard icon="CircleCheck" title={m.mobile_cliproxy_update_done({ new: outcome.tag })} />;
  if (outcome?.k === 'failed') {
    return <SectionCard icon="CircleAlert" tone="danger" title={m.mobile_cliproxy_update_failed_title()} subtitle={outcome.text}>
      <View style={[styles.body, styles.row]}>
        <Pill label={m.mobile_cliproxy_update_retry()} onPress={() => void update(false)} disabled={posting} />
      </View>
    </SectionCard>;
  }
  if (install?.state !== 'managed' || !install.update_available) return null;
  return <SectionCard icon="ArrowUp" title={m.mobile_cliproxy_update_available_title({ new: neu })}
    subtitle={m.mobile_cliproxy_update_available_sub({ old })}
    extra={<Pill label={m.mobile_cliproxy_update_button()} onPress={() => void update(false)} disabled={posting} />}>
    {outcome?.k === 'interrupted'
      ? <View style={styles.body}><Text style={[styles.note, { color: c.muted }]}>{m.mobile_cliproxy_update_interrupted()}</Text></View>
      : null}
  </SectionCard>;
}

const styles = StyleSheet.create((theme) => ({
  body: { paddingHorizontal: theme.base.space[4], paddingBottom: theme.base.space[3] },
  row: { flexDirection: 'row' },
  note: { fontSize: theme.base.text.xs },
}));
