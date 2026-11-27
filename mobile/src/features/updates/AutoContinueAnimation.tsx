import { useEffect, useMemo, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router/react-navigation';
import Animated, { FadeIn, FadeOut, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { AggSession, ChatEvent } from '@hangar/core';
import { ClaudeNotice } from '../../chat/ClaudeNotice';
import { SessionRow } from '../sessions/SessionRow';
import { SessionSectionCard } from '../sessions/SessionSection';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `continuar-depois-da-queda`: a conexão cai no meio do turno e a nota espera a rede, com o Cancelar;
// quando ela volta, a mesma nota diz a hora em que continuou e a sessão volta a trabalhar.
const CYCLE = 6.5;
const DROP = 0.9;
const BACK = 3.6;
const STEPS = [DROP, BACK];
// Movimento reduzido: o desfecho, que diz a queda e a hora em que continuou.
const STILL = 4.5;
const SERVER = 'news';

const noop = () => {};

/** Horário de hoje no relógio do aparelho, em segundos: a nota mostra HH:MM. */
function today(hour: number, minute: number): number {
  const at = new Date();
  at.setHours(hour, minute, 0, 0);
  return at.getTime() / 1000;
}

function note(state: 'waiting' | 'continued'): ChatEvent {
  return { kind: 'notice', id: `news-auto-continue-${state}`, text: '',
    notice_meta: { kind: 'auto_continue', state, stopped_at: today(14, 2), ...(state === 'continued' ? { continued_at: today(14, 5) } : {}) } };
}

export function AutoContinueAnimation({ width }: { width: number }) {
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
  const t = useScriptStep(STEPS, CYCLE, STILL, still);
  const step = t >= BACK ? 'continued' : t >= DROP ? 'waiting' : 'working';
  // Enquanto espera a rede o turno está parado; antes da queda e depois da volta, trabalhando.
  const working = step !== 'waiting';
  const session = useMemo((): AggSession => ({
    serverId: SERVER, serverLabel: '', serverColor: '', provider: 'claude', tracked: true, branch: 'main', name: m.news_tags_sample_session(),
    last_activity: Date.now() / 1000, ...(working ? { state: 'working' as const, label: m.news_codex_agent_task() } : { state: 'idle' as const }),
  }), [working]);
  const event = useMemo(() => (step === 'working' ? null : note(step)), [step]);
  return <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
    <SessionSectionCard index={0} total={1}>
      <SessionRow session={session} compact mostrarServidor={false} onPress={noop} />
    </SessionSectionCard>
    {event ? <Animated.View key={step} entering={still ? undefined : FadeIn.duration(240)} exiting={still ? undefined : FadeOut.duration(150)}>
      <ClaudeNotice event={event} serverId={SERVER} sessionName={session.name} />
    </Animated.View> : null}
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  // Altura reservada para o quadro com a nota: o cartão não pula entre os passos.
  stage: { minHeight: 150, gap: theme.base.space[3], padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
}));
