import { useContext, useEffect, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { TermosPainel } from '../config/AntigravityTermos';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `termos-antigravity`: a folha abre, o agy chega aos Termos com os links e as duas escolhas, e a
// escolha tocada é confirmada no agy.
const TERMS = 1.2;
const ANSWER = 4.2;
const CYCLE = 7;
const STEPS = [TERMS, ANSWER];
// Movimento reduzido: os Termos com as duas escolhas, que é o que a novidade traz.
const STILL = 2;
// Largura de telefone, reduzida ao cartão: o texto dos Termos quebra como na folha de verdade.
const WIDTH = 380;
const noop = () => {};

export type TermosFrame = 'starting' | 'terms' | 'answering';

export function frameAt(t: number): TermosFrame {
  return t < TERMS ? 'starting' : t < ANSWER ? 'terms' : 'answering';
}

export function AntigravityTermsAnimation({ width }: { width: number }) {
  const paused = useContext(WorkingMarkPaused);
  const reduced = useReducedMotion();
  const focused = useIsFocused();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const [inView, setInView] = useState(false);
  const [innerHeight, setInnerHeight] = useState(0);
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
  const frame = frameAt(useScriptStep(STEPS, CYCLE, STILL, still));
  const scale = width / WIDTH;
  const painel = (f: TermosFrame) => (
    <TermosPainel serverLabel={m.native_news_login_browser_laptop()} starting={f === 'starting'}
      terms={f === 'terms' ? m.native_news_agy_terms_sample() : null} answering={f === 'answering'} login={false}
      failed="" error="" cancelDisabled={false} terminal={false}
      onAnswer={noop} onLink={noop} onSignIn={noop} onCancel={noop} onOpenTerminal={noop} />
  );
  return (
    <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants"
      style={[styles.stage, { width, height: innerHeight ? innerHeight * scale : undefined }]}>
      {/* A escala não muda a medida do layout: o cartão recebe a altura medida já reduzida. */}
      <View onLayout={(e) => setInnerHeight(e.nativeEvent.layout.height)} style={[styles.surface, { width: WIDTH, transform: [{ scale }] }]}>
        {/* Os Termos, a etapa mais alta, ficam montados e dão a altura: o cartão não pula entre as etapas. */}
        <View style={frame === 'terms' ? undefined : styles.hidden}>{painel('terms')}</View>
        {frame !== 'terms' ? <View style={styles.over}>{painel(frame)}</View> : null}
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  stage: {
    borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  surface: { transformOrigin: 'top left', padding: theme.base.space[4] },
  hidden: { opacity: 0 },
  over: { position: 'absolute', top: theme.base.space[4], left: theme.base.space[4], right: theme.base.space[4] },
}));
