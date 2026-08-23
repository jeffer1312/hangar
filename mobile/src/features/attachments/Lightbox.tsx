import { Modal, Pressable, StatusBar, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Animated, { useAnimatedStyle, useSharedValue } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as m from '../../paraglide/messages';

interface Props {
  visible: boolean;
  uri: string;
  headers?: Record<string, string>;
  filename: string;
  onClose: () => void;
}

export function Lightbox({ visible, uri, headers, filename, onClose }: Props) {
  const { theme } = useUnistyles();
  const insets = useSafeAreaInsets();
  const scale = useSharedValue(1);
  const savedScale = useSharedValue(1);
  const translateX = useSharedValue(0);
  const translateY = useSharedValue(0);
  const savedX = useSharedValue(0);
  const savedY = useSharedValue(0);

  const pinch = Gesture.Pinch()
    .onUpdate((e) => {
      const next = savedScale.value * e.scale;
      scale.value = Math.min(6, Math.max(1, next));
    })
    .onEnd(() => {
      savedScale.value = scale.value;
      if (scale.value <= 1.05) {
        scale.value = 1;
        savedScale.value = 1;
        translateX.value = 0;
        translateY.value = 0;
        savedX.value = 0;
        savedY.value = 0;
      }
    });

  const pan = Gesture.Pan()
    .onUpdate((e) => {
      if (scale.value > 1) {
        translateX.value = savedX.value + e.translationX;
        translateY.value = savedY.value + e.translationY;
      }
    })
    .onEnd(() => {
      savedX.value = translateX.value;
      savedY.value = translateY.value;
    });

  const composed = Gesture.Simultaneous(pinch, pan);

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: translateX.value }, { translateY: translateY.value }, { scale: scale.value }],
  }));

  if (!visible) return null;

  const handleClose = () => {
    scale.value = 1;
    savedScale.value = 1;
    translateX.value = 0;
    translateY.value = 0;
    savedX.value = 0;
    savedY.value = 0;
    onClose();
  };

  return (
    <Modal visible={visible} transparent animationType="fade" onRequestClose={handleClose}>
      <View style={styles.backdrop}>
        <Pressable style={[styles.closeArea, { top: (insets.top > 0 ? insets.top : (StatusBar.currentHeight ?? 24)) + theme.base.space[2] }]} onPress={handleClose} accessibilityLabel={m.anexos_fechar_imagem()} accessibilityRole="button">
          <View style={[styles.closeBtn, { backgroundColor: 'rgba(0,0,0,0.6)', borderColor: 'rgba(255,255,255,0.25)' }]}>
            <Text style={styles.closeTxt}>✕</Text>
          </View>
        </Pressable>
        <GestureDetector gesture={composed}>
          <Animated.View style={[styles.imgWrap, animatedStyle]}>
            <Image source={{ uri, headers }} style={styles.img} contentFit="contain" transition={200} />
          </Animated.View>
        </GestureDetector>
        <Pressable style={styles.tapClose} onPress={handleClose} />
        <Text style={[styles.caption, { color: '#fff' }]} numberOfLines={1}>
          {filename}
        </Text>
      </View>
    </Modal>
  );
}

const styles = StyleSheet.create((theme) => ({
  backdrop: {
    flex: 1,
    backgroundColor: 'rgba(0,0,0,0.92)',
    alignItems: 'center',
    justifyContent: 'center',
    padding: theme.base.space[4],
  },
  closeArea: {
    position: 'absolute',
    right: theme.base.space[4],
    zIndex: 2,
  },
  closeBtn: {
    width: 40,
    height: 40,
    borderRadius: 20,
    borderWidth: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  closeTxt: {
    color: '#fff',
    fontSize: 18,
  },
  imgWrap: {
    width: '100%',
    height: '100%',
    alignItems: 'center',
    justifyContent: 'center',
  },
  img: {
    width: '100%',
    height: '100%',
  },
  tapClose: {
    ...StyleSheet.absoluteFillObject,
  },
  caption: {
    position: 'absolute',
    bottom: theme.base.space[4],
    left: theme.base.space[4],
    right: theme.base.space[4],
    textAlign: 'center',
    fontSize: theme.base.text.xs,
  },
}));
