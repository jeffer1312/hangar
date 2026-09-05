import { View } from 'react-native';
import { useUnistyles } from 'react-native-unistyles';
import { rotuloEstado, type State } from '@hangar/core';

// O `stateColors` do core devolve `var(--accent)` e afins — CSS custom property, que no React
// Native vira cor inválida (o ponto sumia). Mesma correspondência, lida do tema do app.
const TOKEN: Record<State, (t: { accent: { base: string }; status: { success: string; warning: string; error: string } }) => string> = {
  working: (t) => t.accent.base,
  idle: (t) => t.status.success,
  awaiting_input: (t) => t.status.warning,
  dead: (t) => t.status.error,
};

// O rótulo vai junto porque em lista densa (seletor de sessão) a cor é a ÚNICA pista de estado:
// sem ele, quem usa leitor de tela não ouve nada.
export function StateDot({ state, size = 8 }: { state: State; size?: number }) {
  const { theme } = useUnistyles();
  return (
    <View
      accessible
      accessibilityLabel={rotuloEstado(state)}
      style={{
        width: size,
        height: size,
        borderRadius: size / 2,
        backgroundColor: TOKEN[state](theme.tokens),
      }}
    />
  );
}
