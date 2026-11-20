import { useContext, useEffect, useRef, useState } from 'react';
import { AppState, Text, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import Animated, { FadeIn, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { useSettingsColors } from '../config/colors';
import { AccountLine, FastRow, InstallItems, Preview, Stepper, type Account } from '../accounts/CliproxyWizard';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Os quatro itens da instalação ficam prontos um a um; depois a conta entra e o motor ganha nome.
const ITEMS = [0.6, 1.2, 1.8, 2.4];
const CONNECT = 3;
const NAME = 4.8;
const STEPS = [...ITEMS, CONNECT, NAME];
const CYCLE = 7;
const MODEL = 'gpt-6.1-sol';

export function CliproxyWizardAnimation({ width }: { width: number }) {
  const c = useSettingsColors();
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
  // Parada, mostra o passo final: o nome que aparece na sessão e o Fast disponível.
  const t = useScriptStep(STEPS, CYCLE, NAME, still);
  const step = t >= NAME ? 3 : t >= CONNECT ? 2 : 1;
  const at = ITEMS.filter((s) => t >= s).length;
  const enter = still ? undefined : FadeIn.duration(240);
  const account: Account = { provider: 'codex', email: m.news_cliproxy_sample_email() };
  const name = `GPT ${account.email.split('@')[0]}`;
  return (
    <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
      <Stepper current={step} allDone={false} />
      <Animated.View key={`step-${step}`} entering={enter} style={styles.body}>
        {step === 1 ? <>
          <Text style={[styles.lead, { color: c.text }]}>{m.mobile_cliproxy_wizard_installing()}</Text>
          <InstallItems at={at} failed={false} />
        </> : step === 2 ? <>
          <Text style={[styles.lead, { color: c.text }]}>{m.mobile_cliproxy_wizard_connected_lead()}</Text>
          <AccountLine account={account} />
        </> : <>
          <AccountLine account={account} />
          <Preview label={m.mobile_cliproxy_wizard_name_preview()} name={name} extra={MODEL} />
          <FastRow value disabled />
        </>}
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  stage: { gap: theme.base.space[2], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
  body: { gap: theme.base.space[3] },
  lead: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
}));
