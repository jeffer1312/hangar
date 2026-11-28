import { useEffect, useMemo, useRef, useState } from 'react';
import { AppState, Text, View, useWindowDimensions } from 'react-native';
import { useIsFocused } from 'expo-router/react-navigation';
import Animated, { FadeIn, FadeOut, LinearTransition, SlideOutLeft, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet } from 'react-native-unistyles';
import type { AggSession } from '@hangar/core';
import { SessionRow } from '../sessions/SessionRow';
import { SessionSectionCard, SessionSectionHeader } from '../sessions/SessionSection';
import { buildSessionSections } from '../sessions/sessionSections';
import { withAlpha } from '../config/colors';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `filhas-ligadas`: a conversa que abriu duas filhas trabalha com elas; o Fechar dela pergunta se fecha
// também as filhas; "Fechar só esta" tira só a principal, e as filhas param e ficam abertas com a marca "sem pai".
const CYCLE = 7;
const ASK = 1.3;
const CLOSE = 3.9;
const STEPS = [ASK, CLOSE];
// Movimento reduzido: o fim, com as filhas abertas e a marca de quem as abriu.
const STILL = 5;

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

/** O alerta do Fechar como o iOS o desenha: título, a pergunta e as três ações empilhadas, Cancelar por último. */
function CloseQuestion({ parent, n, width }: { parent: string; n: number; width: number }) {
  return <View style={[styles.alert, { width }]}>
    <View style={styles.alertText}>
      <Text style={styles.alertTitle}>{m.sessao_excluir()}</Text>
      <Text style={styles.alertMessage}>{m.session_close_children_question({ name: parent, n })}</Text>
    </View>
    <Text style={[styles.alertButton, styles.destructive]}>{m.session_close_only_this()}</Text>
    <Text style={[styles.alertButton, styles.destructive]}>{m.session_close_with_children({ n })}</Text>
    <Text style={[styles.alertButton, styles.cancel]}>{m.comum_cancelar()}</Text>
  </View>;
}

export function LinkedChildrenAnimation({ width }: { width: number }) {
  const reduced = useReducedMotion();
  const stage = useRef<View>(null);
  const window = useWindowDimensions();
  const [visible, setVisible] = useState(false);
  const focused = useIsFocused();
  const [active, setActive] = useState(AppState.currentState === 'active');
  useEffect(() => { const sub = AppState.addEventListener('change', (state) => setActive(state === 'active')); return () => sub.remove(); }, []);
  useEffect(() => {
    if (!focused || !active) return;
    const check = () => stage.current?.measureInWindow((_x, y, _w, height) => setVisible(height > 0 && y < window.height && y + height > 0));
    check();
    const timer = setInterval(check, 500);
    return () => clearInterval(timer);
  }, [focused, active, window.height]);
  const still = reduced || !active || !focused || !visible;
  const t = useScriptStep(STEPS, CYCLE, STILL, still);
  const rows = useMemo(sampleRows, []);
  const parent = rows[0].name;
  const asking = t >= ASK && t < CLOSE;
  const closed = t >= CLOSE;
  const sections = useMemo(() => buildSessionSections(rows
    .filter((r) => !closed || r.name !== parent)
    .map((r) => closed ? { ...r, state: 'idle' as const, label: undefined, linked_by: undefined, orphaned_from: parent } : r),
  'expanded', 'none', '', NO_GROUPS), [rows, parent, closed]);
  const layout = still ? undefined : LinearTransition.duration(260);
  const enter = still ? undefined : FadeIn.duration(240);
  return <View ref={stage} style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    {sections.map((section) => <Animated.View key={section.id} layout={layout}>
      {section.label ? <SessionSectionHeader section={section} onToggle={noop} /> : null}
      {section.data.map((s, index) => <Animated.View key={`${section.id}::${s.name}`} layout={layout}
        exiting={still ? undefined : SlideOutLeft.duration(320)}>
        {/* A linha troca de quadro com um esmaecer curto: o trabalho que para, a marca que entra. */}
        <Animated.View key={String(closed)} entering={enter}>
          <SessionSectionCard index={index} total={section.data.length}>
            <SessionRow session={s} compact mostrarServidor={false} onPress={noop} />
          </SessionSectionCard>
        </Animated.View>
      </Animated.View>)}
    </Animated.View>)}
    {asking ? <Animated.View style={styles.veil} entering={enter} exiting={still ? undefined : FadeOut.duration(150)}>
      <CloseQuestion parent={parent} n={rows.length - 1} width={Math.min(width - 32, 270)} />
    </Animated.View> : null}
  </View>;
}

const styles = StyleSheet.create(theme => ({
  // Altura reservada para o quadro mais alto (o alerta sobre as três linhas): o cartão não pula entre os passos.
  stage: {
    minHeight: 248, padding: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  veil: {
    position: 'absolute', top: 0, left: 0, right: 0, bottom: 0, alignItems: 'center', justifyContent: 'center',
    backgroundColor: withAlpha(theme.tokens.bg.base, 0.55),
  },
  alert: {
    borderRadius: theme.base.radius.lg, borderCurve: 'continuous', overflow: 'hidden', backgroundColor: theme.tokens.bg.elevated,
    borderWidth: 1, borderColor: theme.tokens.border.default,
  },
  alertText: { gap: theme.base.space[1], paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3] },
  alertTitle: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary, textAlign: 'center' },
  alertMessage: { fontSize: theme.base.text.xs, color: theme.tokens.text.secondary, textAlign: 'center' },
  // 44 pt: a altura de cada ação no alerta do iOS.
  alertButton: {
    minHeight: 44, paddingHorizontal: theme.base.space[3], textAlign: 'center', textAlignVertical: 'center', lineHeight: 44,
    fontSize: theme.base.text.sm, borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: theme.tokens.border.default,
  },
  destructive: { color: theme.tokens.status.error },
  cancel: { color: theme.tokens.accent.base, fontWeight: theme.base.weight.semibold },
}));
