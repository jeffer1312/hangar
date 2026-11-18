import { useEffect, useState } from 'react';
import { Modal, Pressable, Text, View, useWindowDimensions, type LayoutChangeEvent } from 'react-native';
import { Image } from 'expo-image';
import { Gesture, GestureDetector, GestureHandlerRootView } from 'react-native-gesture-handler';
import Animated, { useAnimatedStyle, useSharedValue } from 'react-native-reanimated';
import { scheduleOnRN } from 'react-native-worklets';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import * as m from '../../paraglide/messages';

type Source = { uri: string; headers: Record<string, string> };

export type CurtainItem = {
  title: string;
  before: Source | null;
  after: Source | null;
  leftTag: string;
  rightTag: string;
  /** Altura ÷ largura, quando o conector disse o tamanho; sem ele vale o da imagem carregada. */
  aspect?: number;
};

type Props = { items: CurtainItem[]; index: number; onIndex: (index: number) => void; onClose: () => void };

const STEP = 0.1;

/** Cortina de antes e depois: arrastar a divisória revela um lado sobre o outro; anterior e próxima passam de item. */
export function CompareCurtain({ items, index, onIndex, onClose }: Props) {
  const { theme } = useUnistyles();
  const window = useWindowDimensions();
  const item = items[index];
  const [box, setBox] = useState(0);
  const [loaded, setLoaded] = useState<number | null>(null);
  const [pct, setPct] = useState(50);
  const pos = useSharedValue(0.5);
  const width = useSharedValue(0);

  // Cada item abre com a divisória no meio, como o desenho.
  useEffect(() => { pos.value = 0.5; setPct(50); setLoaded(null); }, [index, pos]);

  const aspect = item?.aspect ?? loaded ?? 9 / 16;
  // Em pé a largura manda; deitado, a altura da tela limita e a imagem encolhe centrada.
  const imageWidth = box > 0 ? Math.min(box, (window.height * 0.62) / aspect) : 0;
  useEffect(() => { width.value = imageWidth; }, [imageWidth, width]);

  const pan = Gesture.Pan().minDistance(0)
    .onBegin((e) => { if (width.value > 0) pos.value = Math.min(1, Math.max(0, e.x / width.value)); })
    .onUpdate((e) => { if (width.value > 0) pos.value = Math.min(1, Math.max(0, e.x / width.value)); })
    .onFinalize(() => { scheduleOnRN(setPct, Math.round(pos.value * 100)); });
  const clip = useAnimatedStyle(() => ({ left: pos.value * width.value }));
  const shifted = useAnimatedStyle(() => ({ left: -pos.value * width.value }));
  const bar = useAnimatedStyle(() => ({ left: pos.value * width.value - 1 }));

  const adjust = (delta: number) => {
    const next = Math.min(1, Math.max(0, pos.value + delta));
    pos.value = next;
    setPct(Math.round(next * 100));
  };
  const step = (dir: 1 | -1) => { haptic.selection(); onIndex((index + dir + items.length) % items.length); };
  const onLayout = (e: LayoutChangeEvent) => setBox(Math.round(e.nativeEvent.layout.width));
  if (!item) return null;
  const imageHeight = imageWidth * aspect;
  const many = items.length > 1;

  return (
    <Modal visible transparent animationType="fade" statusBarTranslucent onRequestClose={onClose}>
      <GestureHandlerRootView style={styles.root}>
        <Pressable style={styles.backdrop} onPress={onClose} accessibilityRole="button" accessibilityLabel={m.native_close()} />
        <View style={styles.card}>
          <View style={styles.top}>
            <Text style={styles.title} accessibilityRole="header">{item.title}</Text>
            <Pressable onPress={onClose} style={styles.iconButton} accessibilityRole="button" accessibilityLabel={m.native_close()}>
              <Icon name="X" size={20} color={theme.tokens.text.secondary} />
            </Pressable>
          </View>
          <View onLayout={onLayout} style={styles.stage}>
            {imageWidth > 0 ? (
              <GestureDetector gesture={pan}>
                <View style={[styles.curtain, { width: imageWidth, height: imageHeight }]}
                  accessible accessibilityRole="adjustable" accessibilityLabel={m.work_review_divider_label()}
                  accessibilityValue={{ min: 0, max: 100, now: pct }}
                  accessibilityActions={[{ name: 'increment' }, { name: 'decrement' }]}
                  onAccessibilityAction={(e) => adjust(e.nativeEvent.actionName === 'increment' ? STEP : -STEP)}>
                  {item.before ? <Image source={item.before} recyclingKey={item.before.uri} contentFit="cover" contentPosition="top left"
                    onLoad={(e) => { if (e.source.width > 0) setLoaded(e.source.height / e.source.width); }}
                    style={{ width: imageWidth, height: imageHeight }} /> : null}
                  {item.after ? <Animated.View style={[styles.clip, clip]}>
                    <Animated.View style={[styles.shifted, { width: imageWidth, height: imageHeight }, shifted]}>
                      <Image source={item.after} recyclingKey={item.after.uri} contentFit="cover" contentPosition="top left"
                        style={{ width: imageWidth, height: imageHeight }} />
                    </Animated.View>
                  </Animated.View> : null}
                  {item.after ? <Animated.View pointerEvents="none" style={[styles.bar, bar]} /> : null}
                  <Text style={[styles.tag, styles.tagLeft]}>{item.leftTag}</Text>
                  {item.after ? <Text style={[styles.tag, styles.tagRight]}>{item.rightTag}</Text> : null}
                </View>
              </GestureDetector>
            ) : null}
          </View>
          {item.after ? <Text style={styles.hint}>{m.work_review_drag_hint()}</Text> : null}
          {many ? (
            <View style={styles.nav}>
              <Text style={styles.position}>{m.work_review_position({ index: index + 1, total: items.length })}</Text>
              <Pressable onPress={() => step(-1)} style={styles.navButton} accessibilityRole="button" accessibilityLabel={m.work_review_previous_short()}>
                <Icon name="ChevronLeft" size={16} color={theme.tokens.text.primary} />
                <Text style={styles.navText}>{m.work_review_previous_short()}</Text>
              </Pressable>
              <Pressable onPress={() => step(1)} style={styles.navButton} accessibilityRole="button" accessibilityLabel={m.work_review_next_short()}>
                <Text style={styles.navText}>{m.work_review_next_short()}</Text>
                <Icon name="ChevronRight" size={16} color={theme.tokens.text.primary} />
              </Pressable>
            </View>
          ) : null}
        </View>
      </GestureHandlerRootView>
    </Modal>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: { flex: 1, justifyContent: 'center', padding: theme.base.space[4] },
  backdrop: { ...StyleSheet.absoluteFillObject, backgroundColor: theme.ink.scrim },
  card: { gap: theme.base.space[2], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderWidth: 1,
    borderColor: theme.tokens.border.default, backgroundColor: theme.tokens.bg.surface },
  top: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  title: { flex: 1, ...theme.type.label, color: theme.tokens.text.primary },
  iconButton: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  stage: { alignItems: 'center' },
  curtain: { position: 'relative', overflow: 'hidden', borderRadius: theme.base.radius.sm, borderWidth: 1,
    borderColor: theme.tokens.border.default, backgroundColor: theme.tokens.bg.base },
  clip: { position: 'absolute', top: 0, bottom: 0, right: 0, overflow: 'hidden' },
  shifted: { position: 'absolute', top: 0 },
  bar: { position: 'absolute', top: 0, bottom: 0, width: 2, backgroundColor: theme.tokens.accent.base },
  tag: { position: 'absolute', top: theme.base.space[2], overflow: 'hidden', ...theme.type.caption, color: theme.ink.onAccent,
    paddingHorizontal: theme.base.space[1.5], paddingVertical: theme.base.space[0.5], borderRadius: theme.base.radius.xs,
    backgroundColor: theme.ink.scrim },
  tagLeft: { left: theme.base.space[2] },
  tagRight: { right: theme.base.space[2] },
  hint: { ...theme.type.caption, color: theme.tokens.text.muted, textAlign: 'center' },
  nav: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  position: { flex: 1, ...theme.type.meta, color: theme.tokens.text.muted },
  navButton: { minHeight: 44, minWidth: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1],
    paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.sm, borderWidth: 1, borderColor: theme.tokens.border.default },
  navText: { ...theme.type.control, color: theme.tokens.text.primary },
}));
