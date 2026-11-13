import { useEffect, useMemo, useRef, useState } from 'react';
import { AppState, Text, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router/react-navigation';
import Animated, { FadeIn, FadeOut, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { mkMarkdownStyle } from '../../chat/AssistantBubble';
import { Pill } from '../config/PageHeader';
import { SectionCard } from '../config/SectionCard';
import { SettingsMenuGroup } from '../config/SettingsMenu';
import { SettingsRow } from '../config/SettingsRow';
import { useSettingsColors } from '../config/colors';
import { superficie } from '../../theme/superficie';
import { KindPill, UnseenDot } from './DocParts';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// A lista por dia; aos 2,6 s a novidade nova abre a página dela.
const STEPS = [2.6];
const CYCLE = 7;
const noop = () => {};

export function UpdatesManualAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const md = useMemo(() => mkMarkdownStyle(theme), [theme]);
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
  // Parada: a lista por dia com a novidade ainda não vista.
  const still = reduced || !active || !focused || !visible;
  const page = useScriptStep(STEPS, CYCLE, 0, still) >= 2.6;
  return <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
    <Animated.View key={page ? 'page' : 'list'} entering={still ? undefined : FadeIn.duration(240)} exiting={still ? undefined : FadeOut.duration(150)}>
      {page ? <View style={styles.page}>
        <Text style={[styles.date, { color: c.muted }]}>{m.news_guide_sample_date()}</Text>
        <View style={styles.titleLine}>
          <Text style={[styles.title, { color: c.text }]}>{m.news_guide_sample_new()}</Text>
          <KindPill kind="new" />
        </View>
        <EnrichedMarkdownText markdown={m.news_guide_sample_text()} markdownStyle={md} flavor="github" />
        <View style={styles.actions}><Pill label={m.updates_history_read_manual()} icon="BookOpen" onPress={noop} /></View>
      </View> : <View>
        <SettingsMenuGroup title={m.news_guide_sample_day()} />
        <SectionCard>
          <SettingsRow title={m.news_guide_sample_new()} titleBefore={<UnseenDot />} titleAfter={<KindPill kind="new" />} onPress={noop} />
          <SettingsRow title={m.news_guide_sample_improvement()} titleAfter={<KindPill kind="improvement" />} />
          <SettingsRow title={m.news_guide_sample_fix()} titleAfter={<KindPill kind="fix" />} />
        </SectionCard>
      </View>}
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  stage: { minHeight: 300, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
  page: { gap: theme.base.space[2], paddingHorizontal: theme.base.space[1] },
  date: { fontSize: theme.base.text.xs },
  titleLine: { flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: theme.base.space[2] },
  title: { flexShrink: 1, fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold },
  actions: { flexDirection: 'row' },
}));
