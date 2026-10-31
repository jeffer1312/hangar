import { View } from 'react-native';
import Animated, { useAnimatedStyle, useReducedMotion, withTiming } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { VaultApprovalRows } from '../vault/VaultApprovalRows';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `cofre-de-senhas`: o agente pede para usar um token num comando, o Permitir é apertado e a
// saída do comando volta com o valor apagado.
const CYCLE = 6;
const CARD = 0.4;
const PRESS = 3;
const RESULT = 3.6;
const STEPS = [CARD, PRESS, RESULT];
// Movimento reduzido: o cartão à mostra, antes do toque.
const STILL = 1.5;
const FADE = { duration: 240 };
const noop = () => {};

export function VaultAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const showCard = t >= CARD && t < RESULT;
  const showResult = t >= RESULT;
  const card = useAnimatedStyle(() => ({ opacity: reduced ? (showCard ? 1 : 0) : withTiming(showCard ? 1 : 0, FADE) }), [showCard, reduced]);
  const result = useAnimatedStyle(() => ({ opacity: reduced ? (showResult ? 1 : 0) : withTiming(showResult ? 1 : 0, FADE) }), [showResult, reduced]);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {/* O cartão fica no layout mesmo invisível: a altura dele não muda de um quadro para outro. */}
    <Animated.View style={[styles.card, card]}>
      <VaultApprovalRows
        request={{ entry_name: m.vault_app_news_entry(), origin: m.vault_app_news_origin(), target: m.vault_app_news_command(), reason: '' }}
        allowPressed={t >= PRESS} onAllow={noop} onDeny={noop} />
    </Animated.View>
    <Animated.View style={[styles.result, result]}>
      <MachineGroup>
        <MachineRow title={m.vault_app_news_command()} description={m.vault_app_news_output()} />
      </MachineGroup>
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: {
    padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  card: { gap: theme.base.space[3] },
  result: { position: 'absolute', left: theme.base.space[3], right: theme.base.space[3], top: 0, bottom: 0, justifyContent: 'center' },
}));
