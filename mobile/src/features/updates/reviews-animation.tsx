import { useContext, useEffect, useMemo, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import Animated, { useAnimatedStyle, useReducedMotion, withTiming } from 'react-native-reanimated';
import { type ReviewSummary, type ReviewThread } from '@hangar/core';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { ReviewRowView } from '../reviews/review-ui';
import { ReviewThreadView } from '../reviews/review-thread-view';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

const STEPS = [2, 3.5];
const noop = () => {};

export function ReviewsAnimation({ width }: { width: number }) {
  const paused = useContext(WorkingMarkPaused);
  const reduced = useReducedMotion();
  const focused = useIsFocused();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const [inView, setInView] = useState(false);
  const [active, setActive] = useState(AppState.currentState === 'active');
  useEffect(() => { const subscription = AppState.addEventListener('change', (state) => setActive(state === 'active')); return () => subscription.remove(); }, []);
  useEffect(() => {
    if (!focused || !active || paused) return;
    const check = () => stage.current?.measureInWindow((_x, y, _width, height) => setInView(height > 0 && y < window.height && y + height > 0));
    check();
    const timer = setInterval(check, 500);
    return () => clearInterval(timer);
  }, [focused, active, paused, window.height]);
  const still = reduced || paused || !active || !focused || !inView;
  const time = useScriptStep(STEPS, 7, 4, still);
  const passed = time >= 2;
  const visible = time >= 3.5;
  const motion = useAnimatedStyle(() => ({ opacity: still ? Number(visible) : withTiming(Number(visible), { duration: 200 }) }), [visible, still]);
  const review = useMemo((): ReviewSummary => ({ host: '', repo: 'equipe/aplicativo', number: 42,
    provider: 'gitlab', url: '', title: m.native_news_review_sample_title(), author: 'ana', mine: true, review_requested: false,
    draft: false, state: 'open', source: 'exportacao', source_repo: 'equipe/aplicativo', target: 'main',
    pipeline: passed ? 'success' : 'running', open_threads: 1, conflicts: false, sessions: [] }), [passed]);
  const thread = useMemo((): ReviewThread => ({ id: 'sample', file: 'src/export.ts', line: 42, resolved: false, code: '',
    comments: [{ author: 'ana', body: m.native_news_review_sample_comment(), at: '', bot: false }] }), []);
  return <View ref={stage} style={{ width }} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <ReviewRowView review={review} machine={m.mobile_review_news_machine()} />
    <Animated.View style={motion}><ReviewThreadView thread={thread} serverId="news" busy={false} onReply={noop} onResolve={noop} onCopy={noop} /></Animated.View>
  </View>;
}
