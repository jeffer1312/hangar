import { Text, View } from 'react-native';
import Animated, { useAnimatedStyle, useReducedMotion, withTiming } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `hangar-connect`: na lista de máquinas da conta, o toque em Entrar vira "Aguardando" enquanto a pessoa
// aprova no computador; aprovado, a máquina fica neste aparelho.
const CYCLE = 6;
const WAITING = 1.2;
const DONE = 3.6;
const STEPS = [WAITING, DONE];
// Movimento reduzido: o resultado, que é o que a novidade promete.
const STILL = 4;
const FADE = { duration: 240 };
const DEVICE = 'iPhone';
const MACHINE = 'pc-casa';

export function ConnectAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const done = t >= DONE;
  const value = done ? m.connect_machine_added() : t >= WAITING ? m.connect_waiting() : m.login_entrar();
  const result = useAnimatedStyle(() => ({ opacity: reduced ? (done ? 1 : 0) : withTiming(done ? 1 : 0, FADE) }), [done, reduced]);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <MachineGroup>
      <MachineRow title={MACHINE} value={value} valueTone={done ? 'success' : undefined} />
    </MachineGroup>
    {/* A frase fica no layout mesmo invisível: a altura não muda de um quadro para outro. */}
    <Animated.View style={result}>
      <Text style={styles.result}>{m.connect_news_result({ device: DEVICE, machine: MACHINE })}</Text>
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: {
    gap: theme.base.space[1.5], padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  result: { fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
}));
