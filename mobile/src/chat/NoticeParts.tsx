// Peças de desenho do aviso da CLI do Claude, sem store: a conversa e a animação da novidade usam as mesmas.
import type { ReactNode } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from '../ui/Icon';
import * as m from '../paraglide/messages';

/**
 * A linha fechada; `open` é `null` quando não há o que abrir. Com `action` (o botão ou o andamento), o título corta
 * em uma linha para os dois caberem.
 */
export function NoticeLineView({ title, count, open, onPress, action, icon }: {
  title: string; count: string | null; open: boolean | null; onPress?: () => void; action?: ReactNode; icon?: ReactNode;
}) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.line}>
      <View style={styles.rule} />
      <Pressable
        style={styles.head}
        disabled={open == null || !onPress}
        onPress={onPress}
        hitSlop={8}
        accessibilityRole={open == null ? 'text' : 'button'}
        accessibilityState={open == null ? undefined : { expanded: open }}
      >
        {icon ?? <Icon name="TriangleAlert" size={12} color={theme.tokens.text.muted} />}
        <Text style={styles.title} numberOfLines={action ? 1 : undefined}>{title}{count ? ` · ${count}` : ''}</Text>
      </Pressable>
      {action}
      {open == null ? null : (
        <Pressable onPress={onPress} hitSlop={14} accessibilityElementsHidden importantForAccessibility="no">
          <Icon name={open ? 'ChevronDown' : 'ChevronRight'} size={12} color={theme.tokens.text.muted} />
        </Pressable>
      )}
      <View style={styles.rule} />
    </View>
  );
}

/** O botão da ação na linha: 30 pt de altura, toque até 44. */
export function NoticePill({ label, onPress }: { label: string; onPress?: () => void }) {
  return (
    <Pressable style={styles.pill} onPress={onPress} disabled={!onPress} hitSlop={7} accessibilityRole="button">
      <Text style={styles.pillText} numberOfLines={1}>{label}</Text>
    </Pressable>
  );
}

/** Andamento na linha (reinstalando, consertando): o indicador e o texto. */
export function NoticeBusy({ label, onPress }: { label: string; onPress?: () => void }) {
  const { theme } = useUnistyles();
  return (
    <Pressable style={styles.busy} onPress={onPress} disabled={!onPress} hitSlop={10} accessibilityRole={onPress ? 'link' : 'text'}>
      <ActivityIndicator size="small" color={theme.tokens.accent.base} />
      <Text style={onPress ? styles.link : styles.title} numberOfLines={1}>{label}</Text>
    </Pressable>
  );
}

/** Ícone de feito (verde) ou de falha (âmbar) para a linha. */
export function NoticeMark({ ok }: { ok: boolean }) {
  const { theme } = useUnistyles();
  return ok ? <Icon name="Check" size={12} color={theme.tokens.status.success} /> : <Icon name="TriangleAlert" size={12} color={theme.tokens.status.warning} />;
}

/** O que abre embaixo da linha: o detalhe em fonte fixa e o Ignorar. */
export function NoticeBody({ detail, onIgnore }: { detail: string | null; onIgnore?: () => void }) {
  return (
    <View style={styles.body}>
      {detail ? <Text style={styles.detail} selectable>{detail}</Text> : null}
      {onIgnore ? (
        <Pressable style={styles.ignore} onPress={onIgnore} accessibilityRole="button">
          <Text style={styles.ignoreText}>{m.notice_ignore()}</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

export function IgnoredLine({ onUndo }: { onUndo?: () => void }) {
  return (
    <View style={styles.line}>
      <View style={styles.rule} />
      <Text style={styles.title}>{m.notice_ignored_line()}</Text>
      <Pressable onPress={onUndo} disabled={!onUndo} hitSlop={14} accessibilityRole="button">
        <Text style={styles.link}>{m.comum_desfazer()}</Text>
      </Pressable>
      <View style={styles.rule} />
    </View>
  );
}

/** Erro da API que parou o turno: caixa âmbar, sem Ignorar. */
export function ApiErrorBox({ text }: { text: string }) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.errorBox}>
      <View style={styles.errorHead}>
        <Icon name="TriangleAlert" size={14} color={theme.tokens.status.warning} />
        <Text style={styles.errorTitle}>{m.notice_api_error_title()}</Text>
      </View>
      <Text style={styles.errorText} selectable>{text}</Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  line: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1.5],
    minHeight: 44,
  },
  rule: { flex: 1, minWidth: theme.base.space[3], height: StyleSheet.hairlineWidth, backgroundColor: theme.tokens.border.default },
  head: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], flexShrink: 1, minHeight: 44 },
  pill: {
    height: 30,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.full,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: theme.tokens.border.default,
    flexShrink: 0,
  },
  pillText: { ...theme.type.meta, color: theme.tokens.text.primary },
  busy: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], flexShrink: 1, minHeight: 44 },
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
