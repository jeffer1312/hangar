import { Text, View } from 'react-native';
import Animated, { FadeIn, LinearTransition, useReducedMotion } from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { tagHex } from '@hangar/core';
import { SessionAvatar } from '../sessions/SessionAvatar';
import { SessionPlate, type PlateAccount, type PlateMachine } from '../sessions/SessionPlate';
import { MACHINE_ICON_NAME } from '../tags/tagVisuals';
import { superficie } from '../../theme/superficie';
import { useScriptStep } from './scriptStep';
import * as m from '../../paraglide/messages';

// Roteiro do `etiquetas-maquina-conta`: as placas entram nas duas linhas (só a conta na sessão desta máquina,
// máquina e conta na outra) e o anel de contexto da primeira enche até o aviso.
const CYCLE = 6;
const PLATE_1 = 0.4;
const PLATE_2 = 0.9;
const RING_FROM = 1.4;
const RING_STEP = 0.25;
const RING_STEPS = 8;
const RING_START = 15;
const RING_END = 70;
const STEPS = [PLATE_1, PLATE_2, ...Array.from({ length: RING_STEPS }, (_, i) => RING_FROM + i * RING_STEP)];
// Movimento reduzido: as duas placas à mostra, o anel parado perto do aviso.
const STILL = CYCLE - 0.01;
const STILL_PCT = 60;

export function ringAt(t: number): number {
  const k = Math.min(RING_STEPS, Math.max(0, Math.floor((t - RING_FROM) / RING_STEP) + 1));
  return RING_START + ((RING_END - RING_START) * k) / RING_STEPS;
}

const avatar = { provider: 'claude', engine: null, limited: false, status_line: null, context: null, state: 'idle' } as const;

function Row({ name, pct, plate, machine, account, reduced }: {
  name: string; pct: number | null; plate: boolean; machine: PlateMachine | null; account: PlateAccount; reduced: boolean;
}) {
  const { theme } = useUnistyles();
  const enter = reduced ? undefined : FadeIn.duration(260);
  const layout = reduced ? undefined : LinearTransition.duration(220);
  return <Animated.View style={styles.row} layout={layout}>
    <View style={styles.lead}><SessionAvatar session={avatar} ctxPct={pct} /></View>
    <View style={styles.col}>
      <Text style={[styles.nome, { color: theme.tokens.text.primary }]} numberOfLines={1}>{name}</Text>
      {plate ? <Animated.View style={styles.metaRow} entering={enter}>
        <SessionPlate machine={machine} account={account} provider="claude" />
      </Animated.View> : null}
    </View>
  </Animated.View>;
}

export function TagsPlateAnimation({ width }: { width: number }) {
  const { theme } = useUnistyles();
  const reduced = useReducedMotion();
  const t = useScriptStep(STEPS, CYCLE, STILL, reduced);
  const fallback = theme.tokens.text.secondary;
  const work = m.news_tags_sample_account_work();
  const personal = m.news_tags_sample_account_personal();
  return <View style={[styles.stage, { width }]} pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <Row name={m.news_tags_sample_session()} pct={reduced ? STILL_PCT : ringAt(t)} plate={t >= PLATE_1} machine={null}
      account={{ label: work, name: work, color: tagHex('blue') ?? fallback, icon: 'credit-card' }} reduced={reduced} />
    <Row name={m.news_tags_sample_session_other()} pct={35} plate={t >= PLATE_2}
      machine={{ label: m.news_tags_sample_machine(), color: tagHex('violet') ?? fallback, icon: MACHINE_ICON_NAME.server }}
      account={{ label: personal, name: personal, color: tagHex('green') ?? fallback, icon: 'user-round' }} reduced={reduced} />
  </View>;
}

// Medidas da linha da lista (SessionRow), para a miniatura ter o tamanho dela.
const styles = StyleSheet.create(theme => ({
  stage: {
    gap: theme.base.space[1], paddingVertical: theme.base.space[2], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    borderWidth: 1, borderColor: theme.tokens.border.default, backgroundColor: superficie(theme, 0.6), overflow: 'hidden',
  },
  row: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[3], minHeight: 56, paddingVertical: theme.base.space[2], paddingHorizontal: theme.base.space[3] },
  lead: { width: 36, alignItems: 'center', paddingTop: 1 },
  col: { flex: 1, gap: theme.base.space[0.5], minWidth: 0 },
  nome: { fontSize: theme.base.text.base, lineHeight: 22, fontWeight: theme.base.weight.semibold, flexShrink: 1 },
  metaRow: { flexDirection: 'row', alignItems: 'center', minWidth: 0 },
}));
