import { useContext, useEffect, useRef, useState } from 'react';
import { AppState, Text, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import Animated, { FadeIn, useAnimatedStyle, useReducedMotion, useSharedValue, withTiming } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { LinearGradient } from 'expo-linear-gradient';
import type { AttachmentOrigin } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { Icon } from '../../ui/Icon';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { SPACE } from '../../theme/scale';
import { LightboxActions, LightboxCaption, StripThumbFrame } from '../attachments/LightboxParts';
import { MediaCellFrame, MediaTabs, OriginChips } from '../attachments/MediaFilesParts';
import { originLabel } from '../attachments/origin';
import { useScriptStep } from './scriptStep';

// Roteiro da `galeria-da-conversa`: o visor passa de lado pela mídia da conversa inteira, com a faixa embaixo e o
// "Na conversa"; depois a tela Mídia e arquivos mostra tudo em grade e filtra só o que o agente mandou.
const CYCLE = 6;
const NEXT = [0.9, 1.8, 2.7];
const GRID = 3.5;
const FILTER = 4.8;
const STEPS = [...NEXT, GRID, FILTER];
// Movimento reduzido: o visor parado na terceira mídia, com a faixa e o "Na conversa" à vista.
const STILL = 2;
const COLS = 3;
const IMAGE_H = 150;
// As duas cenas cabem nesta altura: a troca não mexe no tamanho do cartão.
const STAGE_H = 400;
const noop = () => {};

type Sample = { name: () => string; kind: 'image' | 'video'; origin: AttachmentOrigin; colors: [string, string] };
const SAMPLES: Sample[] = [
  { name: () => m.news_gallery_sample_1(), kind: 'image', origin: 'agent', colors: ['#5b6cff', '#3fb6a8'] },
  { name: () => m.news_gallery_sample_2(), kind: 'image', origin: 'user', colors: ['#f2a65a', '#e5484d'] },
  { name: () => m.news_gallery_sample_3(), kind: 'image', origin: 'agent', colors: ['#30a46c', '#8bd5c8'] },
  { name: () => m.news_gallery_sample_4(), kind: 'video', origin: 'agent', colors: ['#8e6cf0', '#3e63dd'] },
  { name: () => m.news_gallery_sample_5(), kind: 'image', origin: 'user', colors: ['#f76b15', '#ffd166'] },
  { name: () => m.news_gallery_sample_6(), kind: 'image', origin: 'agent', colors: ['#0f766e', '#93c5fd'] },
];

/** Mídia que o visor mostra no instante `t`; na grade fica a última. */
export function viewerIndex(t: number): number {
  return NEXT.filter((s) => t >= s).length;
}

/** Imagem de exemplo desenhada aqui: degradê com um cartão claro no meio, como um print. */
function Picture({ colors }: { colors: [string, string] }) {
  return (
    <LinearGradient colors={colors} start={{ x: 0, y: 0 }} end={{ x: 1, y: 1 }} style={styles.fill}>
      <View style={styles.card} />
    </LinearGradient>
  );
}

export function ConversationGalleryAnimation({ width }: { width: number }) {
  const { theme } = useUnistyles();
  const paused = useContext(WorkingMarkPaused);
  const reduced = useReducedMotion();
  const focused = useIsFocused();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const [inView, setInView] = useState(false);
  const [active, setActive] = useState(AppState.currentState === 'active');
  useEffect(() => { const sub = AppState.addEventListener('change', (state) => setActive(state === 'active')); return () => sub.remove(); }, []);
  useEffect(() => {
    if (!focused || !active || paused) return;
    const check = () => stage.current?.measureInWindow((_x, y, _w, height) => setInView(height > 0 && y < window.height && y + height > 0));
    check();
    const timer = setInterval(check, 500);
    return () => clearInterval(timer);
  }, [focused, active, paused, window.height]);
  const still = reduced || paused || !active || !focused || !inView;
  const t = useScriptStep(STEPS, CYCLE, STILL, still);
  const grid = t >= GRID;
  const agentOnly = t >= FILTER;
  const index = viewerIndex(t);
  const inner = width - SPACE[3] * 2 - 2;

  // O arrasto de lado do visor: a fileira de mídias anda na thread de UI até a aberta.
  const x = useSharedValue(-index * inner);
  useEffect(() => {
    const target = -index * inner;
    x.value = still || index === 0 ? target : withTiming(target, { duration: 320 });
  }, [index, inner, still, x]);
  const track = useAnimatedStyle(() => ({ transform: [{ translateX: x.value }] }));

  const enter = still ? undefined : FadeIn.duration(220);
  const sample = SAMPLES[index];
  const kept = SAMPLES.map((s, i) => ({ s, i })).reverse().filter(({ s }) => !agentOnly || s.origin === 'agent');
  const cell = Math.floor((inner - SPACE[0.5] * (COLS - 1)) / COLS);
  const rows = [kept.slice(0, COLS), kept.slice(COLS, COLS * 2)].filter((r) => r.length);

  return (
    <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants"
      style={[styles.stage, { width, height: STAGE_H }]}>
      {grid ? (
        <Animated.View key="media" entering={enter} style={styles.scene}>
          <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.media_title()}</Text>
          {/* Arquivos e áudio do exemplo: dois arquivos do agente e o áudio do ditado, que é seu. */}
          <MediaTabs counts={{ media: kept.length, files: 2, audio: agentOnly ? 0 : 1 }} selected="media" />
          <OriginChips selected={agentOnly ? 'agent' : 'all'} />
          <View>
            <Text style={[styles.day, { color: theme.tokens.text.muted }]}>{m.media_day_today()}</Text>
            {rows.map((row) => (
              <View key={row[0].i} style={styles.gridRow}>
                {row.map(({ s, i }) => (
                  <MediaCellFrame key={i} size={cell} origin={s.origin} placeholder={s.kind === 'video' ? 'Play' : undefined}>
                    <Picture colors={s.colors} />
                  </MediaCellFrame>
                ))}
              </View>
            ))}
          </View>
        </Animated.View>
      ) : (
        <Animated.View key="viewer" entering={enter} style={[styles.scene, styles.viewer]}>
          <Text style={styles.counter} numberOfLines={1}>{m.gallery_counter({ n: index + 1, total: SAMPLES.length })}</Text>
          <View style={[styles.window, { width: inner, height: IMAGE_H }]}>
            <Animated.View style={[styles.track, { width: inner * SAMPLES.length }, track]}>
              {SAMPLES.map((s, i) => (
                <View key={i} style={{ width: inner, height: IMAGE_H }}>
                  <Picture colors={s.colors} />
                  {s.kind === 'video' ? (
                    <View style={styles.play}>
                      <View style={[styles.playDot, { backgroundColor: theme.ink.scrim }]}>
                        <Icon name="Play" size={24} color={theme.ink.onAccent} />
                      </View>
                    </View>
                  ) : null}
                </View>
              ))}
            </Animated.View>
          </View>
          <LightboxCaption filename={sample.name()} details={`${m.media_when_today({ time: '11:42' })} · ${originLabel(sample.origin)}`} />
          <LightboxActions actions={[
            { icon: 'Download', label: m.gallery_save(), a11y: m.gallery_save(), onPress: noop },
            { icon: 'MessageSquare', label: m.gallery_in_chat(), a11y: m.gallery_in_chat(), onPress: noop },
          ]} />
          <View style={styles.strip}>
            {/* Quatro cabem no cartão mais estreito; a mídia ativa do roteiro nunca passa da quarta. */}
            {SAMPLES.slice(0, 4).map((s, i) => (
              <StripThumbFrame key={i} active={i === index} kind={s.kind}>
                <Picture colors={s.colors} />
              </StripThumbFrame>
            ))}
          </View>
        </Animated.View>
      )}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  stage: {
    padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  scene: { flex: 1, gap: theme.base.space[2] },
  // O fundo do visor, preto como na tela cheia.
  viewer: { backgroundColor: '#000', borderRadius: theme.base.radius.sm, paddingVertical: theme.base.space[2], overflow: 'hidden' },
  title: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  counter: {
    textAlign: 'center', color: theme.ink.onAccent, fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold, fontVariant: ['tabular-nums'],
  },
  window: { overflow: 'hidden' },
  track: { flexDirection: 'row' },
  fill: { width: '100%', height: '100%' },
  card: {
    position: 'absolute', left: '18%', top: '22%', width: '64%', height: '50%',
    borderRadius: theme.base.radius.sm, backgroundColor: 'rgba(255,255,255,0.35)',
  },
  play: { position: 'absolute', top: 0, right: 0, bottom: 0, left: 0, alignItems: 'center', justifyContent: 'center' },
  playDot: { width: 56, height: 56, borderRadius: theme.base.radius.full, alignItems: 'center', justifyContent: 'center' },
  strip: { flexDirection: 'row', gap: theme.base.space[1], paddingHorizontal: theme.base.space[3] },
  day: { fontSize: theme.base.text.xxs, marginTop: theme.base.space[1], marginBottom: theme.base.space[1.5] },
  gridRow: { flexDirection: 'row', gap: theme.base.space[0.5], marginBottom: theme.base.space[0.5] },
}));
