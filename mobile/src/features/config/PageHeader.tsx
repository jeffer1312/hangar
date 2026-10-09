import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Icon, type IconName } from '../../ui/Icon';
import { useSettingsColors } from './colors';

export interface PageAction {
  label: string;
  icon?: IconName;
  onPress: () => void;
  disabled?: boolean;
}

/** Topo das páginas (page_top do Rust): título, a linha que explica e as pílulas de ação embaixo. */
export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: PageAction[] }) {
  const c = useSettingsColors();
  return (
    <View style={styles.top}>
      <View>
        <Text style={[styles.title, { color: c.text }]} accessibilityRole="header">{title}</Text>
        {subtitle ? <Text style={[styles.subtitle, { color: c.muted }]}>{subtitle}</Text> : null}
      </View>
      {actions?.length ? (
        <View style={styles.pills}>
          {actions.map((a) => <Pill key={a.label} {...a} />)}
        </View>
      ) : null}
    </View>
  );
}

export function Pill({ label, icon, onPress, disabled }: PageAction) {
  const c = useSettingsColors();
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled: !!disabled }}
      hitSlop={6}
      style={({ pressed }) => [styles.pill, { borderColor: c.borderStrong }, pressed && { backgroundColor: c.hover },
        disabled && { opacity: 0.45 }]}
    >
      {icon ? <Icon name={icon} size={14} color={c.text} /> : null}
      <Text style={[styles.pillText, { color: c.text }]} numberOfLines={1}>{label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  top: { gap: 14, paddingHorizontal: 4 },
  title: { fontSize: 20, fontWeight: '600' },
  subtitle: { marginTop: 6, fontSize: 15, lineHeight: 20 },
  pills: { flexDirection: 'row', flexWrap: 'wrap', gap: 8 },
  pill: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    height: 32,
    paddingHorizontal: 12,
    borderWidth: 1,
    borderRadius: 9999,
  },
  pillText: { fontSize: 14, fontWeight: '500' },
});
