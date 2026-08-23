import { Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { PlanDetail } from '@hangar/core';
import { planBadge } from '@hangar/core';
import type { SessionInfo } from '@hangar/core';

interface Props {
  session?: SessionInfo | null;
  detail?: PlanDetail | null;
  compact?: boolean;
}

export function PlanBar({ session, detail, compact = false }: Props) {
  const { theme } = useUnistyles();
  // se veio detail.tasks, usa ele; senão tenta session.plan_tasks (legado)
  const tasks = detail?.tasks ?? null;
  const badge = session ? planBadge(session) : null;
  const label = detail ? `${detail.done}/${detail.total}` : badge ? `${badge.pct.toFixed(0)}%` : '';

  // segmentada só quando tem 2..8 tasks
  const segments = tasks && tasks.length > 1 && tasks.length <= 8 ? tasks : null;

  if (!segments && !badge && !detail) return null;

  return (
    <View style={[styles.row, compact && styles.compact]}>
      <View style={[styles.bar, !segments && styles.solid]}>
        {segments ? (
          segments.map((t, idx) => (
            <View key={idx} style={styles.seg}>
              <View
                style={[
                  styles.fill,
                  { width: `${t.total > 0 ? (t.done / t.total) * 100 : 0}%` },
                  t.total > 0 && t.done >= t.total ? { backgroundColor: theme.tokens.status.success } : { backgroundColor: theme.tokens.accent.base },
                ]}
              />
            </View>
          ))
        ) : (
          <View style={styles.seg}>
            <View style={[styles.fill, { width: `${badge?.pct ?? (detail ? (detail.done / detail.total) * 100 : 0)}%`, backgroundColor: badge?.complete || detail?.complete ? theme.tokens.status.success : theme.tokens.accent.base }]} />
          </View>
        )}
      </View>
      {!compact && label ? <Text style={[styles.lbl, { color: theme.tokens.text.muted }]}>{detail ? `${detail.done}/${detail.total}` : label}</Text> : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], marginTop: theme.base.space[1] },
  compact: { position: 'absolute', left: theme.base.space[1], right: theme.base.space[1], bottom: 2, marginTop: 0 },
  bar: { flex: 1, flexDirection: 'row', gap: 3, minWidth: 0 },
  solid: { gap: 0 },
  seg: { flex: 1, height: 5, borderRadius: theme.base.radius.full, backgroundColor: theme.tokens.bg.elevated, overflow: 'hidden' },
  fill: { height: '100%', borderRadius: theme.base.radius.full },
  lbl: { fontSize: 10, fontVariant: ['tabular-nums'], flexShrink: 0 },
}));
