import { useState, type ReactNode } from 'react';
import { Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from './Icon';
import * as m from '../paraglide/messages';

export interface SettingsItem {
  id: string;
  label: string;
  hint?: string;
  selected?: boolean;
  disabled?: boolean;
  // Ícone da etiqueta (máquina, conta) antes do nome, e a cor dela no nome.
  leading?: ReactNode;
  labelColor?: string;
}

export function SettingsList({ items, onPick, searchable = true }: {
  items: SettingsItem[]; onPick: (id: string) => void; searchable?: boolean;
}) {
  const { theme } = useUnistyles();
  const [query, setQuery] = useState('');
  const long = searchable && items.length > 8;
  const q = long ? query.trim().toLowerCase() : '';
  const visible = q ? items.filter((it) => `${it.label} ${it.hint ?? ''}`.toLowerCase().includes(q)) : items;
  return <>
    {long ? <View style={styles.search}>
      <Icon name="Search" size={14} color={theme.tokens.text.muted} />
      <TextInput value={query} onChangeText={setQuery} placeholder={m.native_ctl_search()}
        accessibilityLabel={m.native_ctl_search()} placeholderTextColor={theme.tokens.text.muted}
        autoCorrect={false} autoCapitalize="none" returnKeyType="search" style={styles.searchInput} />
    </View> : null}
    {visible.length ? <ScrollView style={styles.list} keyboardShouldPersistTaps="handled" nestedScrollEnabled accessibilityRole="radiogroup">
      {visible.map((it) => <Pressable key={it.id} onPress={() => onPick(it.id)} disabled={it.disabled}
        style={({ pressed }) => [styles.row, { backgroundColor: it.selected ? theme.tokens.accent.dim : pressed ? theme.tokens.fillSubtle : 'transparent' }, it.disabled && styles.disabled]}
        accessibilityRole="radio" accessibilityLabel={it.label} accessibilityHint={it.hint}
        accessibilityState={{ selected: !!it.selected, disabled: !!it.disabled }}>
        {it.leading}
        <Text style={styles.rowText} numberOfLines={2}>
          <Text style={[styles.modelName, it.labelColor ? { color: it.labelColor } : null]}>{it.label}</Text>
          {it.hint ? <Text style={styles.detail}>{`  ${it.hint}`}</Text> : null}
        </Text>
        {it.selected ? <Icon name="Check" size={16} color={theme.tokens.accent.base} /> : null}
      </Pressable>)}
    </ScrollView> : <Text style={styles.empty}>{q ? m.native_ctl_no_results() : m.comum_nenhum_modelo()}</Text>}
  </>;
}

const styles = StyleSheet.create((theme) => ({
  search: { height: 32, marginHorizontal: theme.base.space[1], marginBottom: theme.base.space[1], flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5],
    paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm, borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: theme.tokens.fillSubtle },
  searchInput: { flex: 1, padding: 0, fontSize: theme.base.text.xs, color: theme.tokens.text.primary },
  list: { flexGrow: 0, flexShrink: 1, minHeight: 72, maxHeight: 288 },
  row: { minHeight: 36, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[1.5], borderRadius: theme.base.radius.sm },
  rowText: { flex: 1, minWidth: 0, fontSize: theme.base.text.xs, color: theme.tokens.text.primary },
  modelName: { fontWeight: theme.base.weight.medium },
  detail: { fontSize: theme.base.text.xxs, color: theme.tokens.text.muted },
  empty: { paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[4], fontSize: theme.base.text.xxs, textAlign: 'center', color: theme.tokens.text.muted },
  disabled: { opacity: 0.5 },
}));
