import { memo, useEffect, useRef, useState } from 'react';
import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import type { ChatEvent } from '@hangar/core';
import { haptic } from '../ui/haptics';
import { useSessions } from '../stores/sessions';
import { isApiError, noticeCount, noticeDetail, noticeKey, noticeTitle, plainNotice } from '../features/notices/notices';
import { putNoticeIgnore } from '../features/notices/api';
import { ApiErrorBox, IgnoredLine, NoticeBody, NoticeLineView } from './NoticeParts';
import * as m from '../paraglide/messages';

const NO_KEYS: string[] = [];

/** Aviso da CLI do Claude: linha discreta entre fios que abre o detalhe e o Ignorar; erro da API em caixa âmbar. */
export const ClaudeNotice = memo(function ClaudeNotice({ event, serverId }: { event: ChatEvent; serverId?: string }) {
  const key = noticeKey(event);
  const ignoredKeys = useSessions((s) => (serverId ? s.noticeIgnores[serverId] : undefined) ?? NO_KEYS);
  const [open, setOpen] = useState(false);
  // O que a pessoa fez nesta linha vale enquanto ela está na tela: `true` ignorou (fica o Desfazer), `false` desfez.
  // A lista da máquina chega pelo evento e decide as outras linhas.
  const [local, setLocal] = useState<boolean | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const localRef = useRef(local);
  localRef.current = local;
  // Outro aparelho mudou a mesma chave depois: a lista volta a decidir. Só na chegada da lista, para o "ignorado"
  // recém-feito aqui não cair antes de o evento dele chegar.
  useEffect(() => {
    if (!key) return;
    const has = ignoredKeys.includes(key);
    if ((localRef.current === true && !has) || (localRef.current === false && has)) setLocal(null);
  }, [ignoredKeys, key]);

  if (isApiError(event)) return <ApiErrorBox text={plainNotice(event).trim()} />;
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
      setFailed(`${m.notice_ignore_failed()} · ${error instanceof Error ? error.message : String(error)}`);
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

  const detail = noticeDetail(event);
  const expandable = detail != null || key != null;
  return (
    <View>
      <NoticeLineView
        title={noticeTitle(event)}
        count={noticeCount(event)}
        open={expandable ? open : null}
        onPress={() => { haptic.selection(); setOpen((v) => !v); }}
      />
      {open ? <NoticeBody detail={detail} onIgnore={key ? () => change(true) : undefined} /> : null}
      {failed ? <Text style={styles.failed}>{failed}</Text> : null}
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  failed: { ...theme.type.caption, color: theme.tokens.status.warning, textAlign: 'center' },
}));
