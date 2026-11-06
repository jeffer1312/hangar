import { Text, View } from 'react-native';
import Animated, { useAnimatedStyle, useReducedMotion, withTiming } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { ComposerBox } from '../../chat/ComposerField';
import { RequestNotice } from '../vault/VaultRequests';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `pedido-na-sessao`: o agente pede uma senha, a faixa do pedido aparece no chat dele, acima da caixa
// de mensagem; respondido, a faixa sai e o agente segue.
const CYCLE = 6;
const CARD = 0.6;
const RESULT = 3.6;
const STEPS = [CARD, RESULT];
// Movimento reduzido: a faixa à mostra, antes do toque.
const STILL = 1.5;
const FADE = { duration: 240 };
const noop = () => {};

export function SessionVaultAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const showCard = t >= CARD && t < RESULT;
  const showResult = t >= RESULT;
  const card = useAnimatedStyle(() => ({ opacity: reduced ? (showCard ? 1 : 0) : withTiming(showCard ? 1 : 0, FADE) }), [showCard, reduced]);
  const result = useAnimatedStyle(() => ({ opacity: reduced ? (showResult ? 1 : 0) : withTiming(showResult ? 1 : 0, FADE) }), [showResult, reduced]);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <Text style={styles.agent}>{m.vault_app_session_news_agent()}</Text>
    {/* A faixa fica no layout mesmo invisível: a altura não muda de um quadro para outro. */}
    <View>
      <Animated.View style={card}>
        <RequestNotice title={m.vault_app_session_entry({ label: m.vault_app_session_news_label() })} onPress={noop} />
      </Animated.View>
      <Animated.View style={[styles.result, result]}>
        <Text style={styles.agent}>{m.vault_app_session_news_result()}</Text>
      </Animated.View>
    </View>
    <ComposerBox focused={false}>
      <Text style={styles.placeholder} numberOfLines={1}>{m.composer_mensagem_para({ nome: 'Claude' })}</Text>
    </ComposerBox>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: {
    gap: theme.base.space[1.5], padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  agent: { fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
  result: { position: 'absolute', left: 0, right: 0, top: 0, bottom: 0, justifyContent: 'center' },
  placeholder: { padding: theme.base.space[3], fontSize: theme.base.text.sm, color: theme.tokens.text.muted },
}));
