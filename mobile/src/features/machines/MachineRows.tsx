import type { ReactNode } from 'react';
import { Pressable, Text, TextInput, View, type TextInputProps } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from '../../ui/Icon';
import { SectionCard } from '../config/SectionCard';
import { useSettingsColors } from '../config/colors';

export type MachineView = 'edit' | 'connection' | 'discover' | 'remote' | 'advanced' | 'identity' | 'terminal' | 'diagnostics' | 'peers' | 'maintenance';

export const MachineGroup = ({ children }: { children: ReactNode }) => <SectionCard>{children}</SectionCard>;

export function MachineRow({ title, value, description, onPress, disabled, danger, action, selected, expanded, valueTone }: {
  title: string;
  value?: string;
  description?: string;
  onPress?: () => void;
  disabled?: boolean;
  danger?: boolean;
  action?: boolean;
  selected?: boolean;
  expanded?: boolean;
  valueTone?: 'success' | 'error';
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const content = <>
    <View style={rowStyles.texts}>
      <Text style={[rowStyles.label, { color: danger ? theme.tokens.status.error : action ? c.accent : c.text }]}>{title}</Text>
      {description ? <Text selectable={!onPress} style={[rowStyles.description, { color: c.muted }]}>{description}</Text> : null}
    </View>
    {value ? <Text style={[rowStyles.value, { color: valueTone ? theme.tokens.status[valueTone] : c.muted }]}>{value}</Text> : null}
    {selected ? <Icon name="Check" size={18} color={c.accent} />
      : onPress && !action ? <Icon name="ChevronRight" size={17} color={c.faint} /> : null}
  </>;
  const style = [rowStyles.row, { borderTopColor: c.border, opacity: disabled ? 0.5 : 1 }];
  return onPress ? <Pressable accessibilityRole="button" accessibilityLabel={title}
    accessibilityValue={value ? { text: value } : undefined} accessibilityHint={description}
    accessibilityState={{ disabled, selected, expanded }} disabled={disabled} onPress={onPress}
    style={({ pressed }) => [...style, pressed && { backgroundColor: c.hover }]}>{content}</Pressable>
    : <View style={style}>{content}</View>;
}

export function MachineField({ label, stacked = false, ...props }: TextInputProps & { label: string; stacked?: boolean }) {
  const c = useSettingsColors();
  return <View style={[rowStyles.field, stacked && rowStyles.stacked, { borderTopColor: c.border }]}>
    <Text style={[stacked ? rowStyles.description : rowStyles.label, { color: stacked ? c.muted : c.text }]}>{label}</Text>
    <TextInput autoCapitalize="none" autoCorrect={false} {...props} accessibilityLabel={label}
      placeholderTextColor={c.faint} style={[rowStyles.input, stacked && rowStyles.stackedInput, { color: c.text }]} />
  </View>;
}

export function MachineHelp({ children }: { children: ReactNode }) {
  const c = useSettingsColors();
  return <Text style={[rowStyles.help, { color: c.muted }]}>{children}</Text>;
}

const rowStyles = StyleSheet.create((theme) => ({
  row: { minHeight: 56, flexDirection: 'row', alignItems: 'center', paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], gap: theme.base.space[3], borderTopWidth: 1 },
  texts: { flex: 1, minWidth: 0, gap: theme.base.space[1] },
  label: { fontSize: theme.base.text.base },
  description: { fontSize: theme.base.text.xs, lineHeight: 19 },
  value: { fontSize: theme.base.text.sm, maxWidth: '50%', flexShrink: 1, textAlign: 'right' },
  field: { minHeight: 56, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[2], borderTopWidth: 1 },
  stacked: { alignItems: 'stretch', flexDirection: 'column', gap: theme.base.space[0.5] },
  input: { flex: 1, minWidth: 0, minHeight: 44, fontSize: theme.base.text.base, textAlign: 'right', paddingVertical: theme.base.space[2] },
  stackedInput: { flex: 0, textAlign: 'left' },
  help: { paddingHorizontal: theme.base.space[4], fontSize: theme.base.text.xs, lineHeight: 19 },
}));
