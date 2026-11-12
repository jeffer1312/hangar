import { useEffect, useMemo, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router/react-navigation';
import Animated, { FadeIn, FadeOut, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { ScheduledTask } from '@hangar/core';
import { MachineGroup } from '../machines/MachineRows';
import { TaskRow } from '../scheduledTasks/TaskRow';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

const STEPS = [1.5, 3];
const noop = () => {};
export function ScheduledTasksAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const [visible, setVisible] = useState(false);
  const focused = useIsFocused();
  const [active, setActive] = useState(AppState.currentState === 'active');
  useEffect(() => { const sub = AppState.addEventListener('change', (state) => setActive(state === 'active')); return () => sub.remove(); }, []);
  useEffect(() => {
    if (!focused || !active) return;
    const check = () => stage.current?.measureInWindow((_x, y, _w, height) => setVisible(height > 0 && y < window.height && y + height > 0));
    check();
    const timer = setInterval(check, 500);
    return () => clearInterval(timer);
  }, [focused, active, window.height]);
  const still = reduced || !active || !focused || !visible;
  const t = useScriptStep(STEPS, 6, 3, still);
  const sent = t >= 3;
  const enabled = t >= 1.5;
  const task = useMemo<ScheduledTask>(() => ({
    id: 'news', revision: 1, name: m.scheduled_news_name(), prompt: m.scheduled_news_prompt(), enabled,
    schedule: { kind: 'weekly', time: '09:00', weekdays: [1, 2, 3, 4, 5] },
    target: { kind: 'new', launch: {}, worktree: true }, next_run_at: '2026-11-18T09:00:00-03:00', last_run_at: null,
    last_status: sent ? 'sent' : 'never', last_error: null, last_session: sent ? 'news' : null,
  }), [sent, enabled]);
  return <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
    <MachineGroup><Animated.View key={sent ? 'sent' : 'waiting'} entering={still ? undefined : FadeIn.duration(240)} exiting={still ? undefined : FadeOut.duration(150)}>
      <TaskRow task={task} timezone="America/Sao_Paulo" onEdit={noop} onToggle={noop} onRun={noop} onDelete={noop} onOpen={noop} />
    </Animated.View></MachineGroup>
  </View>;
}
const styles = StyleSheet.create((theme) => ({
  stage: { minHeight: 300, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
}));
