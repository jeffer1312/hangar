import { useState, type ReactNode } from 'react';
import { Pressable, Switch, Text, TextInput, View, type TextInputProps } from 'react-native';
import { MenuView } from '@react-native-menu/menu';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { useSettingsColors } from '../config/colors';

// Linhas da página Memória no mesmo desenho da `MachineRow`: nome à esquerda, valor atual à direita.

function Texts({ title, description, error }: { title: string; description?: string; error?: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View style={styles.texts}>
    <Text style={[styles.label, { color: c.text }]}>{title}</Text>
    {description ? <Text style={[styles.description, { color: c.muted }]}>{description}</Text> : null}
    {error ? <Text selectable style={[styles.description, { color: theme.tokens.status.error }]}>{error}</Text> : null}
  </View>;
}

/** O rótulo próprio da linha cala os textos de dentro: a falha precisa ir junto na dica. */
const hintOf = (description?: string, error?: string) => [description, error].filter(Boolean).join('\n') || undefined;

export function SwitchRow({ title, description, value, disabled, onChange }: {
  title: string; description?: string; value: boolean; disabled?: boolean; onChange: (value: boolean) => void;
}) {
  const c = useSettingsColors();
  return <View style={[styles.row, { borderTopColor: c.border }]}>
    <Texts title={title} description={description} />
    <Switch accessibilityLabel={title} accessibilityHint={description} value={value} disabled={disabled}
      onValueChange={(v) => { haptic.selection(); onChange(v); }} />
  </View>;
}

export type PickerOption = { id: string; title: string; selected?: boolean; disabled?: boolean };
/** Menu nativo preso à linha, como o seletor de servidor das Configurações. */
export function PickerRow({ title, value, description, error, options, disabled, onPick }: {
  title: string; value: string; description?: string; error?: string; options: PickerOption[]; disabled?: boolean; onPick: (id: string) => void;
}) {
  const c = useSettingsColors();
  return <MenuView title={title} actions={options.map((o) => ({ id: o.id, title: o.title, state: o.selected ? 'on' as const : 'off' as const, attributes: { disabled: !!o.disabled } }))}
    onPressAction={({ nativeEvent }) => { if (!disabled) onPick(nativeEvent.event); }}>
    <Pressable accessibilityRole="button" accessibilityLabel={title} accessibilityValue={{ text: value }} accessibilityHint={hintOf(description, error)}
      accessibilityState={{ disabled }} disabled={disabled} style={[styles.row, { borderTopColor: c.border, opacity: disabled ? 0.5 : 1 }]}>
      <Texts title={title} description={description} error={error} />
      <Text style={[styles.value, { color: c.muted }]} numberOfLines={1}>{value}</Text>
      <Icon name="ChevronsUpDown" size={15} color={c.faint} />
    </Pressable>
  </MenuView>;
}

/**
 * Campo editado no lugar; grava ao sair do campo, só quando o texto mudou. Terminada a gravação, aceita ou não,
 * o campo volta a mostrar o valor do servidor (`onCommit` só responde depois de reler).
 */
export function FieldRow({ title, description, value, onCommit, disabled, ...input }: Omit<TextInputProps, 'value' | 'onChangeText'> & {
  title: string; description?: string; value: string; disabled?: boolean; onCommit: (text: string) => boolean | Promise<boolean>;
}) {
  const c = useSettingsColors();
  // `null` = mostra o valor do servidor; texto = o que a pessoa digitou e ainda não foi resolvido.
  const [draft, setDraft] = useState<string | null>(null);
  const commit = async () => {
    if (draft !== null && draft !== value) await onCommit(draft);
    setDraft(null);
  };
  return <View style={[styles.row, { borderTopColor: c.border }]}>
    <Texts title={title} description={description} />
    <TextInput {...input} value={draft ?? value} onChangeText={setDraft} onEndEditing={() => void commit()} editable={!disabled}
      autoCapitalize="none" autoCorrect={false} returnKeyType="done" accessibilityLabel={title} accessibilityHint={description}
      placeholderTextColor={c.faint} style={[styles.input, { color: c.text }]} />
  </View>;
}

export function TapRow({ title, value, description, error, onPress, disabled, children }: {
  title: string; value?: string; description?: string; error?: string; onPress: () => void; disabled?: boolean; children?: ReactNode;
}) {
  const c = useSettingsColors();
  return <Pressable accessibilityRole="button" accessibilityLabel={title} accessibilityValue={value ? { text: value } : undefined}
    accessibilityHint={hintOf(description, error)} accessibilityState={{ disabled }} disabled={disabled} onPress={onPress}
    style={({ pressed }) => [styles.row, { borderTopColor: c.border, opacity: disabled ? 0.5 : 1 }, pressed && { backgroundColor: c.hover }]}>
    <Texts title={title} description={description} error={error} />
    {children}
    {value ? <Text style={[styles.value, { color: c.muted }]} numberOfLines={1}>{value}</Text> : null}
    <Icon name="ChevronRight" size={17} color={c.faint} />
  </Pressable>;
}

export function PlainRow({ title, description, error }: { title: string; description?: string; error?: string }) {
  const c = useSettingsColors();
  return <View style={[styles.row, { borderTopColor: c.border }]}><Texts title={title} description={description} error={error} /></View>;
}

/** Linha com um botão de texto à direita (Dispensar). */
export function ActionRow({ title, description, action, onAction, disabled }: {
  title: string; description?: string; action: string; onAction: () => void; disabled?: boolean;
}) {
  const c = useSettingsColors();
  return <View style={[styles.row, { borderTopColor: c.border }]}>
    <View style={styles.texts}>
      <Text selectable style={[styles.description, styles.body, { color: c.text }]}>{title}</Text>
      {description ? <Text selectable style={[styles.description, { color: c.muted }]}>{description}</Text> : null}
    </View>
    <Pressable accessibilityRole="button" accessibilityLabel={`${action}: ${title}`} accessibilityState={{ disabled }} disabled={disabled}
      onPress={onAction} style={[styles.button, { opacity: disabled ? 0.5 : 1 }]}>
      <Text style={[styles.label, { color: c.accentText }]}>{action}</Text>
    </Pressable>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  body: { fontSize: theme.base.text.sm },
  button: { minWidth: 44, minHeight: 44, justifyContent: 'center', alignItems: 'flex-end' },
  row: { minHeight: 56, flexDirection: 'row', alignItems: 'center', paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], gap: theme.base.space[3], borderTopWidth: 1 },
  texts: { flex: 1, minWidth: 0, gap: theme.base.space[1] },
  label: { fontSize: theme.base.text.base },
  description: { fontSize: theme.base.text.xs, lineHeight: 19 },
  value: { fontSize: theme.base.text.sm, maxWidth: '45%', flexShrink: 1, textAlign: 'right' },
  input: { minWidth: 80, maxWidth: '45%', minHeight: 44, fontSize: theme.base.text.base, textAlign: 'right', paddingVertical: theme.base.space[2] },
}));
