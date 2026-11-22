import { View } from 'react-native';
import Animated, { FadeIn, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { ProviderConfirmationSummary } from '../../chat/ClaudeProviderSettings';
import { SessionSettingsContent, SessionSettingsFrame, SessionSettingsLink } from '../../chat/SessionSettingsFrame';
import { SettingsList } from '../../ui/SettingsList';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

const WIDTH = 390;
const HEIGHT = 530;
const STEPS = [1.3, 2.7, 4.2];
const ACCOUNT = 'ChatGPT Pro';
const MODEL = 'GPT-6.2-Sol';
const noop = () => {};

export function ClaudeProvidersAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, 6.5, 4.2, reduced);
  const account = t >= STEPS[0];
  const model = t >= STEPS[1];
  const confirmation = t >= STEPS[2];
  const scale = width / WIDTH;
  return <View style={[styles.stage, { width, height: HEIGHT * scale }]} pointerEvents="none"
    accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={[styles.surface, { width: WIDTH, height: HEIGHT, transform: [{ scale }] }]}>
      <SessionSettingsFrame height={HEIGHT} title={m.native_claude_provider_picker()} subtitle="Claude"
        onTab={noop} onBack={noop} onClose={noop}>
        <Animated.View key={confirmation ? 'confirm' : 'choice'} style={styles.body} entering={reduced ? undefined : FadeIn.duration(150)}>
          <SessionSettingsContent title={confirmation ? undefined : m.native_claude_provider_choose_account()}>
            {confirmation ? <>
              <ProviderConfirmationSummary account={ACCOUNT} model={MODEL} />
              <SessionSettingsLink label={m.native_claude_provider_confirm()} onPress={noop} />
            </> : <>
              <SettingsList comfortable searchable={false} onPick={noop} items={[
                { id: 'pro', label: ACCOUNT, hint: 'CLIProxyAPI', selected: account },
                { id: 'plus', label: 'ChatGPT Plus', hint: 'CLIProxyAPI' },
              ]} />
              {account ? <SettingsList comfortable searchable={false} onPick={noop} items={[
                { id: MODEL, label: MODEL, selected: model },
                { id: 'GPT-6.1-Sol', label: 'GPT-6.1-Sol' },
              ]} /> : null}
              <SessionSettingsLink label={m.native_claude_provider_apply()} disabled={!model} onPress={noop} />
            </>}
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
