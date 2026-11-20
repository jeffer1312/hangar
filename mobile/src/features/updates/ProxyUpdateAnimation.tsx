import { useContext, useEffect, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { CliproxyInstall, CliproxyInstallStep } from '@hangar/core';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { ProxyUpdateCard, type Outcome } from '../accounts/ProxyUpdate';
import { useScriptStep } from './scriptStep';

// O cartão de versão nova do CLIProxyAPI em Contas: Atualizar, as três etapas e o concluído.
const CLICK = 1.6;
const SERVICE = 2.5;
const START = 3.5;
const DONE = 4.4;
const CYCLE = 6;
const OLD = 'v8.0.79';
const NEW = 'v8.1.0';

const install = (patch: Partial<CliproxyInstall>): CliproxyInstall =>
  ({ state: 'managed', version: OLD, latest: NEW, update_available: true, running: true, job: null, ...patch });
const running = (step: CliproxyInstallStep) => install({ job: { kind: 'update', step, done: false, error: null, outcome: null } });

export function frameAt(t: number): { install: CliproxyInstall; outcome: Outcome | null } {
  if (t < CLICK) return { install: install({}), outcome: null };
  if (t < SERVICE) return { install: running('download'), outcome: null };
  if (t < START) return { install: running('service'), outcome: null };
  if (t < DONE) return { install: running('start'), outcome: null };
  return { install: install({ version: NEW, update_available: false }), outcome: { k: 'done', tag: NEW } };
}

export function ProxyUpdateAnimation({ width }: { width: number }) {
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
  // Parada, mostra a versão nova com o Atualizar: o que a novidade traz.
  const t = useScriptStep([CLICK, SERVICE, START, DONE], CYCLE, 0, still);
  const frame = frameAt(t);
  return (
    <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
      <View style={styles.slot}>
        <ProxyUpdateCard state={{ ...frame, posting: false, update: async () => {} }} />
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  stage: { padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
  slot: { minHeight: 150 },
}));
