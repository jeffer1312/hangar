import { useMemo } from 'react';
import { View } from 'react-native';
import Animated, { FadeIn, FadeOut, LinearTransition, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { InstalledPlugin } from '@hangar/core';
import { MachineGroup } from '../machines/MachineRows';
import { InstalledPluginRow, ReopenBannerView } from '../plugins/parts';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `plugins-do-claude`: caveman com versão nova à vista, a chave do humanizer liga e o aviso de
// reabrir as sessões abertas aparece em cima da lista, como na tela.
const CYCLE = 6;
const FLIP = 1.5;
const BANNER = 2.1;
const STEPS = [FLIP, BANNER];
// Movimento reduzido: humanizer ligado, com o aviso de reabrir.
const STILL = 3;
const OPEN_SESSIONS = 3;

const noop = () => {};
const NO_REFUSED: string[] = [];

function samplePlugins(): InstalledPlugin[] {
  const day = 24 * 3600 * 1000;
  const base = { update_known: true, notes: [] };
  return [
    { ...base, id: 'caveman@caveman', name: 'caveman', marketplace: 'caveman', version: '4.1.0', enabled: true,
      last_updated: new Date(Date.now() - 12 * day).toISOString(), available_version: '4.2.0', description: m.news_plugins_sample_caveman() },
    { ...base, id: 'humanizer@humanizer', name: 'humanizer', marketplace: 'humanizer', version: '3.1.0', enabled: false,
      last_updated: new Date(Date.now() - 3 * day).toISOString(), available_version: null, description: m.news_plugins_sample_humanizer() },
  ];
}

export function PluginsAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const plugins = useMemo(samplePlugins, []);
  const on = t >= FLIP;
  const banner = t >= BANNER;
  const layout = reduced ? undefined : LinearTransition.duration(260);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {banner ? <Animated.View style={styles.banner} entering={reduced ? undefined : FadeIn.duration(240)} exiting={reduced ? undefined : FadeOut.duration(200)}>
      <ReopenBannerView n={OPEN_SESSIONS} action={m.plugins_action_enabled({ name: 'humanizer' })} refused={NO_REFUSED}
        reopening={false} onReopen={noop} onLater={noop} />
    </Animated.View> : null}
    <Animated.View layout={layout}>
      <MachineGroup>
        {plugins.map((p) => <InstalledPluginRow key={p.id} plugin={p.name === 'humanizer' ? { ...p, enabled: on } : p}
          updating={false} disabled={false} onPress={noop} onUpdate={noop} onToggle={noop} />)}
      </MachineGroup>
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  // Altura do quadro mais alto (com o aviso): o cartão não pula entre os passos.
  stage: {
    minHeight: 300, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  banner: { gap: theme.base.space[2], marginBottom: theme.base.space[2] },
}));
