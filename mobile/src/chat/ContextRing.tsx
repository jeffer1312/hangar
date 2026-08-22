import { View } from 'react-native';
import Svg, { Circle, G } from 'react-native-svg';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as m from '../paraglide/messages';

// Anel de uso de contexto (irmão do ContextRing.svelte da PWA): mesma geometria em viewBox 24,
// arco começa no topo (rotate -90) e cresce anti-horário. >= 80% pinta de aviso; sem leitura,
// anel tracejado cinza (indeterminado). A % fica no chip "ctx" ao lado (fileira do StatusLine).
const R = 9;
const C = 2 * Math.PI * R;

export function ContextRing({ pct }: { pct?: number | null }) {
  const { theme } = useUnistyles();
  const known = typeof pct === 'number' && isFinite(pct);
  const value = known ? Math.min(100, Math.max(0, pct as number)) : 0;
  const offset = C * (1 - value / 100);
  const cor = value >= 80 ? theme.tokens.status.warning : theme.tokens.accent.base;
  return (
    <View
      style={styles.wrap}
      accessible={true}
      accessibilityLabel={m.ctx_uso_contexto()}
      accessibilityValue={
        known ? { min: 0, max: 100, now: Math.round(value) } : { min: 0, max: 100 }
      }
    >
      <Svg width={18} height={18} viewBox="0 0 24 24">
        {known ? (
          <>
            <Circle
              cx="12"
              cy="12"
              r={R}
              stroke={theme.tokens.border.default}
              strokeWidth={3}
              fill="none"
            />
            <G transform="rotate(-90 12 12)">
              <Circle
                cx="12"
                cy="12"
                r={R}
                stroke={cor}
                strokeWidth={3}
                strokeLinecap="round"
                strokeDasharray={C}
                strokeDashoffset={offset}
                fill="none"
              />
            </G>
          </>
        ) : (
          <Circle
            cx="12"
            cy="12"
            r={R}
            stroke={theme.tokens.border.default}
            strokeWidth={3}
            strokeDasharray="3 3"
            fill="none"
          />
        )}
      </Svg>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    alignItems: 'center',
    justifyContent: 'center',
  },
}));
