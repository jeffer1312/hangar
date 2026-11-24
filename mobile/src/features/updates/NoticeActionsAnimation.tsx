import { useEffect, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router/react-navigation';
import Animated, { FadeIn, FadeOut, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { NoticeBusy, NoticeLineView, NoticeMark, NoticePill } from '../../chat/NoticeParts';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Os botões são tocados e andam; depois os dois ficam resolvidos e a linha do resultado chega.
const WORKING = 2;
const DONE = 4.5;
const STEPS = [WORKING, DONE];
const PLUGIN = 'fmt@tools';
const CHILD = 'conserto-lint';

export function NoticeActionsAnimation({ width }: { width: number }) {
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
  // Parada, mostra o fim: os dois resolvidos e a linha do resultado.
  const t = useScriptStep(STEPS, 8, DONE, still);
  const step = t >= DONE ? 'done' : t >= WORKING ? 'working' : 'idle';
  const plugin = step === 'done'
    ? <NoticeLineView icon={<NoticeMark ok />} title={m.notice_reinstalled({ name: PLUGIN })} count={null} open={null} action={<NoticePill label={m.notice_reload()} />} />
    : <NoticeLineView title={m.notice_hook_failed_line({ name: 'SessionStart' })} count={null} open={null}
        action={step === 'working' ? <NoticeBusy label={m.notice_reinstalling({ name: PLUGIN })} /> : <NoticePill label={m.notice_reinstall()} />} />;
  const hook = step === 'done'
    ? <NoticeLineView icon={<NoticeMark ok />} title={`${m.notice_hook_failed_line({ name: 'PostToolUse:Bash' })} · ${m.notice_fixed()}`} count={null} open={null} />
    : <NoticeLineView title={m.notice_hook_failed_line({ name: 'PostToolUse:Bash' })} count={null} open={null}
        action={step === 'working' ? <NoticeBusy label={m.notice_fixing({ name: CHILD })} /> : <NoticePill label={m.notice_fix()} />} />;
  return <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
    <Animated.View key={step} entering={still ? undefined : FadeIn.duration(240)} exiting={still ? undefined : FadeOut.duration(150)}>
      {plugin}
      {hook}
      {step === 'done' ? (
        <NoticeLineView icon={<NoticeMark ok />} title={`${CHILD}: ${m.native_news_notice_actions_sample_result()}`} count={null} open={null}
          action={<NoticePill label={m.notice_fix_open()} />} />
      ) : null}
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  stage: { minHeight: 170, gap: theme.base.space[3], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
}));
