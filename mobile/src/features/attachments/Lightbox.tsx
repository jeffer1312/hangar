import { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, StatusBar, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { WebView } from 'react-native-webview';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Animated, { useAnimatedStyle, useSharedValue, withTiming } from 'react-native-reanimated';
import { scheduleOnRN } from 'react-native-worklets';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as m from '../../paraglide/messages';
import { Icon } from '../../ui/Icon';
import { localCopy, saveImage } from './mediaCache';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';

export interface LightboxProps {
  visible: boolean;
  uri: string;
  headers?: Record<string, string>;
  filename: string;
  onClose: () => void;
  kind?: 'image' | 'video';
  /** "2/5" quando a mensagem tem várias mídias. */
  counter?: string;
  /** Presente = galeria: arrastar de lado (sem zoom) e as setas passam de mídia. */
  onStep?: (dir: 1 | -1) => void;
  /** Fundo sólido atrás da imagem: PNG transparente (diagrama) deixaria a tela de trás aparecer. */
  imageBackground?: string;
}

const ARRASTO_PASSA = 60;

export function Lightbox({ visible, uri, headers, filename, onClose, kind = 'image', counter, onStep, imageBackground }: LightboxProps) {
  const { theme } = useUnistyles();
  const insets = useSafeAreaInsets();
  const scale = useSharedValue(1);
  const savedScale = useSharedValue(1);
  const translateX = useSharedValue(0);
  const translateY = useSharedValue(0);
  const savedX = useSharedValue(0);
  const savedY = useSharedValue(0);
  const panUsedZoom = useSharedValue(false);

  const reset = () => {
    scale.value = 1;
    savedScale.value = 1;
    translateX.value = 0;
    translateY.value = 0;
    savedX.value = 0;
    savedY.value = 0;
  };
  // Trocar de mídia na galeria não herda o zoom da anterior; abrir ou fechar por qualquer caminho também não.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(reset, [uri, visible]);

  const handleClose = () => {
    reset();
    onClose();
  };

  const [saving, setSaving] = useState(false);
  const savingNow = useRef(false);
  const save = () => {
    // O `disabled` só vale depois do próximo render: dois toques rápidos abriam duas folhas.
    if (savingNow.current) return;
    savingNow.current = true;
    haptic.selection();
    setSaving(true);
    saveImage(uri, headers, filename)
      .then((gravou) => { if (gravou) toast.ok(m.file_card_saved()); })
      .catch((e: unknown) => toast.erro(e instanceof Error ? e.message : String(e)))
      .finally(() => { savingNow.current = false; setSaving(false); });
  };

  const pinch = Gesture.Pinch()
    .onStart(() => { panUsedZoom.value = true; })
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
    .onBegin(() => { panUsedZoom.value = scale.value > 1; })
    .onUpdate((e) => {
      if (e.numberOfPointers > 1) panUsedZoom.value = true;
      if (scale.value > 1) {
        translateX.value = savedX.value + e.translationX;
        translateY.value = savedY.value + e.translationY;
      } else if (onStep && !panUsedZoom.value) {
        translateX.value = e.translationX;
      }
    })
    .onEnd((e, success) => {
      savedX.value = translateX.value;
      savedY.value = translateY.value;
      // Sem zoom, o arrasto de lado é passar de mídia; com zoom ele é panorâmica.
      if (success && !panUsedZoom.value && scale.value <= 1 && onStep && Math.abs(e.translationX) > ARRASTO_PASSA && Math.abs(e.translationX) > Math.abs(e.translationY)) {
        scheduleOnRN(onStep, e.translationX < 0 ? 1 : -1);
      }
    })
    .onFinalize(() => {
      if (scale.value <= 1) {
        translateX.value = withTiming(0);
        savedX.value = 0;
      }
    });

  // No iOS o toque não limita o deslocamento por padrão: um arrasto rápido também fechava.
  const tap = Gesture.Tap().maxDistance(10).onEnd((_e, success) => {
    if (success && scale.value <= 1) scheduleOnRN(handleClose);
  });

  const composed = Gesture.Race(Gesture.Simultaneous(pinch, pan), tap);

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: translateX.value }, { translateY: translateY.value }, { scale: scale.value }],
  }));

  // Camada por cima da própria tela de anexos, e não uma `Sheet`: a rota que a hospeda JÁ é um
  // `formSheet`, e a folha dentro da folha nasce achatada — o nome do arquivo colava no topo, a
  // imagem não aparecia, e o arrasto de fechar disputava o pan do zoom.
  if (!visible) return null;

  const topo = (insets.top > 0 ? insets.top : (StatusBar.currentHeight ?? 24)) + theme.base.space[2];

  return (
    <View style={StyleSheet.absoluteFill}>
      <View style={styles.backdrop}>
        <Pressable style={[styles.closeArea, { top: topo }]} onPress={handleClose} accessibilityLabel={m.anexos_fechar_imagem()} accessibilityRole="button">
          <View style={styles.roundBtn}>
            <Text style={styles.closeTxt}>✕</Text>
          </View>
        </Pressable>
        <Pressable
          style={[styles.saveArea, { top: topo }]}
          onPress={save}
          disabled={saving}
          hitSlop={2}
          accessibilityLabel={m.anexos_baixar({ nome: filename })}
          accessibilityRole="button"
          accessibilityState={{ busy: saving }}
        >
          <View style={styles.roundBtn}>
            {saving ? <ActivityIndicator size="small" color="#fff" /> : <Icon name="Download" size={18} color="#fff" />}
          </View>
        </Pressable>
        {kind === 'video' ? (
          <View style={[styles.videoWrap, { paddingTop: topo + 48 }]}>
            <VideoPage uri={uri} headers={headers} name={filename} />
          </View>
        ) : (
          <GestureDetector gesture={composed}>
            <Animated.View style={[styles.imgWrap, animatedStyle]}>
              <Image source={{ uri, headers }} style={[styles.img, imageBackground ? { backgroundColor: imageBackground } : null]} contentFit="contain" transition={200} />
            </Animated.View>
          </GestureDetector>
        )}
        <View style={[styles.footer, { bottom: Math.max(insets.bottom, theme.base.space[4]) }]}>
          {onStep ? (
            <Pressable onPress={() => onStep(-1)} style={styles.roundBtn} accessibilityRole="button" accessibilityLabel={m.visor_anterior()}>
              <Icon name="ChevronLeft" size={20} color="#fff" />
            </Pressable>
          ) : null}
          <Text style={styles.caption} numberOfLines={1}>
            {counter ? `${counter} · ${filename}` : filename}
          </Text>
          {onStep ? (
            <Pressable onPress={() => onStep(1)} style={styles.roundBtn} accessibilityRole="button" accessibilityLabel={m.visor_proxima()}>
              <Icon name="ChevronRight" size={20} color="#fff" />
            </Pressable>
          ) : null}
        </View>
      </View>
    </View>
  );
}

// Vídeo toca no WebView (o player nativo do sistema dentro dele): o app não tem expo-video. O do
// servidor vai pro cache antes, porque o player pede o arquivo aos pedaços sem o header do token.
function VideoPage({ uri, headers, name }: { uri: string; headers?: Record<string, string>; name: string }) {
  const comToken = !!headers && Object.keys(headers).length > 0;
  const [src, setSrc] = useState<string | null>(comToken ? null : uri);
  const [erro, setErro] = useState('');
  useEffect(() => {
    if (!comToken) {
      setSrc(uri);
      return;
    }
    let vivo = true;
    setSrc(null);
    setErro('');
    localCopy(uri, headers, name)
      .then((f) => { if (vivo) setSrc(f.uri); })
      .catch((e: unknown) => { if (vivo) setErro(e instanceof Error ? e.message : String(e)); });
    return () => { vivo = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [uri]);

  if (erro) {
    return (
      <Text style={styles.videoMsg} accessibilityRole="alert">
        {`${m.visor_nao_carregou()}\n${erro}`}
      </Text>
    );
  }
  if (!src) {
    return (
      <View style={styles.videoLoading}>
        <ActivityIndicator color="#fff" />
        <Text style={styles.videoMsg}>{m.comum_carregando()}</Text>
      </View>
    );
  }
  // Só a cópia baixada pelo app lê arquivo local; URL vinda do texto do assistente fica em http(s),
  // senão o WebView repassaria outro esquema ao sistema.
  const local = comToken;
  if (!local && !/^https?:\/\//i.test(src)) {
    return <Text style={styles.videoMsg} accessibilityRole="alert">{m.visor_nao_carregou()}</Text>;
  }
  return (
    <WebView
      source={{ uri: src }}
      style={styles.video}
      originWhitelist={local ? ['file://*'] : ['http://*', 'https://*']}
      allowFileAccess={local}
      allowingReadAccessToURL={local ? src.slice(0, src.lastIndexOf('/') + 1) : undefined}
      allowsInlineMediaPlayback
      allowsFullscreenVideo
      mediaPlaybackRequiresUserAction={false}
      onError={(e) => setErro(e.nativeEvent.description || m.arquivo_carregar_erro())}
      onHttpError={(e) => setErro(`HTTP ${e.nativeEvent.statusCode}`)}
    />
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
  // À esquerda do botão de fechar, que tem 40 de largura.
  saveArea: {
    position: 'absolute',
    right: theme.base.space[4] + 40 + theme.base.space[3],
    zIndex: 2,
  },
  roundBtn: {
    width: 40,
    height: 40,
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'rgba(0,0,0,0.6)',
    borderColor: 'rgba(255,255,255,0.25)',
  },
  closeTxt: {
    color: '#fff',
    fontSize: theme.base.text.base,
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
  videoWrap: {
    width: '100%',
    flex: 1,
    paddingBottom: 72,
    justifyContent: 'center',
  },
  video: {
    flex: 1,
    backgroundColor: '#000',
  },
  videoLoading: {
    alignItems: 'center',
    gap: theme.base.space[2],
  },
  videoMsg: {
    color: '#fff',
    textAlign: 'center',
    fontSize: theme.base.text.sm,
  },
  footer: {
    position: 'absolute',
    left: theme.base.space[4],
    right: theme.base.space[4],
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    zIndex: 2,
  },
  caption: {
    flex: 1,
    color: '#fff',
    textAlign: 'center',
    fontSize: theme.base.text.xs,
  },
}));
