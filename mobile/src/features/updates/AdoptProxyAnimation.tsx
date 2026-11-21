import { useContext, useEffect, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router';
import { useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { CliproxyInstall, CliproxyInstallStep } from '@hangar/core';
import { WorkingMarkPaused } from '../../ui/WorkingMark';
import { superficie } from '../../theme/superficie';
import { ProxyCard, type Outcome } from '../accounts/ProxyUpdate';
import { useScriptStep } from './scriptStep';

// A linha do CLIProxyAPI de fora em Contas: "Deixar o Hangar cuidar", as quatro etapas e o concluído.
const CLICK = 1.8;
const CONFIG = 2.6;
const SERVICE = 3.4;
const START = 4.2;
const DONE = 5;
const CYCLE = 7;
const OLD = 'v8.0.56';
const NEW = 'v8.0.79';

const external = (patch: Partial<CliproxyInstall> = {}): CliproxyInstall => ({
  state: 'external', version: OLD, latest: NEW, update_available: false, running: true, base_url: 'http://127.0.0.1:8317',
  accounts: 3, sessions: 0, handover: { ok: true }, job: null, ...patch,
});
const running = (step: CliproxyInstallStep) => external({ job: { kind: 'adopt', step, done: false, error: null, outcome: null } });

export function frameAt(t: number): { install: CliproxyInstall; outcome: Outcome | null } {
  if (t < CLICK) return { install: external(), outcome: null };
  if (t < CONFIG) return { install: running('download'), outcome: null };
  if (t < SERVICE) return { install: running('config'), outcome: null };
  if (t < START) return { install: running('service'), outcome: null };
  if (t < DONE) return { install: running('start'), outcome: null };
  return { install: external({ state: 'managed', version: NEW, handover: null }), outcome: { k: 'done', action: 'adopt', tag: NEW } };
}

export function AdoptProxyAnimation({ width }: { width: number }) {
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
  // Parada, mostra o proxy de fora com o "Deixar o Hangar cuidar": o que a novidade traz.
  const t = useScriptStep([CLICK, CONFIG, SERVICE, START, DONE], CYCLE, 0, still);
  const frame = frameAt(t);
  return (
    <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
      <View style={styles.slot}>
        <ProxyCard state={{ ...frame, posting: false, update: async () => {}, adopt: async () => {} }} onConnect={() => {}} />
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  stage: { padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
  slot: { minHeight: 220 },
}));
