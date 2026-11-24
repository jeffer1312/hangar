import { useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import Animated, { useAnimatedStyle, useReducedMotion, withDelay, withSequence, withTiming } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { CheckRow } from '../config/CheckRow';
import { SectionCard } from '../config/SectionCard';
import { LoginField, LoginSubmit } from '../login/LoginFields';
import { LoginHeader } from '../login/LoginHeader';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `entrar-pelo-connect`: a conta se digita letra a letra, a senha entra, o Entrar é tocado e a lista
// "Suas máquinas" aparece com as ligadas já marcadas.
const CYCLE = 6;
const TYPE_START = 0.5;
const TYPE_GAP = 0.18;
const PASSWORD = 1.5;
const PRESS = 2.2;
const LIST = 3;
// Movimento reduzido: a lista com as ligadas marcadas, que é o que a novidade promete.
const STILL = 4.5;
const FADE = { duration: 240 };
const ROW_GAP_MS = 110;
const SAMPLE_PASSWORD = '12345678';
const noop = () => {};

/** Letras digitadas, senha, toque e lista no instante `t` do roteiro. */
export function frameAt(t: number, user: string) {
  const letters = Array.from(user);
  const typed = letters.filter((_, i) => t >= TYPE_START + i * TYPE_GAP).length;
  return { user: letters.slice(0, typed).join(''), password: t >= PASSWORD, pressed: t >= PRESS, list: t >= LIST };
}

export function ConnectSignInAnimation({ width }: { width: number }) {
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
  const sampleUser = m.news_login_sample_user();
  const steps = useMemo(() => [...Array.from(sampleUser).map((_, i) => TYPE_START + i * TYPE_GAP), PASSWORD, PRESS, LIST], [sampleUser]);
  const t = useScriptStep(steps, CYCLE, STILL, still);
  const frame = frameAt(t, sampleUser);
  const { list, pressed } = frame;
  const form = useAnimatedStyle(() => ({ opacity: still ? (list ? 0 : 1) : withTiming(list ? 0 : 1, FADE) }), [list, still]);
  const result = useAnimatedStyle(() => ({ opacity: still ? (list ? 1 : 0) : withTiming(list ? 1 : 0, FADE) }), [list, still]);
  // O toque: o botão afunda e volta, na thread de UI.
  const press = useAnimatedStyle(() => ({
    transform: [{ scale: !still && pressed ? withSequence(withTiming(0.96, { duration: 90 }), withTiming(1, { duration: 160 })) : 1 }],
  }), [pressed, still]);
  const ready = !!frame.user && frame.password;
  const machines = [
    { name: m.news_connect_sample_machine(), on: true },
    { name: m.news_tags_sample_machine(), on: true },
    { name: m.news_auto_report_sample_machine(), on: false },
  ];
  const picked = machines.filter((machine) => machine.on).length;
  return <View ref={stage} style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {/* O formulário fica no layout mesmo invisível: a altura dele não muda de um quadro para outro. */}
    <Animated.View style={[styles.form, form]}>
      <LoginHeader markSize={48} />
      <View style={styles.fields}>
        <LoginField label={m.login_account_or_user()} value={frame.user} placeholder={m.login_account_or_user_hint()} editable={false} />
        <LoginField label={m.login_senha()} value={frame.password ? SAMPLE_PASSWORD : ''} placeholder="••••••••" secureTextEntry editable={false} />
        <Animated.View style={press}>
          <LoginSubmit label={m.login_entrar()} text={pressed ? m.login_entrando() : m.login_entrar()} blocked={pressed || !ready} onPress={noop} />
        </Animated.View>
      </View>
    </Animated.View>
    <Animated.View style={[styles.result, result]}>
      <SectionCard icon="Laptop" title={m.connect_your_machines()} subtitle={m.connect_machines_pull()}>
        {machines.map((machine, i) => <MachineLine key={machine.name} index={i} shown={list} still={still}>
          {machine.on
            ? <CheckRow title={machine.name} dot="on" checked onToggle={noop} />
            : <CheckRow title={machine.name} dot="off" description={m.connect_machine_off_now()} checked={false} disabled onToggle={noop} />}
        </MachineLine>)}
      </SectionCard>
      <LoginSubmit label={m.login_guest_enter_many({ n: String(picked) })} blocked={false} onPress={noop} />
    </Animated.View>
  </View>;
}

/** Cada máquina chega um pouco depois da anterior. */
function MachineLine({ index, shown, still, children }: { index: number; shown: boolean; still: boolean; children: ReactNode }) {
  const style = useAnimatedStyle(() => {
    if (still) return { opacity: shown ? 1 : 0, transform: [{ translateY: shown ? 0 : 6 }] };
    // Saindo, acompanha o esmaecer da lista inteira em vez de sumir antes dela.
    if (!shown) return { opacity: withTiming(0, FADE), transform: [{ translateY: withTiming(6, FADE) }] };
    const delay = 120 + index * ROW_GAP_MS;
    return { opacity: withDelay(delay, withTiming(1, FADE)), transform: [{ translateY: withDelay(delay, withTiming(0, FADE)) }] };
  }, [index, shown, still]);
  return <Animated.View style={style}>{children}</Animated.View>;
}

const styles = StyleSheet.create(theme => ({
  stage: {
    padding: theme.base.space[4], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  form: { gap: theme.base.space[5] },
  fields: { gap: theme.base.space[4] },
  result: {
    position: 'absolute', left: theme.base.space[4], right: theme.base.space[4], top: theme.base.space[4], bottom: theme.base.space[4],
    justifyContent: 'center', gap: theme.base.space[3],
  },
}));
