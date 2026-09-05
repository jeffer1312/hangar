import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import type { State } from '@hangar/core';

// O `stateColors` do core devolve `var(--accent)` e afins — CSS custom property, que no React
// Native vira cor inválida (o ponto sumia). Mesma correspondência, lida do tema do app.
export function StateDot({ state, size = 8 }: { state: State; size?: number }) {
  const { theme } = useUnistyles();
  const cor: Record<State, string> = {
    working: theme.tokens.accent.base,
    idle: theme.tokens.status.success,
    awaiting_input: theme.tokens.status.warning,
    dead: theme.tokens.status.error,
  };
  return (
    <View
      style={{
        width: size,
        height: size,
        borderRadius: size / 2,
        backgroundColor: cor[state] ?? theme.tokens.text.muted,
      }}
    />
  );
}
