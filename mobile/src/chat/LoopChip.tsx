import { Pressable, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { loopBadge } from '@hangar/core';
import type { LoopState } from '@hangar/core';
import * as m from '../paraglide/messages';

interface Props {
  status: LoopState['status'] | null | undefined;
  iter?: number | null;
  max?: number | null;
  onPress: () => void;
}

export function LoopChip({ status, iter, max, onPress }: Props) {
  const { theme } = useUnistyles();
  const badge = loopBadge(status, iter, max);
  if (!badge) return null;
  const color = {
    ok: theme.tokens.accent.base,
    warn: theme.tokens.status.error,
    attention: theme.tokens.status.warning,
    muted: theme.tokens.text.muted,
  }[badge.tone];

  return (
    <Pressable
      onPress={onPress}
      style={[styles.chip, { backgroundColor: theme.tokens.bg.surface, borderColor: color }]}
      accessibilityRole="button"
      accessibilityLabel={m.loop_titulo()}
    >
      <Text style={[styles.text, { color }]}>{badge.label}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  chip: {
    minHeight: 28,
    maxWidth: 68,
    paddingHorizontal: 7,
    borderRadius: 9999,
    borderWidth: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  text: {
    fontSize: 11,
    fontWeight: '600',
    fontVariant: ['tabular-nums'],
  },
});
