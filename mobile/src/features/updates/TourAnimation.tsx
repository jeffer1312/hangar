import { Text, View } from 'react-native';
import Animated, { FadeIn, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { providerName } from '@hangar/core';
import { ComposerActions, ComposerBox } from '../../chat/ComposerField';
import { SessionSettingsControl } from '../../chat/SessionSettings';
import { TourBalloon, tourText } from '../tour/TourBalloon';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `tour-no-celular`: o balão aponta o + do compositor e depois a pílula do modelo, como na conversa.
const MODEL_AT = 2.6;
const CYCLE = 5.2;
const STEPS = [MODEL_AT];
const WIDTH = 390;
const HEIGHT = 300;
const noop = () => {};
const none = async () => {};

export function TourAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, MODEL_AT, reduced);
  const model = t >= MODEL_AT;
  const step = model ? 'model' : 'attach';
  const { title, body } = tourText(step);
  const scale = width / WIDTH;
  return <View style={[styles.stage, { width, height: HEIGHT * scale }]} pointerEvents="none"
    accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={[styles.surface, { width: WIDTH, height: HEIGHT, transform: [{ scale }] }]}>
      <Animated.View key={step} entering={reduced ? undefined : FadeIn.duration(200)} style={styles.balloon}>
        <TourBalloon tag={m.native_tour_progress({ n: model ? 6 : 4, total: 6 })} title={title} body={body} waiting
          arrow="down" arrowX={model ? 214 : 18}
          actions={[{ label: model ? m.mobile_tour_done() : m.native_tour_skip(), primary: true, onPress: noop },
            { label: m.native_tour_skip_all(), onPress: noop }, { label: m.native_tour_later(), onPress: noop }]} />
      </Animated.View>
      <ComposerBox focused={false} footer={(
        <ComposerActions attach={{ onPress: noop, disabled: false }}
          mic={{ onPress: noop, disabled: false, recording: false, styleLabel: '', onStyle: noop }}
          send={{ label: m.nova_conversa_enviar(), disabled: true, onPress: noop }}
          settings={<SessionSettingsControl open={false} onOpen={noop} onClose={noop} panel={false}
            model={{ display: m.news_session_model_primary(), items: [], select: none }}
            effort={{ hidden: true, items: [], select: none }}
            fast={{ hidden: true, tier: 'default', available: false, choose: none }} />} />
      )}>
        <Text style={styles.placeholder} numberOfLines={1}>{m.composer_mensagem_para({ nome: providerName('codex') })}</Text>
      </ComposerBox>
    </View>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  stage: { overflow: 'hidden', borderRadius: theme.base.radius.md, borderCurve: 'continuous', borderWidth: 1, borderColor: theme.tokens.border.default },
  surface: { transformOrigin: 'top left', justifyContent: 'flex-end', paddingBottom: theme.base.space[3], backgroundColor: superficie(theme, 0.95) },
  balloon: { marginHorizontal: theme.base.space[3], marginBottom: theme.base.space[3] },
  placeholder: { fontSize: theme.base.text.base, color: theme.tokens.text.muted, paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3] },
}));
