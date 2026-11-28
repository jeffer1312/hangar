import { View } from 'react-native';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { KIMI_PERMISSION_MODES } from '@hangar/core';
import { SessionSettingsContent, SessionSettingsFrame } from '../../chat/SessionSettingsFrame';
import { permissionLabel } from '../pills/permissionLabel';
import { SettingsList } from '../../ui/SettingsList';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

const WIDTH = 390;
const HEIGHT = 410;
const STEPS = [1.8, 3.6];
const noop = () => {};

export function KimiPermissionsAnimation({ width }: { width: number }) {
  const t = useScriptStep(STEPS, 6, 3.6, useReducedMotion());
  const selected = t >= 3.6 ? 'auto' : t >= 1.8 ? 'yolo' : 'manual';
  const scale = width / WIDTH;
  return <View style={[styles.stage, { width, height: HEIGHT * scale }]} pointerEvents="none"
    accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={[styles.surface, { width: WIDTH, height: HEIGHT, transform: [{ scale }] }]}>
      <SessionSettingsFrame height={HEIGHT} title={m.native_ctl_mode_title()} subtitle="Kimi" onTab={noop} onClose={noop}>
        <SessionSettingsContent>
          <SettingsList comfortable searchable={false} onPick={noop} items={KIMI_PERMISSION_MODES.map(mode => ({
            id: mode, label: permissionLabel(mode, 'kimi'), selected: mode === selected,
          }))} />
        </SessionSettingsContent>
      </SessionSettingsFrame>
    </View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: { overflow: 'hidden', borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default },
  surface: { transformOrigin: 'top left', backgroundColor: superficie(theme, 0.95) },
}));
