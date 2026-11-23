import { View } from 'react-native';
import Animated, { FadeIn, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { PortEntry } from '@hangar/core';
import { PortRow, PortsBody, PortsStatus } from '../../chat/PortsSheet';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';
import { useScriptStep } from './scriptStep';

const WIDTH = 390;
const HEIGHT = 300;
const CYCLE = 7;
/** As portas aparecem, a 3000 abre a página, a 5432 diz que não é página. */
const STEPS = [1.4, 3, 4.8];
/** Movimento reduzido para nas duas portas, com o aviso do Postgres. */
const STILL = 5.5;
const PORTS: PortEntry[] = [{ port: 3000, process: 'next-server', pid: 1 }, { port: 5432, process: 'postgres', pid: 2 }];
const noop = () => {};

export function PortsAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const step = STEPS.filter((s) => t >= s).length;
  const scale = width / WIDTH;
  return <View style={[styles.stage, { width, height: HEIGHT * scale }]} pointerEvents="none"
    accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <View style={[styles.surface, { width: WIDTH, height: HEIGHT, transform: [{ scale }] }]}>
      <PortsBody>
        {step === 0 ? <PortsStatus text={m.mobile_ports_empty()} /> : (
          <Animated.View entering={reduced ? undefined : FadeIn.duration(150)}>
            {PORTS.map((p) => <PortRow key={p.port} port={p} busy={step === 2 ? 3000 : null}
              failure={step === 3 && p.port === 5432 ? m.mobile_ports_not_page() : null} onPress={noop} />)}
          </Animated.View>
        )}
      </PortsBody>
    </View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  stage: { overflow: 'hidden', borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.default },
  surface: { transformOrigin: 'top left', overflow: 'hidden', backgroundColor: superficie(theme, 0.95) },
}));
