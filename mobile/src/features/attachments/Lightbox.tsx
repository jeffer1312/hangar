import { memo, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ActivityIndicator, FlatList, Pressable, StatusBar, Text, View, type AccessibilityActionEvent } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { WebView } from 'react-native-webview';
import { Gesture, GestureDetector } from 'react-native-gesture-handler';
import Animated, { useAnimatedStyle, useSharedValue, withTiming } from 'react-native-reanimated';
import { scheduleOnRN } from 'react-native-worklets';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import * as m from '../../paraglide/messages';
import { Icon, type IconName } from '../../ui/Icon';
import { canShareFile, localCopy, saveImage, shareFile } from './mediaCache';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { SPACE } from '../../theme/scale';
import { LightboxActions, LightboxCaption, StripThumbFrame, THUMB_W, type LightboxAction } from './LightboxParts';

export interface LightboxStripItem {
  key: string;
  uri: string;
  headers?: Record<string, string>;
  name: string;
  kind: 'image' | 'video';
}

export interface LightboxProps {
  visible: boolean;
  uri: string;
  headers?: Record<string, string>;
  filename: string;
  onClose: () => void;
  kind?: 'image' | 'video';
  /** "2 de 5" quando há várias mídias. */
  counter?: string;
  /** Presente = galeria: arrastar de lado (sem zoom) passa de mídia. */
  onStep?: (dir: 1 | -1) => void;
  /** Fundo sólido atrás da imagem: PNG transparente (diagrama) deixaria a tela de trás aparecer. */
  imageBackground?: string;
  /** Linha abaixo do nome: tamanho · data · origem. */
  details?: string;
  /** Galeria da conversa: fecha e leva a conversa até a mensagem desta mídia. */
  onShowInChat?: () => void;
  /** Faixa de miniaturas embaixo; o toque troca de mídia. */
  strip?: { items: LightboxStripItem[]; index: number; onSelect: (i: number) => void };
}

const ARRASTO_PASSA = 60;
// Arrastar para baixo além disto (ou rápido) fecha; menos volta ao lugar.
const ARRASTO_FECHA = 120;
const VELOCIDADE_FECHA = 900;
const ZOOM_DUPLO = 2.5;
// Gesto decide o eixo depois desta distância: lado passa de mídia, baixo fecha.
const EIXO = 8;
// Acima disto o segundo toque já não é toque duplo: o toque simples fecha sem esperar o padrão do sistema.
const DUPLO_MS = 250;

/** Quanto a imagem ampliada pode andar para cada lado sem mostrar a borda: metade do que ela, no
 *  tamanho em que aparece (`contain`), passa da caixa. Caixa ainda sem medida não limita. */
export function panLimits(boxW: number, boxH: number, natW: number, natH: number, scale: number): { x: number; y: number } {
  'worklet';
  if (!(boxW > 0 && boxH > 0)) return { x: Infinity, y: Infinity };
  const fit = natW > 0 && natH > 0 ? Math.min(boxW / natW, boxH / natH) : 0;
  const w = fit ? natW * fit : boxW;
  const h = fit ? natH * fit : boxH;
  return { x: Math.max(0, (w * scale - boxW) / 2), y: Math.max(0, (h * scale - boxH) / 2) };
}

export function clampPan(v: number, limit: number): number {
  'worklet';
  return Math.min(limit, Math.max(-limit, v));
}

export function Lightbox({
  visible, uri, headers, filename, onClose, kind = 'image', counter, onStep, imageBackground, details, onShowInChat, strip,
}: LightboxProps) {
  const { theme } = useUnistyles();
  const insets = useSafeAreaInsets();
  const scale = useSharedValue(1);
  const savedScale = useSharedValue(1);
  const translateX = useSharedValue(0);
  const translateY = useSharedValue(0);
  const savedX = useSharedValue(0);
  const savedY = useSharedValue(0);
  const panUsedZoom = useSharedValue(false);
  // 0 = eixo ainda não decidido, 1 = lado, 2 = vertical.
  const axis = useSharedValue(0);
  const dismissY = useSharedValue(0);
  const closing = useSharedValue(false);
  const boxW = useSharedValue(0);
  const boxH = useSharedValue(0);
  // Tamanho natural da imagem aberta (0 = ainda não carregou): limita a panorâmica à imagem, não à caixa.
  const natW = useSharedValue(0);
  const natH = useSharedValue(0);

  const reset = () => {
    scale.value = 1;
    savedScale.value = 1;
    translateX.value = 0;
    translateY.value = 0;
    savedX.value = 0;
    savedY.value = 0;
    dismissY.value = 0;
    closing.value = false;
  };
  // Trocar de mídia na galeria não herda o zoom da anterior; abrir ou fechar por qualquer caminho também não.
  // Antes da pintura: a mídia nova nunca aparece um quadro com o zoom da anterior.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useLayoutEffect(reset, [uri, visible]);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useLayoutEffect(() => { natW.value = 0; natH.value = 0; }, [uri]);

  const handleClose = () => {
    reset();
    onClose();
  };

  // A falha é da URI que falhou: outra mídia na tela começa sem ela. `attempt` remonta a imagem no Tentar novamente.
  const [failedUri, setFailedUri] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const imageFailed = failedUri === uri;
  // Sair da mídia que falhou esquece a falha: voltar a ela tenta carregar de novo.
  useLayoutEffect(() => { setFailedUri((u) => (u === uri ? u : null)); }, [uri]);
  const retryImage = () => {
    haptic.selection();
    setFailedUri(null);
    setAttempt((n) => n + 1);
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

  const [sharing, setSharing] = useState(false);
  const sharingNow = useRef(false);
  const share = () => {
    if (sharingNow.current) return;
    sharingNow.current = true;
    haptic.selection();
    setSharing(true);
    shareFile(uri, headers, filename)
      .catch((e: unknown) => toast.erro(e instanceof Error ? e.message : String(e)))
      .finally(() => { sharingNow.current = false; setSharing(false); });
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
        return;
      }
      // Diminuir o zoom encolhe o limite: a imagem volta a cobrir a caixa em vez de mostrar a borda.
      const lim = panLimits(boxW.value, boxH.value, natW.value, natH.value, scale.value);
      const x = clampPan(translateX.value, lim.x);
      const y = clampPan(translateY.value, lim.y);
      if (x !== translateX.value) translateX.value = withTiming(x);
      if (y !== translateY.value) translateY.value = withTiming(y);
      savedX.value = x;
      savedY.value = y;
    });

  const pan = Gesture.Pan()
    .onBegin(() => {
      panUsedZoom.value = scale.value > 1;
      axis.value = 0;
    })
    .onUpdate((e) => {
      if (e.numberOfPointers > 1) panUsedZoom.value = true;
      if (scale.value > 1) {
        const lim = panLimits(boxW.value, boxH.value, natW.value, natH.value, scale.value);
        translateX.value = clampPan(savedX.value + e.translationX, lim.x);
        translateY.value = clampPan(savedY.value + e.translationY, lim.y);
        return;
      }
      if (panUsedZoom.value) return;
      if (axis.value === 0 && (Math.abs(e.translationX) > EIXO || Math.abs(e.translationY) > EIXO)) {
        axis.value = Math.abs(e.translationX) > Math.abs(e.translationY) ? 1 : 2;
      }
      if (axis.value === 1 && onStep) translateX.value = e.translationX;
      else if (axis.value === 2) dismissY.value = Math.max(0, e.translationY);
    })
    .onEnd((e, success) => {
      if (scale.value > 1) {
        const lim = panLimits(boxW.value, boxH.value, natW.value, natH.value, scale.value);
        translateX.value = clampPan(translateX.value, lim.x);
        translateY.value = clampPan(translateY.value, lim.y);
      }
      savedX.value = translateX.value;
      savedY.value = translateY.value;
      if (!success || panUsedZoom.value || scale.value > 1) return;
      // Sem zoom, o arrasto de lado é passar de mídia e o para baixo é fechar; com zoom ele é panorâmica.
      if (axis.value === 1 && onStep && Math.abs(e.translationX) > ARRASTO_PASSA && Math.abs(e.translationX) > Math.abs(e.translationY)) {
        scheduleOnRN(onStep, e.translationX < 0 ? 1 : -1);
      } else if (axis.value === 2 && (e.translationY > ARRASTO_FECHA || e.velocityY > VELOCIDADE_FECHA)) {
        closing.value = true;
        scheduleOnRN(handleClose);
      }
    })
    .onFinalize(() => {
      if (scale.value <= 1) {
        translateX.value = withTiming(0);
        savedX.value = 0;
      }
      if (!closing.value) dismissY.value = withTiming(0);
    });

  // Toque duplo alterna entre o tamanho original e o zoom no ponto tocado.
  const doubleTap = Gesture.Tap().numberOfTaps(2).maxDistance(10).maxDelay(DUPLO_MS).onEnd((e, success) => {
    if (!success) return;
    if (scale.value > 1) {
      scale.value = withTiming(1);
      translateX.value = withTiming(0);
      translateY.value = withTiming(0);
      savedScale.value = 1;
      savedX.value = 0;
      savedY.value = 0;
      return;
    }
    const lim = panLimits(boxW.value, boxH.value, natW.value, natH.value, ZOOM_DUPLO);
    const tx = clampPan((boxW.value / 2 - e.x) * (ZOOM_DUPLO - 1), lim.x);
    const ty = clampPan((boxH.value / 2 - e.y) * (ZOOM_DUPLO - 1), lim.y);
    scale.value = withTiming(ZOOM_DUPLO);
    translateX.value = withTiming(tx);
    translateY.value = withTiming(ty);
    savedScale.value = ZOOM_DUPLO;
    savedX.value = tx;
    savedY.value = ty;
  });

  // No iOS o toque não limita o deslocamento por padrão: um arrasto rápido também fechava.
  const tap = Gesture.Tap().maxDistance(10).onEnd((_e, success) => {
    if (success && scale.value <= 1) scheduleOnRN(handleClose);
  });

  const composed = Gesture.Race(Gesture.Simultaneous(pinch, pan), Gesture.Exclusive(doubleTap, tap));

  const animatedStyle = useAnimatedStyle(() => ({
    transform: [
      { translateX: translateX.value },
      { translateY: translateY.value + dismissY.value },
      { scale: scale.value * (1 - Math.min(dismissY.value / 1200, 0.25)) },
    ],
  }));
  // O fundo e as barras somem enquanto a foto desce: dá para ver que soltar fecha.
  // Um estilo animado por vista: o Reanimated não garante o mesmo estilo em vários componentes.
  const fade = () => {
    'worklet';
    return { opacity: 1 - Math.min(dismissY.value / 400, 0.7) };
  };
  const backdropStyle = useAnimatedStyle(fade);
  const topStyle = useAnimatedStyle(fade);
  const bottomStyle = useAnimatedStyle(fade);

  // Camada por cima da própria tela de anexos, e não uma `Sheet`: a rota que a hospeda JÁ é um
  // `formSheet`, e a folha dentro da folha nasce achatada — o nome do arquivo colava no topo, a
  // imagem não aparecia, e o arrasto de fechar disputava o pan do zoom.
  if (!visible) return null;

  const topo = (insets.top > 0 ? insets.top : (StatusBar.currentHeight ?? 24)) + theme.base.space[2];
  const actions: LightboxAction[] = [
    { icon: 'Download', label: m.gallery_save(), a11y: m.anexos_baixar({ nome: filename }), onPress: save, busy: saving },
    ...(canShareFile ? [{ icon: 'Share' as IconName, label: m.gallery_share(), a11y: m.gallery_share_named({ nome: filename }), onPress: share, busy: sharing }] : []),
    ...(onShowInChat ? [{ icon: 'MessageSquare' as IconName, label: m.gallery_in_chat(), a11y: m.gallery_in_chat(), onPress: onShowInChat }] : []),
  ];
  // Arrastar não existe para quem usa o VoiceOver: deslizar para cima/baixo passa de mídia.
  const stepA11y = onStep ? {
    accessibilityRole: 'adjustable' as const,
    accessibilityValue: counter ? { text: counter } : undefined,
    accessibilityActions: [{ name: 'increment' }, { name: 'decrement' }],
    onAccessibilityAction: (ev: AccessibilityActionEvent) => {
      if (ev.nativeEvent.actionName === 'increment') onStep(1);
      else if (ev.nativeEvent.actionName === 'decrement') onStep(-1);
    },
  } : { accessibilityValue: counter ? { text: counter } : undefined };

  return (
    <View style={StyleSheet.absoluteFill}>
      <Animated.View style={[StyleSheet.absoluteFill, styles.backdrop, backdropStyle]} />
      <Animated.View style={[styles.top, { paddingTop: topo }, topStyle]}>
        <Pressable onPress={handleClose} style={styles.roundBtn} hitSlop={2} accessibilityLabel={m.anexos_fechar_imagem()} accessibilityRole="button">
          <Icon name="X" size={20} color={theme.ink.onAccent} />
        </Pressable>
        <Text style={styles.counter} numberOfLines={1} accessibilityLiveRegion="polite">{counter ?? ''}</Text>
        {canShareFile ? (
          <Pressable onPress={share} disabled={sharing} style={styles.roundBtn} hitSlop={2}
            accessibilityLabel={m.gallery_share_named({ nome: filename })} accessibilityRole="button" accessibilityState={{ busy: sharing }}>
            {sharing ? <ActivityIndicator size="small" color={theme.ink.onAccent} /> : <Icon name="Share" size={20} color={theme.ink.onAccent} />}
          </Pressable>
        ) : <View style={styles.roundSpacer} />}
      </Animated.View>
      <View style={styles.stage}>
        {kind === 'video' ? (
          <Animated.View style={[styles.videoWrap, animatedStyle]}>
            <VideoPage uri={uri} headers={headers} name={filename} />
            {/* O WebView fica com os toques do player: passar e fechar moram nesta faixa, fora dele. */}
            <View style={styles.videoBar}>
              {onStep ? (
                <Pressable onPress={() => onStep(-1)} style={styles.roundBtn} accessibilityRole="button" accessibilityLabel={m.visor_anterior()}>
                  <Icon name="ChevronLeft" size={20} color={theme.ink.onAccent} />
                </Pressable>
              ) : null}
              <GestureDetector gesture={pan}>
                <View
                  style={styles.grip}
                  accessible
                  accessibilityLabel={filename}
                  accessibilityHint={onStep ? m.gallery_video_swipe_hint() : m.gallery_video_close_hint()}
                  {...stepA11y}
                >
                  <View style={styles.gripBar} />
                </View>
              </GestureDetector>
              {onStep ? (
                <Pressable onPress={() => onStep(1)} style={styles.roundBtn} accessibilityRole="button" accessibilityLabel={m.visor_proxima()}>
                  <Icon name="ChevronRight" size={20} color={theme.ink.onAccent} />
                </Pressable>
              ) : null}
            </View>
          </Animated.View>
        ) : (
          <GestureDetector gesture={composed}>
            <Animated.View
              style={[styles.imgWrap, animatedStyle]}
              onLayout={(e) => { boxW.value = e.nativeEvent.layout.width; boxH.value = e.nativeEvent.layout.height; }}
              accessible
              accessibilityLabel={filename}
              accessibilityRole="image"
              {...stepA11y}
            >
              {/* `recyclingKey`: a imagem anterior some na troca, em vez de aparecer com o nome da próxima. */}
              <Image key={attempt} source={{ uri, headers }} recyclingKey={uri}
                style={[styles.img, imageBackground ? { backgroundColor: imageBackground } : null]} contentFit="contain" transition={200}
                onLoad={(e) => { natW.value = e.source.width; natH.value = e.source.height; }}
                onError={() => setFailedUri(uri)} />
            </Animated.View>
          </GestureDetector>
        )}
        {kind !== 'video' && imageFailed ? (
          // Fora do detector de gestos: o toque no botão não vira arrasto, e o arrasto em volta continua passando.
          <View style={[StyleSheet.absoluteFill, styles.imageError]} pointerEvents="box-none">
            <Text style={styles.videoMsg} accessibilityRole="alert">{m.arquivo_carregar_erro()}</Text>
            <Pressable onPress={retryImage} style={styles.retryBtn} hitSlop={6} accessibilityRole="button"
              accessibilityLabel={`${m.lista_tentar_novamente()}: ${filename}`}>
              <Text style={styles.videoMsg}>{m.lista_tentar_novamente()}</Text>
            </Pressable>
          </View>
        ) : null}
      </View>
      <Animated.View style={[styles.bottom, { paddingBottom: Math.max(insets.bottom, theme.base.space[4]) }, bottomStyle]}>
        <LightboxCaption filename={filename} details={details} />
        <LightboxActions actions={actions} />
        {strip && strip.items.length > 1 ? <Strip items={strip.items} index={strip.index} onSelect={strip.onSelect} /> : null}
      </Animated.View>
    </View>
  );
}

// Passo e recuo da faixa iguais ao `gap` e ao `paddingHorizontal` do estilo `strip`.
const STEP = THUMB_W + SPACE[1];
const STRIP_INSET = SPACE[3];

// Faixa virtualizada: uma conversa longa tem centenas de mídias e só as visíveis montam.
function Strip({ items, index, onSelect }: { items: LightboxStripItem[]; index: number; onSelect: (i: number) => void }) {
  const list = useRef<FlatList<LightboxStripItem>>(null);
  useEffect(() => {
    if (index < 0 || index >= items.length) return;
    try { list.current?.scrollToIndex({ index, animated: true, viewPosition: 0.5 }); } catch { /* ainda sem layout */ }
  }, [index, items.length]);
  return (
    <FlatList
      ref={list}
      horizontal
      data={items}
      keyExtractor={(it) => it.key}
      showsHorizontalScrollIndicator={false}
      contentContainerStyle={styles.strip}
      getItemLayout={(_d, i) => ({ length: STEP, offset: STRIP_INSET + STEP * i, index: i })}
      initialScrollIndex={Math.max(0, Math.min(index, items.length - 1))}
      initialNumToRender={12}
      windowSize={5}
      extraData={index}
      renderItem={({ item, index: i }) => <StripThumb item={item} active={i === index} onPress={() => onSelect(i)} />}
    />
  );
}

const StripThumb = memo(function StripThumb({ item, active, onPress }: { item: LightboxStripItem; active: boolean; onPress: () => void }) {
  return (
    <StripThumbFrame active={active} kind={item.kind} onPress={onPress}
      label={item.kind === 'video' ? m.anexos_tocar({ nome: item.name }) : m.anexos_ver({ n: item.name })}>
      <Image source={{ uri: item.uri, headers: item.headers }} style={styles.thumbImg} contentFit="cover" transition={100} />
    </StripThumbFrame>
  );
});

// Vídeo toca no WebView (o player nativo do sistema dentro dele): o app não tem expo-video. O do
// servidor vai pro cache antes, porque o player pede o arquivo aos pedaços sem o header do token.
function VideoPage({ uri, headers, name }: { uri: string; headers?: Record<string, string>; name: string }) {
  const comToken = !!headers && Object.keys(headers).length > 0;
  const [src, setSrc] = useState<string | null>(comToken ? null : uri);
  const [erro, setErro] = useState('');
  useEffect(() => {
    // Toda troca de vídeo começa sem a falha do anterior, também o remoto (sem token).
    setErro('');
    if (!comToken) {
      setSrc(uri);
      return;
    }
    let vivo = true;
    setSrc(null);
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
    backgroundColor: '#000',
  },
  top: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    gap: theme.base.space[3],
    paddingHorizontal: theme.base.space[3],
    paddingBottom: theme.base.space[2],
    zIndex: 2,
  },
  counter: {
    flex: 1,
    textAlign: 'center',
    color: theme.ink.onAccent,
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
    fontVariant: ['tabular-nums'],
  },
  roundBtn: {
    width: 44,
    height: 44,
    borderRadius: theme.base.radius.full,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: 'rgba(255,255,255,0.12)',
  },
  roundSpacer: {
    width: 44,
    height: 44,
  },
  stage: {
    flex: 1,
    overflow: 'hidden',
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
    flex: 1,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[2],
  },
  videoBar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingTop: theme.base.space[2],
  },
  grip: {
    flex: 1,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
  gripBar: {
    width: 40,
    height: 4,
    borderRadius: theme.base.radius.full,
    backgroundColor: 'rgba(255,255,255,0.4)',
  },
  video: {
    flex: 1,
    backgroundColor: '#000',
  },
  videoLoading: {
    alignItems: 'center',
    gap: theme.base.space[2],
  },
  imageError: {
    alignItems: 'center',
    justifyContent: 'center',
    gap: theme.base.space[3],
  },
  retryBtn: {
    minHeight: 44,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[4],
    borderRadius: theme.base.radius.full,
    backgroundColor: 'rgba(255,255,255,0.12)',
  },
  videoMsg: {
    color: theme.ink.onAccent,
    textAlign: 'center',
    fontSize: theme.base.text.sm,
  },
  bottom: {
    gap: theme.base.space[1],
    zIndex: 2,
  },
  strip: {
    gap: theme.base.space[1],
    paddingHorizontal: theme.base.space[3],
    paddingTop: theme.base.space[1],
  },
  thumbImg: {
    width: '100%',
    height: '100%',
  },
}));
