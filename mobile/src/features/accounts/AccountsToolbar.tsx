import { memo, useEffect, useState } from 'react';
import { Pressable, Text, TextInput, View, type LayoutChangeEvent } from 'react-native';
import Animated, { useAnimatedStyle, useReducedMotion, useSharedValue, withTiming, Easing } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { ACCOUNT_FILTERS, nextAccountSort, type AccountFilter, type AccountSort } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { Sheet } from '../../ui/Sheet';
import { haptic } from '../../ui/haptics';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

const FILTER_LABEL: Record<AccountFilter, () => string> = {
  all: m.accounts_list_filter_all,
  claude: m.accounts_list_filter_claude,
  codex: m.accounts_list_filter_codex,
  other: m.accounts_list_filter_other,
};
const EASE = Easing.bezier(0.22, 1, 0.36, 1);
// A pílula fica recuada da borda do grupo pelo mesmo vão do preenchimento dele.
const INSET = 3;

export type SortOption = { key: string; label: string };

/** Nome da coluna para as frases do Ordenar; `null` = o padrão. */
export function sortLabel(sort: AccountSort, options: SortOption[]): string {
  if (!sort) return m.accounts_list_sort_default();
  return options.find((o) => o.key === sort.key)?.label ?? sort.key;
}

export const FilterChips = memo(function FilterChips({ value, counts, onChange }: {
  value: AccountFilter; counts: Record<AccountFilter, number>; onChange: (f: AccountFilter) => void;
}) {
  const c = useSettingsColors();
  const reduced = useReducedMotion();
  const [boxes, setBoxes] = useState<Partial<Record<AccountFilter, { x: number; width: number }>>>({});
  const left = useSharedValue(0);
  const width = useSharedValue(0);
  const box = boxes[value];
  useEffect(() => {
    if (!box) return;
    // Primeira medida entra sem deslizar: a pílula nasce no lugar.
    const jump = reduced || width.value === 0;
    left.value = jump ? box.x : withTiming(box.x, { duration: 320, easing: EASE });
    width.value = jump ? box.width : withTiming(box.width, { duration: 320, easing: EASE });
  }, [box?.x, box?.width, reduced]);
  const pill = useAnimatedStyle(() => ({ left: left.value, width: width.value }));
  const measure = (f: AccountFilter) => (e: LayoutChangeEvent) => {
    const { x, width: w } = e.nativeEvent.layout;
    setBoxes((prev) => (prev[f]?.x === x && prev[f]?.width === w ? prev : { ...prev, [f]: { x, width: w } }));
  };
  return (
    <View style={[styles.chips, { borderColor: c.borderStrong }]} accessibilityRole="radiogroup"
      accessibilityLabel={m.accounts_list_filter_label()}>
      {box ? <Animated.View pointerEvents="none" style={[styles.pill, { backgroundColor: c.accentDim }, pill]} /> : null}
      {ACCOUNT_FILTERS.map((f) => {
        const on = f === value;
        return (
          <Pressable key={f} onLayout={measure(f)} hitSlop={4}
            onPress={() => { if (!on) { haptic.selection(); onChange(f); } }}
            accessibilityRole="radio" accessibilityState={{ selected: on }}
            accessibilityLabel={`${FILTER_LABEL[f]()} ${counts[f]}`}
            style={styles.chip}>
            <Text style={[styles.chipText, { color: on ? c.accentText : c.muted }]}>{FILTER_LABEL[f]()}</Text>
            <View style={[styles.count, { backgroundColor: c.hover }]}>
              <Text style={[styles.countText, { color: on ? c.accentText : c.faint }]}>{counts[f]}</Text>
            </View>
          </Pressable>
        );
      })}
    </View>
  );
});

export const AccountSearch = memo(function AccountSearch({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const c = useSettingsColors();
  return (
    <View style={[styles.search, { borderColor: c.borderStrong, backgroundColor: c.inset }]}>
      <Icon name="Search" size={15} color={c.faint} />
      <TextInput value={value} onChangeText={onChange} placeholder={m.accounts_list_search()} placeholderTextColor={c.faint}
        accessibilityLabel={m.accounts_list_search()} autoCapitalize="none" autoCorrect={false} returnKeyType="search"
        clearButtonMode="while-editing" style={[styles.input, { color: c.text }]} />
    </View>
  );
});

export const SortControl = memo(function SortControl({ sort, options, onChange }: {
  sort: AccountSort; options: SortOption[]; onChange: (s: AccountSort) => void;
}) {
  const c = useSettingsColors();
  const [open, setOpen] = useState(false);
  const direction = sort ? (sort.dir > 0 ? m.accounts_list_sort_asc() : m.accounts_list_sort_desc()) : '';
  const label = sortLabel(sort, options);
  const pick = (next: AccountSort) => { setOpen(false); haptic.selection(); onChange(next); };
  return (
    <>
      <Pressable onPress={() => setOpen(true)} hitSlop={6} accessibilityRole="button"
        accessibilityLabel={`${m.accounts_list_sort()}: ${sort ? m.accounts_list_sort_state({ column: label, direction }) : label}`}
        style={({ pressed }) => [styles.sort, { borderColor: c.borderStrong }, pressed && { backgroundColor: c.hover }]}>
        <Icon name="ArrowUpDown" size={14} color={c.text} />
        <Text style={[styles.sortText, { color: c.text }]} numberOfLines={1}>{label}</Text>
        {sort ? <Icon name={sort.dir > 0 ? 'ArrowUp' : 'ArrowDown'} size={13} color={c.accentText} /> : null}
      </Pressable>
      <Sheet open={open} sizes={['auto']} onDismiss={() => setOpen(false)}>
        <View style={styles.sheet}>
          <Text style={[styles.sheetTitle, { color: c.text }]} accessibilityRole="header">{m.accounts_list_sort()}</Text>
          <SortRow label={m.accounts_list_sort_default()} on={!sort} onPress={() => pick(null)} />
          {options.map((o) => {
            const on = sort?.key === o.key;
            return <SortRow key={o.key} label={o.label} on={on} dir={on ? sort!.dir : undefined}
              onPress={() => pick(nextAccountSort(sort, o.key))} />;
          })}
        </View>
      </Sheet>
    </>
  );
});

function SortRow({ label, on, dir, onPress }: { label: string; on: boolean; dir?: 1 | -1; onPress: () => void }) {
  const c = useSettingsColors();
  return (
    <Pressable onPress={onPress} accessibilityRole="radio" accessibilityState={{ selected: on }}
      accessibilityLabel={dir ? m.accounts_list_sort_state({ column: label, direction: dir > 0 ? m.accounts_list_sort_asc() : m.accounts_list_sort_desc() }) : label}
      style={({ pressed }) => [styles.sortRow, on && { backgroundColor: c.accentDim }, pressed && !on && { backgroundColor: c.hover }]}>
      <Text style={[styles.sortRowText, { color: on ? c.accentText : c.text }]}>{label}</Text>
      {dir ? <Icon name={dir > 0 ? 'ArrowUp' : 'ArrowDown'} size={15} color={c.accentText} />
        : on ? <Icon name="Check" size={15} color={c.accentText} /> : null}
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  chips: { flexDirection: 'row', alignSelf: 'flex-start', gap: theme.base.space[0.5], padding: INSET, borderWidth: 1, borderRadius: theme.base.radius.md },
  pill: { position: 'absolute', top: INSET, bottom: INSET, borderRadius: theme.base.radius.sm },
  chip: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], minHeight: 36, paddingHorizontal: theme.base.space[3] },
  chipText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  count: { borderRadius: theme.base.radius.xs, paddingHorizontal: theme.base.space[1.5] },
  countText: { fontSize: theme.base.text.xxs, fontVariant: ['tabular-nums'] },
  search: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 44, paddingHorizontal: theme.base.space[3], borderWidth: 1, borderRadius: theme.base.radius.md },
  input: { flex: 1, minHeight: 44, fontSize: theme.base.text.sm, paddingVertical: 0 },
  sort: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], height: 32, maxWidth: 220, paddingHorizontal: theme.base.space[3], borderWidth: 1, borderRadius: theme.base.radius.full },
  sortText: { flexShrink: 1, fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
  sheet: { gap: theme.base.space[1], padding: theme.base.space[3] },
  sheetTitle: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold, marginBottom: theme.base.space[2], paddingHorizontal: theme.base.space[2] },
  sortRow: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', minHeight: 48, paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md },
  sortRowText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
}));
