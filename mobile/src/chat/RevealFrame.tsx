import { type ReactNode, useContext, useEffect } from 'react';
import { StyleSheet as RNStyleSheet, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { LinearGradient } from 'expo-linear-gradient';
import Animated, { cancelAnimation, Easing, useAnimatedStyle, useSharedValue, withRepeat, withTiming } from 'react-native-reanimated';
import { WorkingMarkPaused } from '../ui/WorkingMark';

/** Caixa da imagem citada: brilho enquanto `chegou` é falso; quando vira verdadeiro, o conteúdo abre do centro. */
export function RevealFrame({ style, chegou, children }: { style: object; chegou: boolean; children: ReactNode }) {
  const { theme } = useUnistyles();
  const flat = RNStyleSheet.flatten(style) as { width: number; height: number };
  const { width: w, height: h } = flat;
  const diag = Math.hypot(w, h);
  // Já chegada ao montar (quadro parado da novidade com movimento reduzido): nasce aberta, sem animar.
  const abre = useSharedValue(chegou ? 1 : 0);
  const passada = useSharedValue(0);

  useEffect(() => {
    if (!chegou) return;
    cancelAnimation(passada);
    abre.value = withTiming(1, { duration: 1400, easing: Easing.inOut(Easing.quad) });
  }, [chegou, abre, passada]);

  // O brilho é contínuo: para com a tela escondida (gaveta aberta, outra tela à frente), como a marca de trabalhando.
  const pausado = useContext(WorkingMarkPaused);
  useEffect(() => {
    if (chegou) return;
    abre.value = 0;
    if (pausado) return;
    passada.value = 0;
    // Teto de passadas: carregamento travado numa linha fora da tela não deixa o brilho rodando para sempre.
    passada.value = withRepeat(withTiming(1, { duration: 1400, easing: Easing.linear }), 10);
    return () => cancelAnimation(passada);
  }, [chegou, pausado, abre, passada]);

  // Um círculo que cresce do centro até a diagonal recorta a imagem, que fica parada no lugar.
  const recorte = useAnimatedStyle(() => {
    const d = abre.value * diag;
    return { width: d, height: d, borderRadius: d / 2, left: (w - d) / 2, top: (h - d) / 2, opacity: Math.min(1, abre.value * 2) };
  });
  const dentro = useAnimatedStyle(() => {
    const d = abre.value * diag;
    return { left: (w - d) / -2, top: (h - d) / -2, transform: [{ scale: 1.08 - 0.08 * abre.value }] };
  });
  const brilho = useAnimatedStyle(() => ({ transform: [{ translateX: -w + passada.value * 2 * w }] }));

  return (
    <View style={[style, styles.revealBox]}>
      {!chegou && (
        <Animated.View pointerEvents="none" style={[styles.band, { width: w }, brilho]}>
          <LinearGradient colors={['transparent', `${theme.tokens.text.primary}1F`, 'transparent']} start={{ x: 0, y: 0.5 }} end={{ x: 1, y: 0.5 }} style={styles.fill} />
        </Animated.View>
      )}
      <Animated.View style={[styles.clip, recorte]}>
        <Animated.View style={[styles.clip, { width: w, height: h }, dentro]}>{children}</Animated.View>
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create(() => ({
  revealBox: { overflow: 'hidden' },
  band: { position: 'absolute', top: 0, bottom: 0, left: 0 },
  fill: { flex: 1 },
  clip: { position: 'absolute', overflow: 'hidden' },
}));
