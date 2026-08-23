import { Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { WorkflowDetail } from '@hangar/core';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { mkMarkdownStyle } from '../../chat/AssistantBubble';
import * as m from '../../paraglide/messages';

interface Props {
  detail: WorkflowDetail | null;
  loading: boolean;
  onAgent: (agentId: string) => void;
}

function fmtTokens(n: number): string {
  if (!n) return '0';
  return n < 1000 ? String(n) : `${(n / 1000).toFixed(1)}k`;
}
function fmtDur(ms: number): string {
  if (!ms) return '';
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
}
function stateGlyph(s: string | null): string {
  if (s === 'done') return '✓';
  if (s === 'error') return '✕';
  return '⟳';
}

export function WorkflowDetailView({ detail, loading, onAgent }: Props) {
  const { theme } = useUnistyles();
  if (loading) {
    return <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>;
  }
  if (!detail) {
    return <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.atividade_run_nao_encontrado()}</Text>;
  }
  const md = mkMarkdownStyle(theme);
  return (
    <ScrollView contentContainerStyle={styles.body}>
      <View style={styles.metrics}>
        <Text style={[styles.metric, { color: theme.tokens.text.muted }]}>
          <Text style={{ color: theme.tokens.text.primary, fontWeight: '600' }}>{detail.agents.length}</Text> {m.atividade_agentes()}
        </Text>
        <Text style={[styles.metric, { color: theme.tokens.text.muted }]}>
          <Text style={{ color: theme.tokens.text.primary, fontWeight: '600' }}>{fmtTokens(detail.totalTokens)}</Text> {m.ctx_tokens()}
        </Text>
        {detail.phases.length ? (
          <Text style={[styles.metric, { color: theme.tokens.text.muted }]}>
            <Text style={{ color: theme.tokens.text.primary, fontWeight: '600' }}>{detail.phases.length}</Text> {m.atividade_fases_curto()}
          </Text>
        ) : null}
        {detail.durationMs ? <Text style={[styles.metric, { color: theme.tokens.text.muted }]}>{fmtDur(detail.durationMs)}</Text> : null}
      </View>
      {detail.agents.map((a, idx) => (
        <Pressable
          key={a.agentId ?? String(idx)}
          onPress={() => a.agentId && onAgent(a.agentId)}
          disabled={!a.agentId}
          style={[styles.agent, { borderBottomColor: theme.tokens.border.subtle }]}
          accessibilityRole="button"
        >
          <View style={styles.agentTop}>
            <Text style={[styles.state, a.state === 'done' ? { color: theme.tokens.status.success } : a.state === 'error' ? { color: theme.tokens.status.error } : { color: theme.tokens.accent.base }]}>{stateGlyph(a.state)}</Text>
            <Text style={[styles.agentLabel, { color: theme.tokens.text.primary }]} numberOfLines={1}>
              {a.label ?? 'agente'}
            </Text>
            {a.agentId ? <Text style={[styles.chevron, { color: theme.tokens.text.muted }]}>›</Text> : null}
          </View>
          <View style={styles.agentMeta}>
            <Text style={[styles.metaTxt, { color: theme.tokens.text.muted }]}>
              {fmtTokens(a.tokens)} {m.atividade_tok()}
              {a.toolCalls ? ` · ${a.toolCalls} ${m.atividade_tools()}` : ''}
              {a.lastToolName ? ` · ${a.lastToolName}` : ''}
              {a.durationMs ? ` · ${fmtDur(a.durationMs)}` : ''}
            </Text>
          </View>
        </Pressable>
      ))}
      {detail.summary ? (
        <View style={styles.summary}>
          <Text style={[styles.sectionLabel, { color: theme.tokens.text.muted }]}>{m.atividade_resumo()}</Text>
          <EnrichedMarkdownText markdown={detail.summary} markdownStyle={md} flavor="github" />
        </View>
      ) : null}
    </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  body: { gap: theme.base.space[3], paddingBottom: theme.base.space[4] },
  empty: { fontSize: theme.base.text.sm, textAlign: 'center', paddingVertical: theme.base.space[4] },
  metrics: { flexDirection: 'row', gap: theme.base.space[3], flexWrap: 'wrap' },
  metric: { fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'] },
  sectionLabel: { fontSize: theme.base.text.xs, textTransform: 'uppercase', letterSpacing: 0.6, fontWeight: '600' },
  agent: { paddingVertical: theme.base.space[2], borderBottomWidth: 1, gap: 2 },
  agentTop: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  state: { width: 18, textAlign: 'center', fontSize: theme.base.text.sm },
  agentLabel: { flex: 1, fontSize: theme.base.text.sm, fontWeight: '500', minWidth: 0 },
  chevron: { fontSize: theme.base.text.base },
  agentMeta: { paddingLeft: 18 + 8 },
  metaTxt: { fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'] },
  summary: { gap: theme.base.space[1], paddingTop: theme.base.space[2] },
}));
