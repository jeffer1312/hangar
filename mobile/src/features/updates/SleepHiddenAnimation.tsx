import { useMemo } from 'react';
import { View } from 'react-native';
import Animated, { FadeIn, LinearTransition, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { AggSession } from '@hangar/core';
import { SessionRow } from '../sessions/SessionRow';
import { SessionSectionCard, SessionSectionHeader } from '../sessions/SessionSection';
import { buildSessionSections } from '../sessions/sessionSections';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `adormecer-ocultas`: a sessão em "Ocultas (1)" passa do prazo e ganha o selo Adormecida; a mensagem
// seguinte a reabre na mesma conversa e ela volta a trabalhar.
const CYCLE = 6.5;
const SLEEP = 1.6;
const WAKE = 4.2;
const STEPS = [SLEEP, WAKE];
// Movimento reduzido: a oculta já adormecida.
const STILL = 2.5;

const noop = () => {};
const NO_GROUPS = new Set<string>();

function sampleRows(): AggSession[] {
  const base = { serverId: 'news', serverLabel: '', serverColor: '', provider: 'claude' as const, tracked: true, state: 'idle' as const, branch: 'main' };
  const now = Date.now() / 1000;
  return [
    { ...base, name: m.news_tags_sample_session(), last_activity: now - 240 },
    { ...base, name: m.news_rename_sample_generic(), last_activity: now - 900 },
    { ...base, name: m.news_tags_sample_session_other(), provider: 'codex' as const, hidden_from_list: true, last_activity: now - 26 * 3600 },
  ];
}

export function SleepHiddenAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const rows = useMemo(sampleRows, []);
  const target = rows[2].name;
  const asleep = t >= SLEEP && t < WAKE;
  const working = t >= WAKE;
  const sections = useMemo(() => buildSessionSections(rows.map((r) => r.name !== target ? r
    : working ? { ...r, state: 'working' as const, label: m.news_codex_agent_task(), last_activity: Date.now() / 1000 }
      : { ...r, asleep }), 'expanded', 'none', '', NO_GROUPS), [rows, target, asleep, working]);
  const layout = reduced ? undefined : LinearTransition.duration(260);
  const enter = reduced ? undefined : FadeIn.duration(240);
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {sections.map((section) => <Animated.View key={section.id} layout={layout}>
      {section.label ? <SessionSectionHeader section={section} onToggle={noop} /> : null}
      {section.data.map((s, index) => <Animated.View key={`${section.id}::${s.name}`} layout={layout}>
        {/* A linha troca de quadro com um esmaecer curto: selo que entra, trabalho que começa. */}
        <Animated.View key={`${asleep}-${working}`} entering={s.name === target ? enter : undefined}>
          <SessionSectionCard index={index} total={section.data.length}>
            <SessionRow session={s} compact mostrarServidor={false} onPress={noop} />
          </SessionSectionCard>
        </Animated.View>
      </Animated.View>)}
    </Animated.View>)}
  </View>;
}

const styles = StyleSheet.create(theme => ({
  // Altura reservada para o quadro mais alto (com a segunda linha do trabalho): o cartão não pula entre os passos.
  stage: {
    minHeight: 232, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
}));
