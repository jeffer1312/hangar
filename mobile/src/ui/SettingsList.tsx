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

// `split`: o nome à esquerda e a dica à direita, apagada, como as linhas do menu de modelo do PC.
export function SettingsList({ items, onPick, searchable = true, comfortable = false, split = false }: {
  items: SettingsItem[]; onPick: (id: string) => void; searchable?: boolean | 'always'; comfortable?: boolean; split?: boolean;
}) {
  const { theme } = useUnistyles();
  const [query, setQuery] = useState('');
  const long = searchable === 'always' || (searchable && items.length > 8);
  const q = long ? query.trim().toLowerCase() : '';
  const visible = q ? items.filter((it) => `${it.label} ${it.hint ?? ''}`.toLowerCase().includes(q)) : items;
  return <>
    {long ? <View style={[styles.search, comfortable && styles.comfortableSearch]}>
      <Icon name="Search" size={14} color={theme.tokens.text.muted} />
      <TextInput value={query} onChangeText={setQuery} placeholder={m.native_ctl_search()}
        accessibilityLabel={m.native_ctl_search()} placeholderTextColor={theme.tokens.text.muted}
        autoCorrect={false} autoCapitalize="none" returnKeyType="search" style={[styles.searchInput, comfortable && styles.comfortableSearchInput]} />
    </View> : null}
    {visible.length ? <ScrollView style={comfortable ? styles.comfortableList : styles.list} keyboardShouldPersistTaps="handled" nestedScrollEnabled accessibilityRole="radiogroup">
      {visible.map((it) => <Pressable key={it.id} onPress={() => onPick(it.id)} disabled={it.disabled}
        style={({ pressed }) => [styles.row, comfortable && styles.comfortableRow, split && styles.splitRow, { backgroundColor: it.selected ? theme.tokens.accent.dim : pressed ? theme.tokens.fillSubtle : 'transparent' }, it.disabled && styles.disabled]}
        accessibilityRole="radio" accessibilityLabel={it.label} accessibilityHint={it.hint}
        accessibilityState={{ selected: !!it.selected, checked: !!it.selected, disabled: !!it.disabled }}>
        {it.leading}
        {comfortable ? <View style={styles.rowText}>
          <Text style={styles.comfortableName}>{it.label}</Text>
          {it.hint ? <Text style={styles.comfortableHint}>{it.hint}</Text> : null}
        </View> : split ? <>
          <Text style={[styles.rowText, styles.modelName, it.labelColor ? { color: it.labelColor } : null]} numberOfLines={1}>{it.label}</Text>
          {it.hint ? <Text style={[styles.detail, styles.splitHint]} numberOfLines={1}>{it.hint}</Text> : null}
        </> : <Text style={styles.rowText} numberOfLines={2}>
          <Text style={[styles.modelName, it.labelColor ? { color: it.labelColor } : null]}>{it.label}</Text>
          {it.hint ? <Text style={styles.detail}>{`  ${it.hint}`}</Text> : null}
        </Text>}
        {comfortable ? <View style={[styles.indicator, it.selected && styles.indicatorSelected]}>
          {it.selected ? <Icon name="Check" size={15} color={theme.tokens.accent.base} /> : null}
        </View> : it.selected ? <Icon name="Check" size={16} color={theme.tokens.accent.base} /> : null}
      </Pressable>)}
    </ScrollView> : <Text style={styles.empty}>{q ? m.native_ctl_no_results() : m.comum_nenhum_modelo()}</Text>}
  </>;
}

const styles = StyleSheet.create((theme) => ({
  search: { height: 32, marginHorizontal: theme.base.space[1], marginBottom: theme.base.space[1], flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5],
    paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.sm, borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: theme.tokens.fillSubtle },
  searchInput: { flex: 1, padding: 0, fontSize: theme.base.text.xs, color: theme.tokens.text.primary },
  comfortableSearch: { height: 44 },
  comfortableSearchInput: { fontSize: theme.base.text.sm },
  list: { flexGrow: 0, flexShrink: 1, minHeight: 72, maxHeight: 288 },
  comfortableList: { flexGrow: 0, flexShrink: 1, minHeight: 0 },
  comfortableRow: { minHeight: 66, paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[3], borderRadius: theme.base.radius.lg },
  comfortableName: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  comfortableHint: { marginTop: theme.base.space[1], fontSize: theme.base.text.xxs, color: theme.tokens.text.secondary },
  indicator: { width: 24, height: 24, borderRadius: theme.base.radius.full, borderWidth: 1, borderColor: theme.tokens.border.default, alignItems: 'center', justifyContent: 'center' },
  indicatorSelected: { borderColor: 'transparent', backgroundColor: theme.tokens.accent.dim },
  row: { minHeight: 36, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[1.5], borderRadius: theme.base.radius.sm },
  rowText: { flex: 1, minWidth: 0, fontSize: theme.base.text.xs, color: theme.tokens.text.primary },
  modelName: { fontWeight: theme.base.weight.medium },
  splitRow: { minHeight: 44 },
  splitHint: { flexShrink: 1, maxWidth: '50%', textAlign: 'right' },
  detail: { fontSize: theme.base.text.xxs, color: theme.tokens.text.muted },
  empty: { paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[4], fontSize: theme.base.text.xxs, textAlign: 'center', color: theme.tokens.text.muted },
  disabled: { opacity: 0.5 },
}));
