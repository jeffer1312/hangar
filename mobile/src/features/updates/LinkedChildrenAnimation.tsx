import { useMemo } from 'react';
import { View } from 'react-native';
import Animated, { FadeIn, LinearTransition, SlideOutLeft, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { AggSession } from '@hangar/core';
import { SessionRow } from '../sessions/SessionRow';
import { SessionSectionCard, SessionSectionHeader } from '../sessions/SessionSection';
import { buildSessionSections } from '../sessions/sessionSections';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `filhas-ligadas`: a conversa que abriu duas filhas trabalha com elas; o Parar dela para as três juntas;
// fechada só ela, as filhas ficam abertas com a marca "sem pai" e o nome de quem as abriu.
const CYCLE = 6.5;
const STOP = 1.8;
const CLOSE = 3.4;
const STEPS = [STOP, CLOSE];
// Movimento reduzido: o fim, com as filhas abertas e a marca de quem as abriu.
const STILL = 4.5;

const noop = () => {};
const NO_GROUPS = new Set<string>();

function sampleRows(): AggSession[] {
  const base = { serverId: 'news', serverLabel: '', serverColor: '', provider: 'claude' as const, tracked: true, state: 'working' as const, branch: 'main' };
  const now = Date.now() / 1000;
  const parent = m.news_linked_sample_parent();
  return [
    { ...base, name: parent, label: m.news_linked_sample_task_parent(), last_activity: now - 60 },
    { ...base, name: m.news_linked_sample_child_docs(), label: m.news_linked_sample_task_docs(), linked_by: parent, last_activity: now - 120 },
    { ...base, name: m.news_linked_sample_child_tests(), label: m.news_linked_sample_task_tests(), linked_by: parent, last_activity: now - 180 },
  ];
}

export function LinkedChildrenAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const rows = useMemo(sampleRows, []);
  const parent = rows[0].name;
  const stopped = t >= STOP;
  const closed = t >= CLOSE;
  const sections = useMemo(() => buildSessionSections(rows
    .filter((r) => !closed || r.name !== parent)
    .map((r) => !stopped ? r
      : closed ? { ...r, state: 'idle' as const, label: undefined, linked_by: undefined, orphaned_from: parent }
        : { ...r, state: 'idle' as const, label: undefined }), 'expanded', 'none', '', NO_GROUPS), [rows, parent, stopped, closed]);
  const layout = reduced ? undefined : LinearTransition.duration(260);
  const enter = reduced ? undefined : FadeIn.duration(240);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {sections.map((section) => <Animated.View key={section.id} layout={layout}>
      {section.label ? <SessionSectionHeader section={section} onToggle={noop} /> : null}
      {section.data.map((s, index) => <Animated.View key={`${section.id}::${s.name}`} layout={layout}
        exiting={reduced ? undefined : SlideOutLeft.duration(320)}>
        {/* A linha troca de quadro com um esmaecer curto: o trabalho que para, a marca que entra. */}
        <Animated.View key={`${stopped}-${closed}`} entering={enter}>
          <SessionSectionCard index={index} total={section.data.length}>
            <SessionRow session={s} compact mostrarServidor={false} onPress={noop} />
          </SessionSectionCard>
        </Animated.View>
      </Animated.View>)}
    </Animated.View>)}
  </View>;
}

const styles = StyleSheet.create(theme => ({
  // Altura reservada para o quadro mais alto (as três linhas trabalhando): o cartão não pula entre os passos.
  stage: {
    minHeight: 232, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
}));
