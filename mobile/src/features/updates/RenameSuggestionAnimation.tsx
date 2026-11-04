import { Text, View } from 'react-native';
import Animated, { FadeIn, FadeInDown, FadeOut, LinearTransition, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { SessionAvatar } from '../sessions/SessionAvatar';
import { UserBubble } from '../../chat/UserBubble';
import { RenameSuggestionView } from '../../chat/RenameSuggestion';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `sugestao-de-nome`: a sessão de nome genérico recebe a primeira mensagem, a faixa propõe um
// nome, o Renomear é apertado e o título troca.
const CYCLE = 6;
const BUBBLE = 0.5;
const STRIP = 1.4;
const PRESS = 2.8;
const APPLY = 3.3;
const STEPS = [BUBBLE, STRIP, PRESS, APPLY];
// Movimento reduzido: a mensagem e a faixa à mostra, antes do toque.
const STILL = 2;

const avatar = { provider: 'claude', engine: null, limited: false, status_line: null, context: null, state: 'idle' } as const;
const noop = () => {};

export function RenameSuggestionAnimation({ width }: { width: number }) {
  const { theme } = useUnistyles();
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const enter = reduced ? undefined : FadeIn.duration(220);
  const exit = reduced ? undefined : FadeOut.duration(200);
  const layout = reduced ? undefined : LinearTransition.duration(220);
  const title = t >= APPLY ? m.news_rename_sample_name() : m.news_rename_sample_generic();
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={styles.header}>
      <SessionAvatar session={avatar} size={30} />
      <Animated.View key={title} style={styles.titleBox} entering={enter}>
        <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>{title}</Text>
      </Animated.View>
    </View>
    <View style={styles.body}>
      {t >= BUBBLE ? <Animated.View entering={enter} exiting={exit} layout={layout}>
        <UserBubble text={m.news_rename_sample_message()} />
      </Animated.View> : null}
    </View>
    {/* Altura reservada para a faixa: o cartão não pula quando ela entra e sai. */}
    <View style={styles.footer}>
      {t >= STRIP && t < APPLY ? <Animated.View entering={reduced ? undefined : FadeInDown.duration(260)} exiting={exit}>
        <RenameSuggestionView suggestion={m.news_rename_sample_name()} applyPressed={t >= PRESS} onApply={noop} onDismiss={noop} />
      </Animated.View> : null}
    </View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: {
    gap: theme.base.space[2], paddingTop: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  // Medidas do título do cabeçalho da conversa (ChatHeader).
  header: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 44, paddingHorizontal: theme.base.space[3] },
  titleBox: { flex: 1, minWidth: 0 },
  name: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  body: { minHeight: 64, paddingHorizontal: theme.base.space[3], justifyContent: 'flex-end' },
  footer: { minHeight: 104 },
}));
