import { View } from 'react-native';
import Svg, { Circle, G, Text as SvgText } from 'react-native-svg';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as m from '../paraglide/messages';

// Anel de uso de contexto (irmão do ContextRing.svelte da PWA): mesma geometria em viewBox 24,
// arco começa no topo (rotate -90) e cresce anti-horário, e a % vai DENTRO do círculo.
//
// Em repouso o arco é neutro, não indigo: o indigo é reservado pro que é clicável, e o anel é
// mostrador (mesma escolha da PWA). Aviso a partir de 70%, erro a partir de 90%.
//
// Sem leitura o anel não desenha nada: um círculo vazio no composer não informa e ainda ocupa
// lugar de quem informa.
const R = 9;
const C = 2 * Math.PI * R;

// showValue=false: anel pequeno da linha de status, com a % escrita ao lado — o número de 9 unidades
// dentro de um anel de 12 pt não se lê.
// stroke (pt): só o arco, por cima da borda de quem chama (o avatar), sem trilho.
export function ContextRing({ pct, size = 24, showValue = true, stroke }: { pct?: number | null; size?: number; showValue?: boolean; stroke?: number }) {
  const { theme } = useUnistyles();
  if (typeof pct !== 'number' || !isFinite(pct)) return null;
  const value = Math.min(100, Math.max(0, pct));
  const r = stroke ? 12 - (stroke * 24) / size : R;
  const c = stroke ? 2 * Math.PI * r : C;
  const offset = c * (1 - value / 100);
  const cor =
    value >= 90
      ? theme.tokens.status.error
      : value >= 70
        ? theme.tokens.status.warning
        : theme.tokens.text.secondary;
  return (
    <View
      style={styles.wrap}
      accessible={true}
      accessibilityLabel={m.ctx_uso_contexto()}
      accessibilityValue={{ min: 0, max: 100, now: Math.round(value) }}
    >
      <Svg width={size} height={size} viewBox="0 0 24 24">
        {stroke ? null : <Circle cx="12" cy="12" r={R} stroke={theme.tokens.border.default} strokeWidth={3} fill="none" />}
        <G transform="rotate(-90 12 12)">
          <Circle
            cx="12"
            cy="12"
            r={r}
            stroke={cor}
            strokeWidth={stroke ? (stroke * 24) / size : 3}
            strokeLinecap="round"
            strokeDasharray={c}
            strokeDashoffset={offset}
            fill="none"
          />
        </G>
        {/* só o número, sem '%': o anel já diz que é percentual, e o '%' não caberia */}
        {showValue ? <SvgText x="12" y="12" fontSize="9" fontWeight="600" fill={cor} textAnchor="middle" alignmentBaseline="central">
          {Math.round(value)}
        </SvgText> : null}
      </Svg>
    </View>
  );
}

const styles = StyleSheet.create(() => ({
  wrap: {
    alignItems: 'center',
    justifyContent: 'center',
  },
}));
