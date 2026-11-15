import { memo, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { ChatEvent } from '@hangar/core';
import { Icon } from '../ui/Icon';
import { haptic } from '../ui/haptics';
import { useSessions } from '../stores/sessions';
import { isApiError, noticeCount, noticeDetail, noticeKey, noticeTitle, plainNotice } from '../features/notices/notices';
import { putNoticeIgnore } from '../features/notices/api';
import * as m from '../paraglide/messages';

const NO_KEYS: string[] = [];

/** Aviso da CLI do Claude: linha discreta entre fios que abre o detalhe e o Ignorar; erro da API em caixa âmbar. */
export const ClaudeNotice = memo(function ClaudeNotice({ event, serverId }: { event: ChatEvent; serverId?: string }) {
  const { theme } = useUnistyles();
  const key = noticeKey(event);
  const ignoredKeys = useSessions((s) => (serverId ? s.noticeIgnores[serverId] : undefined) ?? NO_KEYS);
  const [open, setOpen] = useState(false);
  // Ignorado aqui: a linha fica com Desfazer enquanto estiver montada; nas outras vezes some.
  const [ignoredHere, setIgnoredHere] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);

  if (isApiError(event)) {
    return (
      <View style={styles.errorBox}>
        <View style={styles.errorHead}>
          <Icon name="TriangleAlert" size={14} color={theme.tokens.status.warning} />
          <Text style={styles.errorTitle}>{m.notice_api_error_title()}</Text>
        </View>
        <Text style={styles.errorText} selectable>{plainNotice(event).trim()}</Text>
      </View>
    );
  }
  if (key && ignoredKeys.includes(key) && !ignoredHere) return null;

  const change = (ignored: boolean) => {
    if (!key || !serverId) return;
    setFailed(null);
    putNoticeIgnore(serverId, key, ignored).then(() => {
      haptic.success();
      setIgnoredHere(ignored);
      setOpen(false);
    }, (error: unknown) => {
      haptic.error();
      setFailed(`${m.notice_ignore_failed()} · ${error instanceof Error ? error.message : String(error)}`);
    });
  };

  if (ignoredHere && key && ignoredKeys.includes(key)) {
    return (
      <View style={styles.line}>
        <View style={styles.rule} />
        <Text style={styles.title}>{m.notice_ignored_line()}</Text>
        <Pressable onPress={() => change(false)} hitSlop={14} accessibilityRole="button">
          <Text style={styles.link}>{m.comum_desfazer()}</Text>
        </Pressable>
        <View style={styles.rule} />
      </View>
    );
  }

  const detail = noticeDetail(event);
  const count = noticeCount(event);
  const expandable = detail != null || key != null;
  return (
    <View>
      <Pressable
        style={styles.line}
        disabled={!expandable}
        onPress={() => { haptic.selection(); setOpen((v) => !v); }}
        hitSlop={8}
        accessibilityRole={expandable ? 'button' : 'text'}
        accessibilityState={expandable ? { expanded: open } : undefined}
      >
        <View style={styles.rule} />
        <Icon name="TriangleAlert" size={12} color={theme.tokens.text.muted} />
        <Text style={styles.title}>{noticeTitle(event)}{count ? ` · ${count}` : ''}</Text>
        {expandable ? <Icon name={open ? 'ChevronDown' : 'ChevronRight'} size={12} color={theme.tokens.text.muted} /> : null}
        <View style={styles.rule} />
      </Pressable>
      {open ? (
        <View style={styles.body}>
          {detail ? <Text style={styles.detail} selectable>{detail}</Text> : null}
          {key ? (
            <Pressable style={styles.ignore} onPress={() => change(true)} accessibilityRole="button">
              <Text style={styles.ignoreText}>{m.notice_ignore()}</Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
      {failed ? <Text style={styles.failed}>{failed}</Text> : null}
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  line: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1.5],
    minHeight: 44,
  },
  rule: { flex: 1, minWidth: theme.base.space[3], height: StyleSheet.hairlineWidth, backgroundColor: theme.tokens.border.default },
  title: { ...theme.type.meta, color: theme.tokens.text.muted, textAlign: 'center', flexShrink: 1 },
  link: { ...theme.type.meta, color: theme.tokens.accent.base },
  body: { alignItems: 'center', gap: theme.base.space[2], paddingBottom: theme.base.space[2] },
  detail: {
    ...theme.type.mono,
    alignSelf: 'stretch',
    color: theme.tokens.text.secondary,
    backgroundColor: theme.tokens.bg.hover,
    borderRadius: theme.base.radius.sm,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
  },
  ignore: {
    minHeight: 44,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[4],
    borderRadius: theme.base.radius.full,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: theme.tokens.border.default,
  },
  ignoreText: { ...theme.type.label, color: theme.tokens.text.primary },
  failed: { ...theme.type.caption, color: theme.tokens.status.warning, textAlign: 'center' },
  errorBox: {
    gap: theme.base.space[1],
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    borderRadius: theme.base.radius.md,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: theme.tokens.status.warning,
  },
  errorHead: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5] },
  errorTitle: { ...theme.type.label, color: theme.tokens.status.warning },
  errorText: { ...theme.type.meta, color: theme.tokens.text.secondary },
}));
