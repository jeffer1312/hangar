import { useEffect, useMemo, useState } from 'react';
import { AccessibilityInfo, Keyboard, View, useWindowDimensions } from 'react-native';
import Animated, { FadeIn, FadeOut, useReducedMotion } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { nextTip, runStep, type TourStep } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { planTour, setTipsOff, syncTour, takeReplay, tipSeen, tourChoice, useTour } from './store';
import { anchorNode, useAnchors } from './anchors';
import { TourBalloon, tourText, type BalloonAction } from './TourBalloon';
import * as m from '../../paraglide/messages';

type Target = { kind: 'step'; step: TourStep; n: number; total: number } | { kind: 'tip'; step: TourStep };
type Rect = { x: number; y: number; width: number; height: number };

const MARGIN = 16;
const GAP = 12;
const RING = 4;
const MAX_WIDTH = 320;

/** Planeja o tour e escolhe o balão da vez; o desenho só monta quando há balão com a peça na tela. */
export function TourHost({ enabled }: { enabled: boolean }) {
  const serverKey = useServers((s) => s.servers.map((x) => `${x.id}:${x.baseUrl}`).join('|'));
  const listRead = useSessions((s) => s.byServer.some((b) => b.loaded));
  const count = useSessions((s) => s.rows.filter((r) => r.state !== 'dead').length);
  const synced = useTour((s) => s.synced);
  const planned = useTour((s) => s.planned);
  const run = useTour((s) => s.run);
  const tips = useTour((s) => s.tips);
  const onScreen = useAnchors((s) => s.onScreen);
  const progress = useTour((s) => s.progress);

  useEffect(() => { if (enabled) void syncTour(); }, [enabled, serverKey]);
  useEffect(() => {
    if (enabled && synced && listRead && !planned) planTour(count, takeReplay());
  }, [enabled, synced, listRead, planned, count]);

  const target = useMemo<Target | null>(() => {
    if (!planned) return null;
    const step = run ? runStep(run) : null;
    // Passo pendente fora da tela espera a peça aparecer; enquanto isso nenhuma dica o atropela.
    if (step && run?.index != null) return onScreen.includes(step.anchor) ? { kind: 'step', step, n: run.index + 1, total: run.steps.length } : null;
    const tip = nextTip(tips, new Set(onScreen), progress);
    return tip ? { kind: 'tip', step: tip } : null;
  }, [planned, run, tips, onScreen, progress]);

  if (!enabled || !target) return null;
  return <TourLayer key={`${target.kind}:${target.step.id}`} target={target} />;
}

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));

function TourLayer({ target }: { target: Target }) {
  const { theme } = useUnistyles();
  const insets = useSafeAreaInsets();
  const reduced = useReducedMotion();
  const { width: W, height: H } = useWindowDimensions();
  const moved = useAnchors((s) => s.moved);
  const [rect, setRect] = useState<Rect | null>(null);
  const [height, setHeight] = useState(0);
  const anchor = target.step.anchor;

  useEffect(() => {
    const node = anchorNode(anchor);
    if (!node) { setRect(null); return; }
    const measure = () => node.measureInWindow((x, y, width, h) => setRect(width > 0 && h > 0 ? { x, y, width, height: h } : null));
    measure();
    // A peça sobe e desce com o teclado.
    const subs = [Keyboard.addListener('keyboardDidShow', measure), Keyboard.addListener('keyboardDidHide', measure)];
    return () => subs.forEach((s) => s.remove());
  }, [anchor, moved, W, H]);

  const { title, body } = tourText(target.step.id);
  useEffect(() => { AccessibilityInfo.announceForAccessibility(`${title}. ${body}`); }, [title, body]);

  if (!rect) return null;
  const hole = { x: rect.x - RING, y: rect.y - RING, w: rect.width + RING * 2, h: rect.height + RING * 2 };
  const width = Math.min(MAX_WIDTH, W - MARGIN * 2);
  const left = clamp(hole.x + hole.w / 2 - width / 2, MARGIN, W - MARGIN - width);
  const room = H - insets.bottom - (hole.y + hole.h) - GAP;
  const below = room >= (height || 200) || hole.y + hole.h / 2 < H / 2;
  const top = below ? hole.y + hole.h + GAP : Math.max(insets.top + GAP, hole.y - GAP - height);
  const arrowX = clamp(hole.x + hole.w / 2 - left - 7, MARGIN, width - MARGIN - 14);

  const actions: BalloonAction[] = target.kind === 'step'
    ? [
      { label: target.n === target.total ? m.mobile_tour_done() : m.native_tour_skip(), primary: true, onPress: () => tourChoice('next') },
      { label: m.native_tour_skip_all(), onPress: () => tourChoice('skipTour') },
      { label: m.native_tour_later(), onPress: () => tourChoice('later') },
    ]
    : [
      { label: m.native_tour_tip_ok(), primary: true, onPress: () => tipSeen(target.step.id) },
      { label: m.native_tour_tips_off(), onPress: () => { tipSeen(target.step.id); setTipsOff(true); } },
    ];
  const shade = (style: object) => <View pointerEvents="none" style={[styles.shade, style]} />;

  // Escurece em volta da peça sem cobri-la: o toque nela é a ação que faz o tour andar.
  return <View style={StyleSheet.absoluteFill} pointerEvents="box-none">
    {shade({ top: 0, left: 0, width: W, height: Math.max(0, hole.y) })}
    {shade({ top: hole.y + hole.h, left: 0, width: W, height: Math.max(0, H - hole.y - hole.h) })}
    {shade({ top: hole.y, left: 0, width: Math.max(0, hole.x), height: hole.h })}
    {shade({ top: hole.y, left: hole.x + hole.w, width: Math.max(0, W - hole.x - hole.w), height: hole.h })}
    <View pointerEvents="none" style={[styles.ring, { top: hole.y, left: hole.x, width: hole.w, height: hole.h, borderColor: theme.tokens.accent.base }]} />
    <Animated.View entering={reduced ? undefined : FadeIn.duration(180)} exiting={reduced ? undefined : FadeOut.duration(120)}
      style={[styles.place, { top, left, width, opacity: height ? 1 : 0 }]}
      onLayout={(e) => setHeight(e.nativeEvent.layout.height)}>
      <TourBalloon tip={target.kind === 'tip'} title={title} body={body} waiting={target.kind === 'step'}
        tag={target.kind === 'tip' ? m.native_tour_tip_tag() : m.native_tour_progress({ n: target.n, total: target.total })}
        arrow={below ? 'up' : 'down'} arrowX={arrowX} actions={actions} />
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  shade: { position: 'absolute', backgroundColor: 'rgba(0,0,0,0.45)' },
  ring: { position: 'absolute', borderWidth: 2, borderRadius: theme.base.radius.md },
  place: { position: 'absolute' },
}));
