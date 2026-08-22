import { Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { rotuloEstado } from '@hangar/core';
import type { State } from '@hangar/core';

const map: Record<State, keyof import('@hangar/core').ThemeTokens['pill']> = {
  working: 'working',
  idle: 'idle',
  awaiting_input: 'input',
  dead: 'dead',
};

export function StatePill({ state }: { state: State }) {
  const { theme } = useUnistyles();
  const key = map[state];
  const pill = (key ? theme.tokens.pill[key] : null) ?? theme.tokens.pill.idle;
  return (
    <View style={[styles.pill, { backgroundColor: pill.bg }]}>
      <Text style={[styles.txt, { color: pill.fg }]}>{rotuloEstado(state)}</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  pill: {
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: 9999,
  },
  txt: {
    fontSize: 11,
    fontWeight: '600',
    letterSpacing: 0.2,
  },
});
