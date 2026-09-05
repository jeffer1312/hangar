import { useEffect, useState } from 'react';
import { AccessibilityInfo, Platform, View, type ViewProps } from 'react-native';
import { BlurView } from 'expo-blur';
import { GlassView, isLiquidGlassAvailable } from 'expo-glass-effect';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';

type Props = ViewProps & { variant?: 'panel' | 'modal' | 'chrome' };

export function useReduceTransparency() {
  const [on, setOn] = useState(false);
  useEffect(() => {
    AccessibilityInfo.isReduceTransparencyEnabled().then(setOn).catch(() => setOn(false));
    const sub = AccessibilityInfo.addEventListener('reduceTransparencyChanged', setOn);
    return () => sub.remove();
  }, []);
  return on;
}

// Um componente só decide o material: iOS 26 = vidro de verdade; iOS antigo = blur; Android = blur ou cor com alpha.
export function Glass({ variant = 'panel', style, children, ...rest }: Props) {
  const { theme, rt } = useUnistyles();
  const reduzir = useReduceTransparency();
  const [r, g, b] = theme.tokens.glass.panelRgb;
  const alpha =
    variant === 'modal'
      ? theme.tokens.glass.modalAlpha
      : variant === 'chrome'
        ? theme.tokens.glass.solidAlpha
        : theme.panelAlpha;
  const bg = `rgba(${r},${g},${b},${alpha})`;
  if (reduzir) {
    return <View style={[styles.box, { backgroundColor: `rgb(${r},${g},${b})` }, style]} {...rest}>{children}</View>;
  }
  if (Platform.OS === 'ios' && isLiquidGlassAvailable()) {
    return (
      <View style={[styles.box, style]} {...rest}>
        {/* key pelo tema: o material do glass-effect não acompanha troca de tema em runtime.
            Isolado do children (só o fundo remonta) pra não perder rascunho/foco de um input dentro. */}
        <GlassView key={rt.themeName} glassEffectStyle="regular" tintColor={bg} style={StyleSheet.absoluteFillObject} pointerEvents="none" />
        {children}
      </View>
    );
  }
  return (
    <BlurView intensity={40} blurMethod="dimezisBlurView" tint={rt.themeName === 'dark' ? 'dark' : 'light'} style={[styles.box, style]} {...rest}>
      <View style={[StyleSheet.absoluteFillObject, { backgroundColor: bg }]} />
      {children}
    </BlurView>
  );
}

const styles = StyleSheet.create((theme) => ({
  box: {
    borderRadius: theme.base.radius.lg,
    borderWidth: 1,
    borderColor: theme.tokens.glass.border,
    overflow: 'hidden',
  },
}));
