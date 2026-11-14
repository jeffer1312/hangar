import type { ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon, type IconName } from '../../ui/Icon';
import { useSettingsColors } from './colors';

/** Título de grupo da coluna das Configurações do Rust ("Este aparelho", "Servidor"). */
export function SettingsMenuGroup({ title, right }: { title: string; right?: ReactNode }) {
  const c = useSettingsColors();
  return (
    <View style={styles.group}>
      <Text style={[styles.groupText, { color: c.faint }]} accessibilityRole="header">{title}</Text>
      {right}
    </View>
  );
}

/** Item da navegação: ícone discreto e nome, sem caixa; o toque é o realce. */
export function SettingsMenuItem({ icon, label, badge, onPress }: { icon?: IconName; label: string; badge?: string; onPress: () => void }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={badge ? `${label}, ${badge}` : label}
      style={({ pressed }) => [styles.item, pressed && { backgroundColor: c.hover }]}
    >
      {icon ? <Icon name={icon} size={18} color={c.faint} /> : null}
      <Text style={[styles.itemText, { color: c.muted }]}>{label}</Text>
      {badge ? <Text style={[styles.badge, { color: theme.tokens.text.inverse, backgroundColor: c.accentText }]}>{badge}</Text> : null}
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  group: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[2], paddingTop: theme.base.space[3], paddingBottom: theme.base.space[1.5] },
  groupText: { flex: 1, fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
  item: { minHeight: 48, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[1.5], borderRadius: theme.base.radius.sm },
  itemText: { flex: 1, fontSize: theme.base.text.sm },
  badge: { ...theme.type.badge, paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[0.5], borderRadius: theme.base.radius.full, overflow: 'hidden' },
}));
