import RNSlider from '@react-native-community/slider';
import { useUnistyles } from 'react-native-unistyles';

/**
 * O valor só sobe pro store em `onSlidingComplete`: cada `setPanelAlpha` refaz o tema inteiro
 * (dois `updateTheme` e um re-render da árvore), e um por quadro de arrasto trava a tela.
 * O intermediário fica dentro do próprio slider, que já se anima sozinho.
 */
export function Slider({
  valor,
  min,
  max,
  onChange,
  label,
}: {
  valor: number;
  min: number;
  max: number;
  onChange: (v: number) => void;
  label: string;
}) {
  const { theme } = useUnistyles();
  return (
    <RNSlider
      value={valor}
      minimumValue={min}
      maximumValue={max}
      onSlidingComplete={onChange}
      minimumTrackTintColor={theme.tokens.accent.base}
      maximumTrackTintColor={theme.tokens.border.strong}
      thumbTintColor={theme.tokens.accent.base}
      accessibilityLabel={label}
      style={{ width: '100%', height: 40 }}
    />
  );
}
