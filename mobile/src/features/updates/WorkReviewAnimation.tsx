import { useContext, useEffect, useMemo, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import Animated, { FadeIn, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { WorkReview, WorkReviewChoices, WorkReviewDecision, WorkReviewSide } from '@hangar/core';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { DecisionCard } from '../workReview/DecisionCard';
import { WorkReviewChip } from '../workReview/WorkReviewChip';
import { HeadlineBand } from '../workReview/WorkReviewSection';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// "Igualar ao mock" escolhido na primeira e na segunda decisão; o cartão passa à seguinte depois do respiro da tela real.
const PICKS = [1.6, 4];
const NEXTS = [2.4, 4.8];
const STEPS = [...PICKS, ...NEXTS];
const CYCLE = 7;
const TOTAL = 3;
// Captura desenhada: o trecho de cada lado é medido nela, como o do conector.
const SHOT = { width: 1600, height: 900 };
const noop = () => {};

type Box = [number, number, number, number];
const SAMPLES: { title: () => string; left: [Box, string, string]; right: [Box, string, string] }[] = [
  { title: m.native_news_work_review_sample_title_1, left: [[560, 380, 480, 140], '#4472c4', '~480 px'], right: [[600, 380, 400, 140], '#4472c4', '~400 px'] },
  { title: m.native_news_work_review_sample_title_2, left: [[520, 300, 560, 120], '#2f6fde', '#2f6fde'], right: [[520, 300, 560, 120], '#c8783c', '#c8783c'] },
  { title: m.native_news_work_review_sample_title_3, left: [[480, 320, 640, 200], '#5a9e6f', '24 px'], right: [[480, 300, 640, 240], '#5a9e6f', '40 px'] },
];
const COLORS = new Map(SAMPLES.flatMap((s, n) => [[`left-${n}`, s.left[1]], [`right-${n}`, s.right[1]]]));

function side(asset: string, label: string, [box, , value]: [Box, string, string]): WorkReviewSide {
  return { label, value, asset, width: SHOT.width, height: SHOT.height, box };
}

function decision(n: number): WorkReviewDecision {
  const sample = SAMPLES[n]!;
  return {
    id: String(n), tag: m.native_news_work_review_sample_tag({ n: n + 1 }), title: sample.title(),
    reason: m.native_news_work_review_sample_reason(), reason_label: m.native_news_work_review_sample_reason_label(),
    left: side(`left-${n}`, m.native_news_work_review_sample_left(), sample.left),
    right: side(`right-${n}`, m.native_news_work_review_sample_right(), sample.right),
    choices: [
      { id: 'match', label: m.native_news_work_review_sample_match(), hint: m.native_news_work_review_sample_match_hint() },
      { id: 'keep', label: m.native_news_work_review_sample_keep(), hint: m.native_news_work_review_sample_keep_hint(), requires_reason: true },
    ],
  };
}

/** A captura desenhada: fundo da tela, duas linhas de conteúdo e o elemento da decisão no trecho, na cor do lado. */
function Shot({ side: { asset, box } }: { side: WorkReviewSide }) {
  const [x, y, w, h] = box ?? [0, 0, 0, 0];
  const pct = (v: number, of: number) => `${(v / of) * 100}%` as const;
  return (
    <View style={styles.shot}>
      <View style={[styles.line, { top: '12%', width: '40%' }]} />
      <View style={[styles.line, { top: '20%', width: '26%' }]} />
      <View style={[styles.element, { left: pct(x, SHOT.width), top: pct(y, SHOT.height), width: pct(w, SHOT.width), height: pct(h, SHOT.height),
        backgroundColor: COLORS.get(asset) }]} />
    </View>
  );
}

const drawing = (s: WorkReviewSide) => <Shot side={s} />;
const asset = (id: string) => ({ uri: id, headers: {} });

export function WorkReviewAnimation({ width }: { width: number }) {
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
  // Parada, mostra a primeira decisão já escolhida e a conta baixando: é o quadro que explica a revisão.
  const t = useScriptStep(STEPS, CYCLE, PICKS[0]!, still);
  const picked = PICKS.filter((p) => t >= p).length;
  const index = NEXTS.filter((n) => t >= n).length;
  const pending = TOTAL - picked;
  const review = useMemo((): WorkReview => ({
    id: 'news', title: '', updated_at: '', state: 'waiting', links: [],
    headline: { value: String(TOTAL), text: m.work_review_waiting(), detail: m.native_news_work_review_sample_detail() },
    decisions: Array.from({ length: TOTAL }, (_, n) => decision(n)),
  }), []);
  const choices = useMemo((): WorkReviewChoices =>
    Object.fromEntries(Array.from({ length: picked }, (_, n) => [String(n), { choice: 'match', reason: '' }])), [picked]);
  const enter = still ? undefined : FadeIn.duration(240);
  return (
    <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
      <Animated.View key={`chip-${pending}`} entering={enter} style={styles.chip}>
        <WorkReviewChip badge={{ text: m.native_news_work_review_sample_badge({ count: pending }), tone: 'warning' }} />
      </Animated.View>
      <Animated.View key={`band-${pending}`} entering={enter}><HeadlineBand review={review} pending={pending} /></Animated.View>
      <Animated.View key={`card-${index}`} entering={enter}>
        <DecisionCard review={review} choices={choices} index={index} sending={false} sendError={null} asset={asset} drawing={drawing}
          onIndex={noop} onPick={noop} onReason={noop} onSend={noop} onCompare={noop} onAssetError={noop} />
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  stage: { gap: theme.base.space[3], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
  chip: { alignSelf: 'flex-start' },
  shot: { flex: 1, backgroundColor: '#b8bac0' },
  line: { position: 'absolute', left: '6%', height: '3%', borderRadius: 2, backgroundColor: '#9a9ca3' },
  element: { position: 'absolute', borderRadius: 3 },
}));
