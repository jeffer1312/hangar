import { Text, View } from 'react-native';
import Animated, { FadeIn, FadeOut, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { ComposerBox } from '../../chat/ComposerField';
import { WorkingLine } from '../../chat/LiveWork';
import { NoticeLine } from '../../chat/MessageList';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `compactar-parada`: a conversa parada é compactada antes do cache vencer, e fica o aviso
// "Contexto compactado" no lugar da linha de trabalho.
const CYCLE = 6;
const COMPACTING = 1;
const DONE = 3.2;
const STEPS = [COMPACTING, DONE];
// Movimento reduzido: o aviso de compactado à vista.
const STILL = 4;

export function CompactIdleAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const enter = reduced ? undefined : FadeIn.duration(220);
  const exit = reduced ? undefined : FadeOut.duration(180);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {/* Altura reservada para a linha e o aviso: o cartão não pula entre os passos. */}
    <View style={styles.body}>
      {t >= COMPACTING && t < DONE ? <Animated.View key="working" entering={enter} exiting={exit}>
        <WorkingLine label={m.news_compact_idle_working()} since={null} />
      </Animated.View> : null}
      {t >= DONE ? <Animated.View key="notice" entering={enter} exiting={exit}>
        <NoticeLine text={m.notice_compacted()} />
      </Animated.View> : null}
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
  body: { minHeight: 44, justifyContent: 'flex-end', paddingHorizontal: theme.base.space[2] },
  placeholder: { padding: theme.base.space[3], fontSize: theme.base.text.sm, color: theme.tokens.text.muted },
}));
