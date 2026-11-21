import { ActivityIndicator, Pressable, View } from 'react-native';
import Svg, { Circle, G, Path } from 'react-native-svg';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { haptic } from '../../ui/haptics';

const R = 12;
const C = 2 * Math.PI * R;

/**
 * Anel do download em andamento com o X no meio: o toque cancela. `pct` nulo = o servidor não disse o
 * tamanho, e o anel vira o indicador do sistema. `onDark` = sobre o fundo escuro do visor.
 */
export function DownloadRing({ pct, onCancel, label, onDark = false }: { pct: number | null; onCancel: () => void; label: string; onDark?: boolean }) {
  const { theme } = useUnistyles();
  const track = onDark ? 'rgba(255,255,255,0.18)' : theme.tokens.border.default;
  const arc = onDark ? '#fff' : theme.tokens.accent.base;
  const mark = onDark ? '#fff' : theme.tokens.text.secondary;
  return (
    <Pressable
      onPress={() => { haptic.selection(); onCancel(); }}
      style={styles.hit}
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityValue={pct == null ? undefined : { min: 0, max: 100, now: pct }}
    >
      <View style={styles.box}>
        {pct == null ? <ActivityIndicator size="small" color={arc} style={styles.spin} /> : null}
        <Svg width={30} height={30} viewBox="0 0 30 30">
          <Circle cx="15" cy="15" r={R} stroke={track} strokeWidth={3} fill="none" />
          {pct == null ? null : (
            <G transform="rotate(-90 15 15)">
              <Circle cx="15" cy="15" r={R} stroke={arc} strokeWidth={3} strokeLinecap="round" fill="none"
                strokeDasharray={C} strokeDashoffset={C * (1 - pct / 100)} />
            </G>
          )}
          <Path d="M11.5 11.5l7 7M18.5 11.5l-7 7" stroke={mark} strokeWidth={1.6} strokeLinecap="round" />
        </Svg>
      </View>
    </Pressable>
  );
}

const styles = StyleSheet.create(() => ({
  hit: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  box: { width: 30, height: 30, alignItems: 'center', justifyContent: 'center' },
  spin: { position: 'absolute', top: 0, right: 0, bottom: 0, left: 0 },
}));
