import { useEffect, useMemo, useRef, useState } from 'react';
import { AppState, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router/react-navigation';
import Animated, { FadeIn, LinearTransition, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { StatusAttention, StatusExit, StatusPoint, StatusServerBlock } from '@hangar/core';
import { DaysCard, HealthCard, ServerTiles, dayStartOf } from '../status/StatusBlocks';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';

// Roteiro do `tela-de-status`: tudo funcionando, uma queda chega à linha do tempo e vira ponto de atenção, e o
// bloco dos 7 dias ganha o destaque como quando a pessoa toca no ponto. Mesmos blocos da tela Status.
const CYCLE = 6;
const CRASH = 2.2;
const JUMP = 3.8;
const STEPS = [CRASH, JUMP];
// Movimento reduzido e fora da tela: o quadro com o ponto de atenção e o bloco destacado.
const STILL = 5;
const VERSION = '0.1.0.9160';
const MIB = 1024 * 1024;
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const noop = () => {};

const iso = (ms: number) => new Date(ms).toISOString();

/** Uma hora, um ponto por minuto, com uma onda leve. */
function series(now: number, base: number, swing: number): StatusPoint[] {
  return Array.from({ length: 60 }, (_, k) => {
    const i = k + 1;
    return { ts: iso(now - (59 - k) * MIN), v: base + swing * (Math.sin(i / 7) + 0.4 * Math.cos(i / 2.3)) };
  });
}

function sample(now: number, crashed: boolean) {
  const update: StatusExit = { ts: iso(now - 2 * DAY - 5 * HOUR), kind: 'update', from: '0.1.0.9150', to: VERSION, error_lines: [] };
  const crash: StatusExit = { ts: iso(now - 3 * HOUR), kind: 'crash', cause: 'out_of_memory', error_lines: [] };
  const server: StatusServerBlock = {
    version: VERSION, pid: 4242, started_at: iso(crashed ? now - 3 * HOUR : now - 2 * DAY - 5 * HOUR),
    rss_bytes: 182 * MIB, cpu_pct: 3.1, rss_series: series(now, 176 * MIB, 9 * MIB), cpu_series: series(now, 3.2, 1.4),
    rss_avg_bytes: 175 * MIB, cpu_avg_pct: 3,
  };
  const attention: StatusAttention[] = crashed ? [{ severity: 'warn', code: 'exits_crash', target: 'exits', args: ['1'] }] : [];
  return { server, attention, exits: { items: crashed ? [update, crash] : [update] } };
}

export function StatusAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const [visible, setVisible] = useState(false);
  const focused = useIsFocused();
  const [active, setActive] = useState(AppState.currentState === 'active');
  useEffect(() => { const sub = AppState.addEventListener('change', (state) => setActive(state === 'active')); return () => sub.remove(); }, []);
  useEffect(() => {
    if (!focused || !active) return;
    const check = () => stage.current?.measureInWindow((x, y, w, height) =>
      setVisible(height > 0 && w > 0 && y < window.height && y + height > 0 && x < window.width && x + w > 0));
    check();
    const timer = setInterval(check, 500);
    return () => clearInterval(timer);
  }, [focused, active, window.height, window.width]);
  const still = reduced || !active || !focused || !visible;
  const t = useScriptStep(STEPS, CYCLE, STILL, still);
  const crashed = t >= CRASH;
  const flash = t >= JUMP;
  // O instante da amostra fica fixo: os blocos em `memo` não redesenham a cada passo sem mudança.
  const [now] = useState(() => Date.now());
  const calm = useMemo(() => sample(now, false), [now]);
  const hit = useMemo(() => sample(now, true), [now]);
  const snap = crashed ? hit : calm;
  return <View ref={stage} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={[styles.stage, { width }]}>
    <Animated.View layout={still ? undefined : LinearTransition.duration(260)} style={styles.column}>
      <Animated.View key={crashed ? 'crash' : 'calm'} entering={still ? undefined : FadeIn.duration(240)}>
        <HealthCard attention={snap.attention} server={snap.server} onJump={noop} />
      </Animated.View>
      <ServerTiles server={snap.server} />
      <DaysCard exits={snap.exits} today={dayStartOf(iso(now))} flash={flash} />
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  // Altura do quadro com o ponto de atenção: o cartão não pula entre os passos.
  stage: { minHeight: 560, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden' },
  column: { gap: theme.base.space[2] },
}));
