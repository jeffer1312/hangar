import { View } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { LinearGradient } from 'expo-linear-gradient';
import { RevealFrame } from '../../chat/ImageThumb';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';

// Roteiro da `revelacao-imagem`: a caixa da imagem citada brilha enquanto carrega e, quando ela chega, abre do centro.
const CYCLE = 6;
const ARRIVE = 1.8;
const STEPS = [ARRIVE];
// Movimento reduzido: a imagem já aberta.
const STILL = 4;

export function ImageRevealAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const w = Math.min(width - 24, 280);
  const h = Math.round(w * 0.62);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <RevealFrame style={[styles.thumb, { width: w, height: h }]} chegou={t >= ARRIVE}>
      {/* Imagem de exemplo: faixas de cor em diagonal no lugar de uma foto baixada. */}
      <LinearGradient colors={['#5b6cff', '#3fb6a8', '#f2a65a']} start={{ x: 0, y: 0 }} end={{ x: 1, y: 1 }} style={{ width: w, height: h }} />
    </RevealFrame>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: {
    padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  thumb: { borderRadius: theme.base.radius.sm },
}));
