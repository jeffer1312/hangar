import { memo, useMemo, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { dreamLineDiff, type DreamFileChange } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { MONO } from '../../theme/scale';
import { useSettingsColors } from '../config/colors';
import * as m from '../../paraglide/messages';

const KIND = { added: m.dream_file_added, removed: m.dream_file_removed, changed: m.dream_file_changed };

/** Um arquivo da rodada; o toque abre o antes e depois, linha a linha. */
export const FileChange = memo(function FileChange({ change, before, after }: { change: DreamFileChange; before: string; after: string }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const [open, setOpen] = useState(false);
  // Só monta a comparação quando aberta.
  const lines = useMemo(() => (open ? dreamLineDiff(before, after) : []), [open, before, after]);
  const tone = change.kind === 'added' ? theme.tokens.status.success : change.kind === 'removed' ? theme.tokens.status.error : c.muted;
  return <View style={[styles.box, { borderTopColor: c.border }]}>
    <Pressable accessibilityRole="button" accessibilityLabel={`${change.file}: ${KIND[change.kind]()}`} accessibilityState={{ expanded: open }}
      onPress={() => setOpen((v) => !v)} style={({ pressed }) => [styles.row, pressed && { backgroundColor: c.hover }]}>
      <Text style={[styles.file, { color: c.text }]} numberOfLines={1} ellipsizeMode="middle">{change.file}</Text>
      <Text style={[styles.kind, { color: tone }]}>{KIND[change.kind]()}</Text>
      <Icon name={open ? 'ChevronUp' : 'ChevronDown'} size={15} color={c.faint} />
    </Pressable>
    {open ? <View style={[styles.diff, { backgroundColor: c.inset }]}>
      {lines.map((line, i) => <Text key={i} selectable style={[styles.line, {
        color: line.kind === '+' ? theme.tokens.status.success : line.kind === '-' ? theme.tokens.status.error : c.muted,
      }]}>{`${line.kind} ${line.text}`}</Text>)}
    </View> : null}
  </View>;
});

const styles = StyleSheet.create((theme) => ({
  box: { borderTopWidth: 1 },
  row: { minHeight: 48, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingHorizontal: theme.base.space[4] },
  file: { flex: 1, minWidth: 0, fontSize: theme.base.text.sm },
  kind: { fontSize: theme.base.text.xs },
  diff: { paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2] },
  line: { fontFamily: MONO, fontSize: theme.base.text.xs, lineHeight: 18 },
}));
