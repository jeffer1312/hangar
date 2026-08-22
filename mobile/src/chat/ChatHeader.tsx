import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import type { State } from '@hangar/core';
import * as m from '../paraglide/messages';
import { StatePill } from '../features/sessions/StatePill';

// Cabeçalho do chat: voltar (44pt, HIG), nome da sessão e pílula de estado.
export function ChatHeader({
  name,
  state,
  onBack,
  onMore,
  chipLoop,
  chipPlan,
}: {
  name: string;
  state: State | null;
  onBack: () => void;
  onMore: () => void;
  chipLoop?: React.ReactNode;
  chipPlan?: React.ReactNode;
}) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.bar}>
      <Pressable
        onPress={onBack}
        hitSlop={8}
        style={styles.back}
        accessibilityRole="button"
        accessibilityLabel={m.chat_voltar_sessoes()}
      >
        <Ionicons name="chevron-back" size={24} color={theme.tokens.accent.base} />
      </Pressable>
      <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>
        {name}
      </Text>
      {state ? <StatePill state={state} /> : null}
      {chipPlan}
      {chipLoop}
      <Pressable
        onPress={onMore}
        hitSlop={8}
        style={styles.more}
        accessibilityRole="button"
        accessibilityLabel={m.navbar_mais_acoes()}
      >
        <Ionicons name="ellipsis-horizontal" size={20} color={theme.tokens.text.primary} />
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1],
    borderBottomWidth: 1,
    borderBottomColor: theme.tokens.border.subtle,
    minHeight: 44,
  },
  back: {
    width: 44,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
  name: {
    fontSize: theme.base.text.base,
    fontWeight: '600',
    flexShrink: 1,
    minWidth: 0,
    flex: 1,
  },
  more: {
    width: 44,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
    marginLeft: 'auto',
  },
}));
