import { useContext, useEffect, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import Animated, { FadeOut, LinearTransition, SlideInDown, SlideOutDown, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { CheckRow } from '../config/CheckRow';
import { SectionCard } from '../config/SectionCard';
import { useSettingsColors } from '../config/colors';
import { MachineRow } from '../machines/MachineRows';
import { MachineSheetHeading } from '../machines/parts';
import { ComputerDetail } from '../connect/ComputerDetail';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `connect-remover-computador`: o toque no computador fora do ar abre o detalhe dele, Remover da conta é
// tocado e o computador sai da lista "Seus computadores".
const CYCLE = 6;
const TAP = 0.8;
const DETAIL = 1.1;
const PRESS = 2.8;
const CLOSE = 3.8;
const STEPS = [TAP, DETAIL, PRESS, CLOSE];
// Parada: o detalhe aberto com Remover da conta à vista.
const STILL = 2;
const noop = () => {};

/** Linha tocada, detalhe aberto, remoção em andamento e computador já fora da lista no instante `t`. */
export function frameAt(t: number) {
  return { tapped: t >= TAP && t < DETAIL, detail: t >= DETAIL && t < CLOSE, removing: t >= PRESS && t < CLOSE, gone: t >= CLOSE };
}

export function ConnectRemoveComputerAnimation({ width }: { width: number }) {
  const paused = useContext(WorkingMarkPaused);
  const reduced = useReducedMotion();
  const focused = useIsFocused();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const { theme } = useUnistyles();
  const c = useSettingsColors();
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
  const frame = frameAt(t);
  const target = m.news_auto_report_sample_machine();
  const layout = still ? undefined : LinearTransition.duration(260);
  return <View ref={stage} style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <SectionCard icon="Laptop" title={m.connect_computers_title()} subtitle={m.connect_computers_help({ account: m.news_login_sample_user() })}>
      {/* As três linhas como a tela as escolhe: a que já está no app, a que vem marcada para entrar e a fora do ar. */}
      <Animated.View layout={layout}>
        <MachineRow title={m.news_connect_sample_machine()} value={m.connect_machine_added()} valueTone="success" onPress={noop} />
      </Animated.View>
      <Animated.View layout={layout}>
        <CheckRow title={m.news_tags_sample_machine()} dot="on" checked onToggle={noop} />
      </Animated.View>
      {frame.gone ? null : <Animated.View layout={layout} exiting={still ? undefined : FadeOut.duration(220)}
        style={frame.tapped && { backgroundColor: c.hover }}>
        <MachineRow title={target} description={m.connect_machine_off_now()} onPress={noop} />
      </Animated.View>}
    </SectionCard>
    {/* O detalhe sobe por cima da lista como a folha sobe na tela. */}
    {frame.detail ? <Animated.View entering={still ? undefined : SlideInDown.duration(280)} exiting={still ? undefined : SlideOutDown.duration(240)}
      style={[styles.sheet, { backgroundColor: `rgba(${theme.tokens.glass.panelRgb.join(',')},0.97)`, borderColor: c.border }]}>
      <MachineSheetHeading title={target} onClose={noop} />
      <ComputerDetail online={false} inApp={false} next={null} busy={frame.removing} canEnter={false} removeDisabled={frame.removing}
        onOpen={noop} onEnter={noop} onRemove={noop} />
    </Animated.View> : null}
  </View>;
}

const styles = StyleSheet.create(theme => ({
  // Altura reservada para o detalhe aberto: o cartão não pula entre os passos.
  stage: {
    minHeight: 340, padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  sheet: {
    position: 'absolute', left: 0, right: 0, bottom: 0, top: theme.base.space[8], gap: theme.base.space[3], padding: theme.base.space[3],
    borderTopLeftRadius: theme.base.radius.lg, borderTopRightRadius: theme.base.radius.lg, borderWidth: 1,
  },
}));
