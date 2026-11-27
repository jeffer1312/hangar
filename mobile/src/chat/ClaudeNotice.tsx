import { memo, useEffect, useRef, useState, type ReactNode } from 'react';
import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { cancelSessionAutoContinue, recarregarSessao, type ChatEvent } from '@hangar/core';
import { haptic } from '../ui/haptics';
import { useSessions } from '../stores/sessions';
import { autoContinueCancellable, autoContinueText, isApiError, isAutoContinue, isFixNote, noticeAction, noticeCount, noticeDetail, noticeKey, noticeTitle, plainNotice, type FixState } from '../features/notices/notices';
import { fixPreview, putNoticeIgnore, reinstallPlugin, serverOf, startFix, type FixPreview, type FixRequest } from '../features/notices/api';
import { ApiErrorBox, IgnoredLine, NoticeBody, NoticeBusy, NoticeLineView, NoticeMark, NoticePill } from './NoticeParts';
import { NoticeFixSheet } from './NoticeFixSheet';
import * as m from '../paraglide/messages';

const NO_KEYS: string[] = [];

type Reinstall = 'idle' | 'running' | 'done' | 'reloading' | { failed: string };

const why = (error: unknown) => (error instanceof Error ? error.message : String(error));

/** Aviso da CLI do Claude: linha discreta entre fios que abre o detalhe e o Ignorar; erro da API em caixa âmbar. */
export const ClaudeNotice = memo(function ClaudeNotice({ event, serverId, sessionName, fix }: {
  event: ChatEvent; serverId?: string; sessionName?: string; fix?: FixState;
}) {
  const key = noticeKey(event);
  const ignoredKeys = useSessions((s) => (serverId ? s.noticeIgnores[serverId] : undefined) ?? NO_KEYS);
  const router = useRouter();
  const [open, setOpen] = useState(false);
  // O que a pessoa fez nesta linha vale enquanto ela está na tela: `true` ignorou (fica o Desfazer), `false` desfez.
  // A lista da máquina chega pelo evento e decide as outras linhas.
  const [local, setLocal] = useState<boolean | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [reinstall, setReinstall] = useState<Reinstall>('idle');
  // A folha só monta na primeira abertura e a prévia fica depois de fechar, para o conteúdo descer junto.
  const [preview, setPreview] = useState<FixPreview | null>(null);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [fixBusy, setFixBusy] = useState(false);
  const [fixError, setFixError] = useState<string | null>(null);
  // Aberta e ainda sem a nota "consertando": sem isto o botão voltaria e um segundo toque abriria outra sessão.
  // Guarda o estado de quando abriu: qualquer nota nova deste aviso troca o objeto e devolve a linha a ela.
  const [fixStarted, setFixStarted] = useState<{ from: FixState | undefined } | null>(null);
  const localRef = useRef(local);
  localRef.current = local;
  // Outro aparelho mudou a mesma chave depois: a lista volta a decidir. Só na chegada da lista, para o "ignorado"
  // recém-feito aqui não cair antes de o evento dele chegar.
  useEffect(() => {
    if (!key) return;
    const has = ignoredKeys.includes(key);
    if ((localRef.current === true && !has) || (localRef.current === false && has)) setLocal(null);
  }, [ignoredKeys, key]);

  if (isAutoContinue(event)) return <AutoContinueLine event={event} serverId={serverId} sessionName={sessionName} />;
  if (isApiError(event)) return <ApiErrorBox text={plainNotice(event).trim()} />;
  if (isFixNote(event)) {
    const child = event.notice_meta?.child ?? '';
    return (
      <NoticeLineView
        icon={<NoticeMark ok={event.notice_meta?.state === 'done'} />}
        title={plainNotice(event).trim()} count={null} open={null}
        action={child && serverId ? <NoticePill label={m.notice_fix_open()} onPress={() => router.push(`/s/${serverId}/${child}` as never)} /> : null}
      />
    );
  }
  if (key && local === null && ignoredKeys.includes(key)) return null;

  const change = (ignored: boolean) => {
    if (!key || !serverId) return;
    setFailed(null);
    putNoticeIgnore(serverId, key, ignored).then(() => {
      haptic.success();
      setLocal(ignored);
      setOpen(false);
    }, (error: unknown) => {
      haptic.error();
      setFailed(`${m.notice_ignore_failed()} · ${why(error)}`);
    });
  };

  if (local === true) {
    return (
      <View>
        <IgnoredLine onUndo={() => change(false)} />
        {failed ? <Text style={styles.failed}>{failed}</Text> : null}
      </View>
    );
  }

  const action = noticeAction(event);
  const doReinstall = (plugin: string) => {
    if (!serverId) return;
    setReinstall('running');
    reinstallPlugin(serverId, plugin).then(() => { haptic.success(); setReinstall('done'); },
      (error: unknown) => { haptic.error(); setReinstall({ failed: why(error) }); });
  };
  const reload = () => {
    if (!serverId || !sessionName) return;
    setReinstall('reloading');
    Promise.resolve().then(() => recarregarSessao(sessionName, serverOf(serverId)))
      .then(() => setReinstall('idle'), (error: unknown) => { haptic.error(); setReinstall({ failed: why(error) }); });
  };
  const request = (): FixRequest | null => {
    const subject = event.notice_meta?.subject;
    return subject ? { notice_id: event.id, subject, detail: event.hook_error ?? '', blocking: key == null } : null;
  };
  const openFix = () => {
    const body = request();
    if (!serverId || !sessionName || !body) return;
    setFixError(null);
    setFixBusy(true);
    fixPreview(serverId, sessionName, body).then((got) => { setPreview(got); setSheetOpen(true); },
      (error: unknown) => { haptic.error(); setFixError(why(error)); })
      .finally(() => setFixBusy(false));
  };
  const confirmFix = () => {
    const body = request();
    if (!serverId || !sessionName || !body) return;
    setFixBusy(true);
    startFix(serverId, sessionName, body).then(() => { haptic.success(); setFixStarted({ from: fix }); setSheetOpen(false); },
      (error: unknown) => { haptic.error(); setFixError(why(error)); }).finally(() => setFixBusy(false));
  };

  const starting = fixStarted !== null && fixStarted.from === fix;
  let title = noticeTitle(event);
  let icon: ReactNode;
  let actionPart: ReactNode = null;
  if (action?.kind === 'reinstall') {
    if (reinstall === 'running') actionPart = <NoticeBusy label={m.notice_reinstalling({ name: action.plugin })} />;
    else if (reinstall === 'reloading') actionPart = <NoticeBusy label={m.notice_reloading()} />;
    else if (reinstall === 'done') {
      icon = <NoticeMark ok />;
      title = m.notice_reinstalled({ name: action.plugin });
      actionPart = <NoticePill label={m.notice_reload()} onPress={reload} />;
    } else {
      actionPart = <NoticePill label={typeof reinstall === 'object' ? m.notice_retry() : m.notice_reinstall()} onPress={() => doReinstall(action.plugin)} />;
    }
  } else if (action?.kind === 'fix') {
    if (fix?.state === 'running') {
      actionPart = <NoticeBusy label={m.notice_fixing({ name: fix.child })} onPress={() => router.push(`/s/${serverId}/${fix.child}` as never)} />;
    } else if (fix?.state === 'done') {
      icon = <NoticeMark ok />;
      title = `${title} · ${m.notice_fixed()}`;
    } else if (starting || (fixBusy && !sheetOpen)) {
      actionPart = <NoticeBusy label={m.notice_fix()} />;
    } else {
      actionPart = <NoticePill label={m.notice_fix()} onPress={openFix} />;
    }
  }
  const lineError = typeof reinstall === 'object' ? m.notice_reinstall_failed({ why: reinstall.failed })
    : fix?.state === 'failed' && !starting ? m.notice_fix_failed() : fixError && !sheetOpen ? m.notice_fix_start_failed({ why: fixError }) : null;

  const detail = noticeDetail(event);
  const expandable = detail != null || key != null;
  return (
    <View>
      <NoticeLineView
        icon={icon}
        title={title}
        count={reinstall === 'done' ? null : noticeCount(event)}
        open={expandable ? open : null}
        onPress={() => { haptic.selection(); setOpen((v) => !v); }}
        action={actionPart}
      />
      {lineError ? <Text style={styles.failed}>{lineError}</Text> : null}
      {open ? <NoticeBody detail={detail} onIgnore={key ? () => change(true) : undefined} /> : null}
      {failed ? <Text style={styles.failed}>{failed}</Text> : null}
      {preview ? (
        <NoticeFixSheet open={sheetOpen} preview={preview} busy={fixBusy} error={sheetOpen ? fixError : null} onConfirm={confirmFix}
          onClose={() => setSheetOpen(false)} />
      ) : null}
    </View>
  );
});

type Cancel = 'idle' | 'cancelling' | { failed: string };

/** Nota do continuar sozinho: linha neutra entre fios, com o Cancelar enquanto espera a rede. */
function AutoContinueLine({ event, serverId, sessionName }: { event: ChatEvent; serverId?: string; sessionName?: string }) {
  const [cancel, setCancel] = useState<Cancel>('idle');
  // Cancelou: o giro fica até a nota trocada pelo desfecho chegar, e o botão sai com ela.
  const state = event.notice_meta?.state;
  useEffect(() => setCancel('idle'), [state]);
  const cancellable = autoContinueCancellable(event) && !!serverId && !!sessionName;
  const doCancel = () => {
    if (!serverId || !sessionName) return;
    setCancel('cancelling');
    Promise.resolve().then(() => cancelSessionAutoContinue(sessionName, serverOf(serverId))).then((cancelled) => {
      // Nada esperando (o desfecho já saiu): a nota chega trocada sozinha.
      if (!cancelled) setCancel('idle');
    }, (error: unknown) => { haptic.error(); setCancel({ failed: why(error) }); });
  };
  const action = !cancellable ? null : cancel === 'cancelling'
    ? <NoticeBusy label={m.auto_continue_cancel()} />
    : <NoticePill label={m.auto_continue_cancel()} onPress={doCancel} />;
  return (
    <View>
      {/* Nota neutra: sem o triângulo de aviso. */}
      <NoticeLineView icon={false} title={autoContinueText(event)} count={null} open={null} action={action} />
      {typeof cancel === 'object' ? <Text style={styles.failed}>{m.auto_continue_cancel_failed({ why: cancel.failed })}</Text> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  failed: { ...theme.type.caption, color: theme.tokens.status.warning, textAlign: 'center' },
}));
