import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { useSettingsColors } from './colors';

/** Linha de marcar várias, dentro de um `SectionCard`: o quadrado diz o estado e o toque inteiro troca. */
export function CheckRow({ title, description, checked, onToggle, disabled, dot }: {
  title: string;
  description?: string;
  checked: boolean;
  onToggle: () => void;
  disabled?: boolean;
  /** Ponto de ligado/desligado à esquerda; ausente quando o estado não é conhecido. */
  dot?: 'on' | 'off';
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <Pressable accessibilityRole="checkbox" accessibilityLabel={title} accessibilityHint={description}
    accessibilityState={{ checked, disabled }} disabled={disabled}
    onPress={() => { haptic.selection(); onToggle(); }}
    style={({ pressed }) => [styles.row, { borderTopColor: c.border, opacity: disabled ? 0.5 : 1 }, pressed && { backgroundColor: c.hover }]}>
    {dot ? <View style={[styles.dot, { backgroundColor: dot === 'on' ? theme.tokens.status.success : c.faint }]} /> : null}
    <View style={styles.texts}>
      <Text style={[styles.title, { color: c.text }]}>{title}</Text>
      {description ? <Text style={[styles.description, { color: c.muted }]}>{description}</Text> : null}
    </View>
    <View style={[styles.box, checked ? { backgroundColor: c.accent, borderColor: c.accent } : { borderColor: c.borderStrong }]}>
      {checked ? <Icon name="Check" size={14} color={theme.ink.onAccent} /> : null}
    </View>
  </Pressable>;
}

const styles = StyleSheet.create((theme) => ({
  row: { minHeight: 56, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[4],
    paddingVertical: theme.base.space[3], borderTopWidth: 1 },
  dot: { width: 9, height: 9, borderRadius: theme.base.radius.full },
  texts: { flex: 1, minWidth: 0, gap: theme.base.space[1] },
  title: { fontSize: theme.base.text.base },
  description: { fontSize: theme.base.text.xs, lineHeight: 19 },
  box: { width: 24, height: 24, borderRadius: theme.base.radius.xs, borderWidth: 1, alignItems: 'center', justifyContent: 'center' },
}));
