import { Platform, View, type ViewProps } from 'react-native';
import { BlurView } from 'expo-blur';
import { GlassView, isLiquidGlassAvailable } from 'expo-glass-effect';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';

type Props = ViewProps & { variant?: 'panel' | 'modal' | 'chrome' };

// Um componente só decide o material: iOS 26 = vidro de verdade; iOS antigo = blur; Android = blur ou cor com alpha.
export function Glass({ variant = 'panel', style, children, ...rest }: Props) {
  const { theme, rt } = useUnistyles();
  const [r, g, b] = theme.tokens.glass.panelRgb;
  const alpha =
    variant === 'modal'
      ? theme.tokens.glass.modalAlpha
      : variant === 'chrome'
        ? theme.tokens.glass.solidAlpha
        : theme.panelAlpha;
  const bg = `rgba(${r},${g},${b},${alpha})`;
  if (Platform.OS === 'ios' && isLiquidGlassAvailable()) {
    return (
      <GlassView glassEffectStyle="regular" tintColor={bg} style={[styles.box, style]} {...rest}>
        {children}
      </GlassView>
    );
  }
  return (
    <BlurView intensity={40} tint={rt.themeName === 'dark' ? 'dark' : 'light'} style={[styles.box, style]} {...rest}>
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
