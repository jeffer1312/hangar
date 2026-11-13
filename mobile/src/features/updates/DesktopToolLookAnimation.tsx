import { useCallback, useMemo } from 'react';
import { View } from 'react-native';
import Animated, { FadeIn, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { ChatEvent } from '@hangar/core';
import { DesktopRunView } from '../../chat/tools/DesktopToolGroup';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';

// Roteiro do `estilo-claude-desktop`: a linha cinza "Executou 4 comandos (2 falhas)" abre no cartão com uma linha por
// chamada, e as que deram erro trazem "Falhou".
const CYCLE = 6.5;
const OPEN = 1.6;
const STEPS = [OPEN];
// Movimento reduzido: o cartão aberto.
const STILL = 3;
const SAMPLE: Array<[string, boolean]> = [['npm run build', false], ['cargo test -p hangar-api', true], ['git status --short', false], ['npm test', true]];

function sample(): { calls: ChatEvent[]; results: Map<string, ChatEvent> } {
  const calls: ChatEvent[] = [];
  const results = new Map<string, ChatEvent>();
  SAMPLE.forEach(([command, failed], n) => {
    const id = `news-desktop-${n}`;
    calls.push({ id, kind: 'tool_use', tool_use_id: id, tool_name: 'Bash', tool_input: { command } });
    results.set(id, { id: `r-${id}`, kind: 'tool_result', tool_use_id: id, is_error: failed });
  });
  return { calls, results };
}

export function DesktopToolLookAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const { calls, results } = useMemo(sample, []);
  const resultOf = useCallback((call: ChatEvent) => results.get(call.id) ?? null, [results]);
  const open = t >= OPEN;
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <Animated.View key={open ? 'open' : 'closed'} entering={reduced || !open ? undefined : FadeIn.duration(220)}>
      <DesktopRunView calls={calls} resultOf={resultOf} open={open} />
    </Animated.View>
  </View>;
}

const styles = StyleSheet.create(theme => ({
  // Altura do cartão aberto: o quadro não pula quando ele abre.
  stage: {
    minHeight: 250, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
}));
