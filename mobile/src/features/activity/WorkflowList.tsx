import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { WorkflowSummary } from '@hangar/core';
import * as m from '../../paraglide/messages';

interface Props {
  workflows: WorkflowSummary[];
  onSelect: (runId: string) => void;
}

function fmtTokens(n: number): string {
  if (!n) return '0';
  return n < 1000 ? String(n) : `${(n / 1000).toFixed(1)}k`;
}

export function WorkflowList({ workflows, onSelect }: Props) {
  const { theme } = useUnistyles();
  if (!workflows.length) return null;
  return (
    <View style={styles.section}>
      <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.atividade_workflows()}</Text>
      {workflows.map((w) => (
        <Pressable
          key={w.runId}
          onPress={() => onSelect(w.runId)}
          style={[styles.card, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.subtle }]}
          accessibilityRole="button"
          accessibilityLabel={w.name}
        >
          <View style={[styles.icon, { backgroundColor: w.running ? 'transparent' : theme.tokens.bg.elevated }]}>
            <Text style={[styles.iconText, { color: w.running ? theme.tokens.accent.base : theme.tokens.status.success }]}>{w.running ? '◐' : '✓'}</Text>
          </View>
          <View style={styles.body}>
            <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>
              {w.name}
            </Text>
            <Text style={[styles.meta, { color: theme.tokens.text.muted }]} numberOfLines={1}>
              {w.agentCount} {m.atividade_agentes()} · {fmtTokens(w.totalTokens)} {m.ctx_tokens()}
              {w.phaseCount ? ` · ${w.phaseCount} ${m.atividade_fases_curto()}` : ''}
            </Text>
          </View>
          <View style={[styles.badge, { backgroundColor: theme.tokens.bg.elevated }]}>
            <Text style={[styles.badgeText, { color: w.running ? theme.tokens.accent.base : theme.tokens.text.muted }]}>{w.running ? m.atividade_rodando() : m.atividade_concluido()}</Text>
          </View>
          <Text style={[styles.chevron, { color: theme.tokens.text.muted }]}>›</Text>
        </Pressable>
      ))}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  section: {
    gap: theme.base.space[2],
  },
  label: {
    fontSize: theme.base.text.xs,
    textTransform: 'uppercase',
    letterSpacing: 0.6,
    fontWeight: '600',
  },
  card: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    padding: theme.base.space[3],
    borderWidth: 1,
    borderRadius: theme.base.radius.lg,
  },
  icon: {
    width: 28,
    height: 28,
    borderRadius: 14,
    alignItems: 'center',
    justifyContent: 'center',
  },
  iconText: {
    fontSize: 13,
    fontWeight: '700',
  },
  body: {
    flex: 1,
    gap: 2,
    minWidth: 0,
  },
  name: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
  meta: {
    fontSize: theme.base.text.xs,
    fontVariant: ['tabular-nums'],
  },
  badge: {
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 1,
    borderRadius: theme.base.radius.full,
  },
  badgeText: {
    fontSize: 10,
    fontWeight: '600',
  },
  chevron: {
    fontSize: theme.base.text.lg,
    lineHeight: theme.base.text.lg,
  },
}));
