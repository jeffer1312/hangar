import { Pressable, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { planBadge } from '@hangar/core';
import type { SessionInfo } from '@hangar/core';
import * as m from '../../paraglide/messages';

interface Props {
  session: SessionInfo | null | undefined;
  onPress: () => void;
}

export function PlanChip({ session, onPress }: Props) {
  const { theme } = useUnistyles();
  const badge = planBadge(session);
  if (!badge) return null;
  return (
    <Pressable
      onPress={onPress}
      style={[styles.chip, { backgroundColor: theme.tokens.bg.surface, borderColor: badge.complete ? theme.tokens.status.success : theme.tokens.accent.base }]}
      accessibilityRole="button"
      accessibilityLabel={m.ctx_plano()}
    >
      <Text style={[styles.text, { color: badge.complete ? theme.tokens.status.success : theme.tokens.text.primary }]} numberOfLines={1}>
        {badge.label}
      </Text>
    </Pressable>
  );
}

const styles = StyleSheet.create({
  chip: {
    minHeight: 28,
    maxWidth: 140,
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
