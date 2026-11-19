import { memo, useState, type ReactNode } from 'react';
import { Pressable, View, type LayoutChangeEvent } from 'react-native';
import { Image } from 'expo-image';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { frameZoom, type WorkReviewSide } from '@hangar/core';
import { Icon } from '../../ui/Icon';

// Véu fora do trecho: uma borda larga em volta da moldura, recortada pelo quadro (o `box-shadow` do desenho).
const VEIL = 999;

type Props = {
  side: WorkReviewSide;
  source: { uri: string; headers: Record<string, string> };
  height: number;
  label: string;
  onPress: () => void;
  /** Recebe o id do asset: o diário registra qual imagem falhou, sem a URL. */
  onError: (asset: string) => void;
  /** Desenho no lugar da imagem, no retângulo que ela ocuparia (a animação da novidade não baixa captura). */
  drawing?: ReactNode;
};

/** Quadro com zoom no `box` da imagem e moldura vermelha no trecho, como o `enquadrar` do desenho. */
export const ZoomFrame = memo(function ZoomFrame({ side, source, height, label, onPress, onError, drawing }: Props) {
  const { theme } = useUnistyles();
  const [width, setWidth] = useState(0);
  const [broken, setBroken] = useState(false);
  const onLayout = (e: LayoutChangeEvent) => setWidth(Math.round(e.nativeEvent.layout.width));
  const zoom = width > 0 ? frameZoom({ width, height }, side) : null;
  return (
    <Pressable onPress={onPress} onLayout={onLayout} accessibilityRole="imagebutton" accessibilityLabel={label}
      style={[styles.frame, { height }]}>
      {broken ? <View style={styles.broken}><Icon name="ImageOff" size={20} color={theme.tokens.text.muted} /></View>
        : zoom ? <>
          {drawing ? <View style={{ position: 'absolute', left: zoom.left, top: zoom.top, width: zoom.imageWidth, height: zoom.imageHeight }}>{drawing}</View>
            : <Image source={source} recyclingKey={source.uri} contentFit="fill" transition={150}
              onError={() => { setBroken(true); onError(side.asset); }}
              style={{ position: 'absolute', left: zoom.left, top: zoom.top, width: zoom.imageWidth, height: zoom.imageHeight }} />}
          {zoom.mark ? <View pointerEvents="none" style={[styles.veil, {
            left: zoom.mark.left - VEIL, top: zoom.mark.top - VEIL,
            width: zoom.mark.width + VEIL * 2, height: zoom.mark.height + VEIL * 2,
          }]}><View style={styles.mark} /></View> : null}
        </> : null}
    </Pressable>
  );
});

// Branco em qualquer tema, como o `.quadro` do desenho: a sobra em volta do print não vira faixa escura.
const PAPER = '#fff';

const styles = StyleSheet.create((theme) => ({
  frame: { position: 'relative', overflow: 'hidden', borderWidth: 1, borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.sm, backgroundColor: PAPER },
  veil: { position: 'absolute', borderWidth: VEIL, borderColor: theme.tokens.glass.shadow },
  mark: { flex: 1, borderWidth: 2, borderColor: theme.tokens.status.error, borderRadius: theme.base.radius.xs },
  broken: { flex: 1, alignItems: 'center', justifyContent: 'center' },
}));
