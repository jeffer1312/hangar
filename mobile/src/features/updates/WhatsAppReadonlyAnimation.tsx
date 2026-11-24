import { useMemo, useState } from 'react';
import { View } from 'react-native';
import Animated, { FadeIn, useAnimatedStyle, useReducedMotion, withTiming } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { WhatsAppState, WhatsAppStatus } from '@hangar/core';
import { Connected, Off, Pair } from '../config/WhatsAppParts';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';

const WIDTH = 390;
const HEIGHT = 520;
const CYCLE = 9;
/** Marca o aceite, mostra o QR, conecta e desce até as ferramentas das sessões. */
const STEPS = [1.4, 2.8, 4.8, 6.6];
/** Movimento reduzido para no conectado, com os totais à vista. */
const STILL = 5;
// Número sempre mascarado, como o servidor manda.
const DEVICE = '+55 64 •••••-1234';
const noop = () => {};

/** Um QR de mentira (não abre nada): os três quadrados de canto e módulos espalhados. */
export function sampleQr(): string {
  const n = 21;
  const corners = [[0, 0], [n - 7, 0], [0, n - 7]];
  const finder = (x: number, y: number) => {
    const c = corners.find(([fx, fy]) => x >= fx && x < fx + 7 && y >= fy && y < fy + 7);
    if (!c) return null;
    const [dx, dy] = [x - c[0], y - c[1]];
    return dx === 0 || dx === 6 || dy === 0 || dy === 6 || (dx >= 2 && dx <= 4 && dy >= 2 && dy <= 4);
  };
  const nearFinder = (x: number, y: number) => (x < 8 && y < 8) || (x >= n - 8 && y < 8) || (x < 8 && y >= n - 8);
  let rects = '';
  for (let y = 0; y < n; y++) {
    for (let x = 0; x < n; x++) {
      const f = finder(x, y);
      const dark = f ?? (!nearFinder(x, y) && (x * 31 + y * 17 + x * y * 7) % 5 < 2);
      if (dark) rects += `<rect x="${x}" y="${y}" width="1" height="1"/>`;
    }
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n} ${n}" shape-rendering="crispEdges"><g fill="#000">${rects}</g></svg>`;
}

const status = (s: Partial<WhatsAppStatus>): WhatsAppStatus =>
  ({ state: 'unpaired', error: null, device: null, pair_code: null, since: null, last_message_at: null, ...s });
const base = { installed: true, accepted_at: '2026-11-22T10:00:00Z', service: 'running' as const, job: null, qr_svg: null };

export function WhatsAppReadonlyAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const step = STEPS.filter((s) => t >= s).length;
  const [contentHeight, setContentHeight] = useState(0);
  const pairing: WhatsAppState = useMemo(() => ({ ...base, status: status({ state: 'pairing' }), qr_svg: sampleQr() }), []);
  const connected: WhatsAppState = useMemo(() => ({
    ...base,
    status: status({
      state: 'connected', device: DEVICE, device_name: 'Hangar', since: '2025-06-01T12:00:00Z',
      last_message_at: new Date(Date.now() - 120_000).toISOString(),
      totals: { messages: 4195, chats: 128, groups: 23, media_saved: 566, media_phone_only: 1204, media_lost: 37 },
    }),
  }), []);
  // Desce o conectado até o fim, onde ficam as ferramentas que as sessões usam.
  const down = step >= 4 ? -Math.max(0, contentHeight - HEIGHT) : 0;
  const scroll = useAnimatedStyle(() => ({ transform: [{ translateY: reduced ? down : withTiming(down, { duration: 900 }) }] }), [down, reduced]);
  const frame = step <= 1 ? 'off' : step === 2 ? 'pair' : 'connected';
  const scale = width / WIDTH;
  return <View style={[styles.stage, { width, height: HEIGHT * scale }]} pointerEvents="none"
    accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={[styles.surface, { width: WIDTH, height: HEIGHT, transform: [{ scale }] }]}>
      <Animated.View key={frame} style={[styles.body, frame === 'connected' && scroll]} entering={reduced ? undefined : FadeIn.duration(150)}
        onLayout={frame === 'connected' ? (e) => setContentHeight(e.nativeEvent.layout.height) : undefined}>
        {frame === 'off' ? <Off accepted={step === 1} busy={null} onToggle={noop} onInstall={noop} />
          : frame === 'pair' ? <Pair state={pairing} view="pairing" busy={null} qrWanted asked={false} phone="" phoneError="" awaiting={false}
            onPhone={noop} onRequestCode={noop} onShowQr={noop} onBackToPhone={noop} onChangeNumber={noop} onCancel={noop} onRetry={noop} />
          : <Connected state={connected} />}
      </Animated.View>
    </View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: { overflow: 'hidden', borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default },
  surface: { transformOrigin: 'top left', overflow: 'hidden', backgroundColor: superficie(theme, 0.95) },
  body: { padding: theme.base.space[4], gap: theme.base.space[3] },
}));
