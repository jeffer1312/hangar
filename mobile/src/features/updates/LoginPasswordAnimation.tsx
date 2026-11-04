import { View } from 'react-native';
import Animated, { useAnimatedStyle, useReducedMotion, withTiming } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { LoginField, LoginSubmit } from '../login/LoginFields';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `login-usuario-senha`: usuário e senha são preenchidos, o Entrar é apertado e as máquinas da conta aparecem.
const CYCLE = 6.5;
const USER = 0.5;
const PASSWORD = 1.3;
const PRESS = 2.2;
const RESULT = 3.4;
const STEPS = [USER, PASSWORD, PRESS, RESULT];
// Movimento reduzido: as máquinas já na lista.
const STILL = 4;
const FADE = { duration: 240 };
const SAMPLE_PASSWORD = '12345678';
const noop = () => {};

export function LoginPasswordAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const showResult = t >= RESULT;
  const form = useAnimatedStyle(() => ({ opacity: reduced ? (showResult ? 0 : 1) : withTiming(showResult ? 0 : 1, FADE) }), [showResult, reduced]);
  const result = useAnimatedStyle(() => ({ opacity: reduced ? (showResult ? 1 : 0) : withTiming(showResult ? 1 : 0, FADE) }), [showResult, reduced]);
  const pressed = t >= PRESS;
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {/* O formulário fica no layout mesmo invisível: a altura dele não muda de um quadro para outro. */}
    <Animated.View style={[styles.form, form]}>
      <LoginField label={m.login_usuario()} value={t >= USER ? m.news_login_sample_user() : ''} editable={false} />
      <LoginField label={m.login_senha()} value={t >= PASSWORD ? SAMPLE_PASSWORD : ''} secureTextEntry editable={false} />
      <LoginSubmit label={m.login_entrar()} text={pressed ? m.login_conectando() : m.login_entrar()} blocked={pressed} onPress={noop} />
    </Animated.View>
    <Animated.View style={[styles.result, result]}>
      <MachineGroup>
        <MachineRow title={m.news_tags_sample_machine()} value={m.mobile_machines_connected()} valueTone="success" />
        <MachineRow title={m.news_auto_report_sample_machine()} value={m.mobile_machines_connected()} valueTone="success" />
      </MachineGroup>
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: {
    padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  form: { gap: theme.base.space[3] },
  result: { position: 'absolute', left: theme.base.space[3], right: theme.base.space[3], top: 0, bottom: 0, justifyContent: 'center' },
}));
