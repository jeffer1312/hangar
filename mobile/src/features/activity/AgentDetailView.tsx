import { ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { WorkflowAgentDetail } from '@hangar/core';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { mkMarkdownStyle } from '../../chat/AssistantBubble';
import * as m from '../../paraglide/messages';

interface Props {
  detail: WorkflowAgentDetail | null;
  loading: boolean;
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

export function AgentDetailView({ detail, loading }: Props) {
  const { theme } = useUnistyles();
  if (loading) {
    return <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>;
  }
  if (!detail) {
    return <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.atividade_agente_nao_encontrado()}</Text>;
  }
  const md = mkMarkdownStyle(theme);
  return (
    <ScrollView contentContainerStyle={styles.body}>
      <View style={styles.meta}>
        {detail.state === 'done' ? <Text style={[styles.badge, { color: theme.tokens.status.success }]}>✓ {m.atividade_concluido()}</Text> : null}
        {detail.state === 'progress' ? <Text style={[styles.badge, { color: theme.tokens.accent.base }]}>◐ {m.atividade_rodando()}</Text> : null}
        {detail.state === 'error' ? <Text style={[styles.badge, { color: theme.tokens.status.error }]}>✕ {m.term_erro()}</Text> : null}
        <Text style={[styles.metaTxt, { color: theme.tokens.text.muted }]}>
          {fmtTokens(detail.tokens)} {m.ctx_tokens()}
        </Text>
        <Text style={[styles.metaTxt, { color: theme.tokens.text.muted }]}>
          {detail.toolCalls} {m.atividade_tools()}
        </Text>
        {detail.durationMs ? <Text style={[styles.metaTxt, { color: theme.tokens.text.muted }]}>{fmtDur(detail.durationMs)}</Text> : null}
        {detail.model ? <Text style={[styles.metaTxt, { color: theme.tokens.text.muted }]}>{detail.model}</Text> : null}
      </View>
      {detail.prompt ? (
        <View style={styles.block}>
          <Text style={[styles.rotulo, { color: theme.tokens.text.muted }]}>{m.atividade_prompt()}</Text>
          <View style={[styles.bubble, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.subtle }]}>
            <EnrichedMarkdownText markdown={detail.prompt} markdownStyle={md} flavor="github" />
          </View>
        </View>
      ) : null}
      {detail.tools.length > 0 ? (
        <View style={styles.block}>
          <Text style={[styles.rotulo, { color: theme.tokens.text.muted }]}>{m.atividade_ferramentas_chamadas({ n: detail.toolCalls })}</Text>
          <View style={styles.chips}>
            {detail.tools.map((t) => (
              <View key={t.name} style={[styles.chip, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]}>
                <Text style={[styles.chipTxt, { color: theme.tokens.text.secondary }]}>
                  {t.name} ×{t.count}
                </Text>
              </View>
            ))}
          </View>
          {detail.state === 'progress' && detail.lastToolName ? (
            <Text style={[styles.now, { color: theme.tokens.text.secondary }]}>
              {detail.lastToolName}
              {detail.lastToolTarget ? ` — ${detail.lastToolTarget}` : ''}
            </Text>
          ) : null}
        </View>
      ) : null}
      {detail.result ? (
        <View style={styles.block}>
          <Text style={[styles.rotulo, { color: theme.tokens.text.muted }]}>{m.atividade_resultado()}</Text>
          <EnrichedMarkdownText markdown={detail.result} markdownStyle={md} flavor="github" />
        </View>
      ) : detail.state === 'progress' && detail.toolCalls === 0 ? (
        <View style={styles.block}>
          <Text style={[styles.rotulo, { color: theme.tokens.text.muted }]}>{m.atividade_resultado()}</Text>
          <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>◐ {m.atividade_pensando()}</Text>
        </View>
      ) : null}
      <Text style={[styles.rodape, { color: theme.tokens.text.muted, borderTopColor: theme.tokens.border.subtle }]}>{m.atividade_conversa_so_leitura({ nome: detail.label })}</Text>
    </ScrollView>
  );
}

const styles = StyleSheet.create((theme) => ({
  body: { gap: theme.base.space[3], paddingBottom: theme.base.space[4] },
  empty: { fontSize: theme.base.text.sm, textAlign: 'center', paddingVertical: theme.base.space[4] },
  meta: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[3], paddingVertical: theme.base.space[2], borderBottomWidth: 1, borderBottomColor: theme.tokens.border.subtle },
  badge: { fontSize: theme.base.text.xs, fontWeight: '600' },
  metaTxt: { fontSize: theme.base.text.xs },
  block: { gap: theme.base.space[2] },
  rotulo: { fontSize: 10.5, letterSpacing: 0.8, textTransform: 'uppercase', fontWeight: '600' },
  bubble: { padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderWidth: 1 },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[1] },
  chip: { paddingHorizontal: theme.base.space[2], paddingVertical: 3, borderRadius: theme.base.radius.sm, borderWidth: 1 },
  chipTxt: { fontFamily: theme.base.fontMono, fontSize: 11 },
  now: { fontSize: theme.base.text.xs, marginTop: theme.base.space[1] },
  rodape: { textAlign: 'center', fontSize: 11.5, fontStyle: 'italic', borderTopWidth: 1, paddingTop: theme.base.space[3], marginTop: theme.base.space[2] },
}));
