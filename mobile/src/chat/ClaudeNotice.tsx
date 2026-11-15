import { memo, useState } from 'react';
import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import type { ChatEvent } from '@hangar/core';
import { haptic } from '../ui/haptics';
import { setNoticeIgnores, useSessions } from '../stores/sessions';
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
  // Ignorado aqui: a linha fica com Desfazer enquanto estiver montada; nas outras vezes some.
  const [ignoredHere, setIgnoredHere] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);

  if (isApiError(event)) return <ApiErrorBox text={plainNotice(event).trim()} />;
  if (key && ignoredKeys.includes(key) && !ignoredHere) return null;

  const change = (ignored: boolean) => {
    if (!key || !serverId) return;
    setFailed(null);
    putNoticeIgnore(serverId, key, ignored).then((keys) => {
      haptic.success();
      // Mesmo instante: a lista nova e o "ignorado aqui" mudam juntos, sem a linha sumir no meio.
      setIgnoredHere(ignored);
      setOpen(false);
      if (keys) setNoticeIgnores(serverId, keys);
    }, (error: unknown) => {
      haptic.error();
      setFailed(`${m.notice_ignore_failed()} · ${error instanceof Error ? error.message : String(error)}`);
    });
  };

  if (ignoredHere && key && ignoredKeys.includes(key)) {
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
