import type { ReactNode } from 'react';
import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Icon, type IconName } from '../ui/Icon';
import * as m from '../paraglide/messages';

export type SessionSettingsTab = 'model' | 'session' | 'usage';

export function SessionSettingsFrame({ height, title, subtitle, tab, onTab, onBack, onClose, footer, children }: {
  height: number;
  title: string;
  subtitle?: string;
  tab?: SessionSettingsTab;
  onTab: (tab: SessionSettingsTab) => void;
  onBack?: () => void;
  onClose: () => void;
  footer?: ReactNode;
  children: ReactNode;
}) {
  return <View testID="session-settings-sheet" style={[styles.frame, { height }]}>
    <View style={styles.header}>
      {onBack ? <Pressable accessibilityRole="button" accessibilityLabel={m.comum_voltar()} onPress={onBack} style={({ pressed }) => [styles.headerAction, pressed && styles.pressed]}>
        <Icon name="ChevronLeft" size={20} />
      </Pressable> : null}
      <View style={styles.grow}>
        <Text style={styles.title} accessibilityRole="header">{title}</Text>
        {subtitle ? <Text style={styles.subtitle}>{subtitle}</Text> : null}
      </View>
      <Pressable accessibilityRole="button" accessibilityLabel={m.native_close()} onPress={onClose} style={({ pressed }) => [styles.headerAction, pressed && styles.pressed]}>
        <Icon name="X" size={20} />
      </Pressable>
    </View>
    {tab ? <View style={styles.tabs}>
      {([
        ['model', m.composer_modelo()], ['session', m.native_usage_card_session()], ['usage', m.live_voice_action_usage_label()],
      ] as const).map(([key, label]) => <Pressable key={key} onPress={() => onTab(key)} accessibilityRole="tab"
        accessibilityLabel={label} accessibilityState={{ selected: tab === key }} style={[styles.tab, tab === key && styles.tabSelected]}>
        <Text style={[styles.tabText, tab === key && styles.tabTextSelected]}>{label}</Text>
      </Pressable>)}
    </View> : null}
    <View style={styles.body}>{children}</View>
    <View style={styles.footer}>
      {footer}
      <Pressable onPress={onClose} accessibilityRole="button" accessibilityLabel={m.codex_ui_concluir()} style={({ pressed }) => [styles.done, pressed && styles.pressed]}>
        <Text style={styles.doneText}>{m.codex_ui_concluir()}</Text>
      </Pressable>
    </View>
  </View>;
}

export function SessionSettingsContent({ title, children }: { title?: string; children: ReactNode }) {
  return <ScrollView contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
    {title ? <Text style={styles.sectionTitle} accessibilityRole="header">{title}</Text> : null}
    {children}
  </ScrollView>;
}

export function SessionSettingsLink({ label, value, disabled, icon, onPress }: {
  label: string; value?: string; disabled?: boolean; icon?: IconName; onPress: () => void;
}) {
  return <Pressable onPress={onPress} disabled={disabled} accessibilityRole="button" accessibilityLabel={label}
    accessibilityValue={value ? { text: value } : undefined} accessibilityState={{ disabled: !!disabled }}
    style={({ pressed }) => [styles.row, pressed && styles.pressed]}>
    {icon ? <Icon name={icon} size={18} /> : null}
    <Text style={[styles.rowLabel, !value && styles.secondary]}>{label}</Text>
    {value ? <Text style={styles.value}>{value}</Text> : null}
    <Icon name="ChevronRight" size={16} />
  </Pressable>;
}

const styles = StyleSheet.create(theme => ({
  frame: { minHeight: 0, flexShrink: 1 },
  header: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[4], paddingTop: theme.base.space[1.5], paddingBottom: theme.base.space[4] },
  grow: { flex: 1, minWidth: 0 },
  title: { fontSize: theme.base.text.xl, lineHeight: 28, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  subtitle: { marginTop: theme.base.space[0.5], fontSize: theme.base.text.xxs, lineHeight: 16, color: theme.tokens.text.secondary },
  headerAction: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center' },
  tabs: { flexDirection: 'row', gap: theme.base.space[5], paddingHorizontal: theme.base.space[4], borderBottomWidth: 1, borderColor: theme.tokens.border.default },
  tab: { minHeight: 44, justifyContent: 'center', borderBottomWidth: 2, borderBottomColor: 'transparent' },
  tabSelected: { borderBottomColor: theme.tokens.accent.base },
  tabText: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  tabTextSelected: { color: theme.tokens.accent.base, fontWeight: theme.base.weight.medium },
  body: { flex: 1, minHeight: 0 },
  content: { padding: theme.base.space[4], gap: theme.base.space[1] },
  sectionTitle: { marginTop: theme.base.space[1], marginBottom: theme.base.space[2], fontSize: theme.base.text.xxs, lineHeight: 18, color: theme.tokens.text.secondary },
  footer: { padding: theme.base.space[4], borderTopWidth: 1, borderColor: theme.tokens.border.default },
  done: { minHeight: 48, alignItems: 'center', justifyContent: 'center', borderRadius: theme.base.radius.md, backgroundColor: theme.tokens.accent.base },
  doneText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.ink.onAccent },
  row: { minHeight: 56, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], justifyContent: 'space-between' },
  rowLabel: { flex: 1, fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
  secondary: { fontSize: theme.base.text.xs, color: theme.tokens.text.secondary },
  value: { maxWidth: '55%', fontSize: theme.base.text.xs, color: theme.tokens.text.secondary },
  pressed: { opacity: 0.65 },
}));
