import { useEffect } from 'react';
import Animated, {
  Easing, useAnimatedProps, useAnimatedStyle, useSharedValue, withDelay, withSequence, withTiming, type SharedValue,
} from 'react-native-reanimated';
import Svg, { Path } from 'react-native-svg';
import { View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { HANGAR_MARK_ARCS, HangarMark } from '../../ui/HangarMark';

const AnimatedPath = Animated.createAnimatedComponent(Path);
const ease = Easing.out(Easing.cubic);

function Arc({ d, length, progress }: { d: string; length: number; progress: SharedValue<number> }) {
  // Lacuna maior que o traço: em repouso nem a ponta redonda aparece.
  const props = useAnimatedProps(() => ({ strokeDashoffset: (length + 1) * (1 - progress.value) }));
  return <AnimatedPath d={d} strokeDasharray={[length, length + 2]} animatedProps={props} />;
}

// Abertura da tela inicial: a marca se desenha e some encolhendo. O SVG só troca o traço; escala e
// opacidade ficam na View, porque transform em <Svg> repinta a tela inteira.
// `epoch` muda a cada vez que a tela aparece; quem chama não monta isto com "Reduzir movimento".
export function HomeIntroMark({ epoch }: { epoch: number }) {
  const { theme } = useUnistyles();
  const outer = useSharedValue(0);
  const inner = useSharedValue(0);
  const shown = useSharedValue(0);

  useEffect(() => {
    outer.value = 0;
    inner.value = 0;
    shown.value = 0;
    outer.value = withTiming(1, { duration: 420, easing: ease });
    inner.value = withDelay(80, withTiming(1, { duration: 380, easing: ease }));
    shown.value = withSequence(
      withTiming(1, { duration: 120, easing: ease }),
      withDelay(340, withTiming(0, { duration: 240, easing: ease })),
    );
  }, [epoch, outer, inner, shown]);

  const style = useAnimatedStyle(() => ({ opacity: shown.value, transform: [{ scale: 0.8 + 0.2 * shown.value }] }));
  const [a, b] = HANGAR_MARK_ARCS;

  return (
    <Animated.View style={[styles.wrap, style]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Svg width={56} height={56} viewBox="0 0 24 24" fill="none" stroke={theme.tokens.text.secondary} strokeWidth={2.1} strokeLinecap="round">
        <Arc d={a.d} length={a.length} progress={outer} />
        <Arc d={b.d} length={b.length} progress={inner} />
      </Svg>
    </Animated.View>
  );
}

// A marca parada e apagada no vazio acima do título: decoração, fora do toque e do leitor de tela.
export function HomeQuietMark({ size }: { size: number }) {
  const { theme } = useUnistyles();
  return (
    <View style={[styles.wrap, styles.quiet]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <HangarMark size={size} color={theme.tokens.text.secondary} />
    </View>
  );
}

const styles = StyleSheet.create({
  wrap: { ...StyleSheet.absoluteFillObject, alignItems: 'center', justifyContent: 'center' },
  quiet: { opacity: 0.09 },
});
