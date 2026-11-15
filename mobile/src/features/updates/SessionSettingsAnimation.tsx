import { View } from 'react-native';
import Animated, { FadeIn, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { SessionSettingsContent, SessionSettingsFrame, SessionSettingsLink } from '../../chat/SessionSettingsFrame';
import { SettingsList } from '../../ui/SettingsList';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

const WIDTH = 390;
const HEIGHT = 584;
const CYCLE = 6.5;
const STEPS = [2, 3.5, 4.5];
const noop = () => {};

export function SessionSettingsAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, 4.5, reduced);
  const detail = t >= 2 && t < 4.5;
  const low = t >= 3.5;
  const scale = width / WIDTH;
  const effort = low ? m.reasoning_level_low() : m.reasoning_level_high();
  const models = [
    { id: 'primary', label: m.news_session_model_primary(), hint: m.news_session_model_primary_hint(), selected: true },
    { id: 'second', label: m.news_session_model_second(), hint: m.news_session_model_second_hint() },
    { id: 'third', label: m.news_session_model_third(), hint: m.news_session_model_third_hint() },
  ];
  const levels = [
    { id: 'low', label: m.reasoning_level_low(), selected: low },
    { id: 'medium', label: m.reasoning_level_medium() },
    { id: 'high', label: m.reasoning_level_high(), selected: !low },
  ];
  return <View style={[styles.stage, { width, height: HEIGHT * scale }]} pointerEvents="none"
    accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={[styles.surface, { width: WIDTH, height: HEIGHT, transform: [{ scale }] }]}>
      <SessionSettingsFrame height={HEIGHT} title={detail ? m.native_new_chat_reasoning() : m.native_session_settings_title()}
        subtitle={`${m.news_session_model_primary()} · ${effort}`} tab={detail ? undefined : 'model'}
        onTab={noop} onBack={detail ? noop : undefined} onClose={noop}
        footer={detail ? undefined : <SessionSettingsLink label={m.native_new_chat_reasoning()} value={effort} onPress={noop} />}>
        <Animated.View key={detail ? 'effort' : 'models'} style={styles.body} entering={reduced ? undefined : FadeIn.duration(180)}>
          <SessionSettingsContent title={detail ? undefined : m.native_ctl_featured_models()}>
            <SettingsList comfortable searchable={false} items={detail ? levels : models} onPick={noop} />
            {detail ? null : <SessionSettingsLink label={m.native_ctl_all_models()} onPress={noop} />}
          </SessionSettingsContent>
        </Animated.View>
      </SessionSettingsFrame>
    </View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: { overflow: 'hidden', borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default },
  surface: { transformOrigin: 'top left', backgroundColor: superficie(theme, 0.95) },
  body: { flex: 1, minHeight: 0 },
}));
