import { useContext, useEffect, useRef, useState } from 'react';
import { AppState, Text, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { useSettingsColors } from '../config/colors';
import { FastControl } from '../../chat/SessionSettings';
import { SPEED_TIERS, speedLabel } from '../pills/serviceTierControl';
import { useScriptStep } from './scriptStep';

// No menu do modelo a velocidade passa de Normal a Fast e a Ultra Fast; a linha do modelo mostra a escolha.
const FAST = 1.8;
const ULTRA = 3.6;
const CYCLE = 6;
const MODEL = 'gpt-6.2-sol';

export function SpeedChoiceAnimation({ width }: { width: number }) {
  const c = useSettingsColors();
  const paused = useContext(WorkingMarkPaused);
  const reduced = useReducedMotion();
  const focused = useIsFocused();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const [inView, setInView] = useState(false);
  const [active, setActive] = useState(AppState.currentState === 'active');
  useEffect(() => { const sub = AppState.addEventListener('change', (state) => setActive(state === 'active')); return () => sub.remove(); }, []);
  useEffect(() => {
    if (!focused || !active || paused) return;
    const check = () => stage.current?.measureInWindow((_x, y, _w, height) => setInView(height > 0 && y < window.height && y + height > 0));
    check();
    const timer = setInterval(check, 500);
    return () => clearInterval(timer);
  }, [focused, active, paused, window.height]);
  const still = reduced || paused || !active || !focused || !inView;
  // Parada, mostra o Ultra Fast escolhido: o que a novidade traz.
  const t = useScriptStep([FAST, ULTRA], CYCLE, ULTRA, still);
  const tier = SPEED_TIERS[t >= ULTRA ? 2 : t >= FAST ? 1 : 0];
  return (
    <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
      <FastControl comfortable fast={{ tier, available: true, ultrafast: true, choose: async () => {} }} />
      <Text style={[styles.model, { color: c.text }]}>
        {MODEL}{tier === 'default' ? '' : <Text style={{ color: c.muted }}>{` · ${speedLabel(tier)}`}</Text>}
      </Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  stage: { gap: theme.base.space[3], padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
  model: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
}));
