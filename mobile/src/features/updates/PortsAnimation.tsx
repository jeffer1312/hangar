import { Text, View } from 'react-native';
import Animated, { FadeIn, SlideInRight, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { PortEntry } from '@hangar/core';
import { PortRow, PortsBody, PortsStatus, PortTabs } from '../../chat/PortsPieces';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';
import { useScriptStep } from './scriptStep';

const WIDTH = 390;
const HEIGHT = 300;
const CYCLE = 7;
/** As portas aparecem, a 3000 abre a página dentro do app, a aba da 5173 abre a outra sem fechar a primeira. */
const STEPS = [1.2, 2.6, 4.8];
/** Movimento reduzido para na página da 3000 aberta no app. */
const STILL = 3.5;
const PORTS: PortEntry[] = [{ port: 3000, process: 'next-server', pid: 1 }, { port: 5173, process: 'vite', pid: 2 }];
const TABS = PORTS.map((p) => p.port);
const noop = () => {};

/** A página do projeto, que no app é uma WebView: só a forma dela, sem texto do Hangar. */
function Page({ port }: { port: number }) {
  return <View style={styles.page}>
    <Text style={styles.address} numberOfLines={1}>localhost:{port}</Text>
    <View style={[styles.bar, { width: port === 3000 ? '70%' : '45%' }]} />
    <View style={[styles.bar, styles.thin, { width: '90%' }]} />
    <View style={[styles.bar, styles.thin, { width: port === 3000 ? '60%' : '80%' }]} />
    <View style={[styles.block, port === 3000 ? styles.blockA : styles.blockB]} />
  </View>;
}

export function PortsAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const step = STEPS.filter((s) => t >= s).length;
  const scale = width / WIDTH;
  const active = step === 3 ? 5173 : 3000;
  return <View style={[styles.stage, { width, height: HEIGHT * scale }]} pointerEvents="none"
    accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={[styles.surface, { width: WIDTH, height: HEIGHT, transform: [{ scale }] }]}>
      {step < 2 ? (
        <PortsBody note={m.mobile_ports_tunnel_note()}>
          {step === 0 ? <PortsStatus text={m.mobile_ports_empty()} /> : (
            <Animated.View entering={reduced ? undefined : FadeIn.duration(150)}>
              {PORTS.map((p) => <PortRow key={p.port} port={p} busy={null} failure={null} onPress={noop} onLink={noop} onStop={noop} />)}
            </Animated.View>
          )}
        </PortsBody>
      ) : (
        <Animated.View style={styles.fill} entering={reduced ? undefined : SlideInRight.duration(260)}>
          <PortTabs tabs={TABS} active={active} onSelect={noop} />
          <Animated.View key={active} style={styles.fill} entering={reduced ? undefined : FadeIn.duration(180)}>
            <Page port={active} />
          </Animated.View>
        </Animated.View>
      )}
    </View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: { overflow: 'hidden', borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default },
  surface: { transformOrigin: 'top left', overflow: 'hidden', backgroundColor: superficie(theme, 0.95) },
  fill: { flex: 1 },
  page: { flex: 1, gap: theme.base.space[2], marginHorizontal: theme.base.space[3], marginBottom: theme.base.space[3], padding: theme.base.space[3],
    borderRadius: theme.base.radius.md, backgroundColor: theme.tokens.bg.base, borderWidth: 1, borderColor: theme.tokens.border.default },
  address: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted, fontVariant: ['tabular-nums'] },
  bar: { height: 14, borderRadius: theme.base.radius.xs, backgroundColor: theme.tokens.text.muted, opacity: 0.35 },
  thin: { height: 8, opacity: 0.2 },
  block: { flex: 1, borderRadius: theme.base.radius.md, opacity: 0.25, marginTop: theme.base.space[1] },
  blockA: { backgroundColor: theme.tokens.accent.base },
  blockB: { backgroundColor: theme.tokens.status.warning },
}));
