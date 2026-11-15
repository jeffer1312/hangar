import { useEffect, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router/react-navigation';
import Animated, { FadeIn, FadeOut, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { ApiErrorBox, IgnoredLine, NoticeBody, NoticeLineView } from '../../chat/ClaudeNotice';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// O hook falha de novo e o total sobe; a linha abre com o Ignorar; ignorado, fica o Desfazer.
const AGAIN = 1.5;
const OPEN = 3;
const IGNORED = 5;
const STEPS = [AGAIN, OPEN, IGNORED];
const noop = () => {};

export function ClaudeNoticesAnimation({ width }: { width: number }) {
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
  // Parada, mostra a linha aberta: é o quadro que explica o Ignorar.
  const t = useScriptStep(STEPS, 7, OPEN, still);
  const open = t >= OPEN && t < IGNORED;
  const ignored = t >= IGNORED;
  const count = m.notice_count({ count: t >= AGAIN ? '4' : '3' });
  const step = ignored ? 'ignored' : open ? 'open' : 'closed';
  return <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
    <Animated.View key={step} entering={still ? undefined : FadeIn.duration(240)} exiting={still ? undefined : FadeOut.duration(150)}>
      {ignored ? <IgnoredLine /> : <NoticeLineView title={m.notice_hook_failed_line({ name: 'PostToolUse:Bash' })} count={count} open={open} />}
      {open ? <NoticeBody detail={m.native_news_notice_sample_detail()} onIgnore={noop} /> : null}
    </Animated.View>
    <ApiErrorBox text={m.native_news_notice_sample_api_error()} />
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  stage: { minHeight: 260, gap: theme.base.space[3], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
}));
