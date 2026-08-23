import { useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { Stack, useLocalSearchParams } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { getSubagents, getWorkflow, getWorkflowAgent, getWorkflows, planBadge } from '@hangar/core';
import type { SubagentRun, WorkflowDetail, WorkflowAgentDetail, WorkflowSummary } from '@hangar/core';
import { chatStore } from '../../../../src/stores/chat';
import { useServers } from '../../../../src/stores/servers';
import { useSessions } from '../../../../src/stores/sessions';
import { useActivity } from '../../../../src/features/activity/useActivity';
import { TaskRows } from '../../../../src/features/activity/TaskRows';
import { WorkflowList } from '../../../../src/features/activity/WorkflowList';
import { WorkflowDetailView } from '../../../../src/features/activity/WorkflowDetailView';
import { AgentDetailView } from '../../../../src/features/activity/AgentDetailView';
import { SubagentLive } from '../../../../src/features/activity/SubagentLive';
import { PlanPanel } from '../../../../src/features/plan/PlanPanel';
import * as m from '../../../../src/paraglide/messages';

type Level = 'list' | 'workflow' | 'agent' | 'subagent';

export default function ActivitySheet() {
  const { theme } = useUnistyles();
  const params = useLocalSearchParams<{ server: string; name: string }>();
  const serverId = Array.isArray(params.server) ? params.server[0] : (params.server ?? '');
  const name = Array.isArray(params.name) ? params.name[0] : (params.name ?? '');

  const chat = chatStore(serverId, name);
  const events = chat.use((s) => s.events);
  const activity = useActivity(events);

  const session = useSessions((s: any) => s.rows.find((r: any) => r.serverId === serverId && r.name === name) ?? null);
  const server = useServers((s: any) => s.servers.find((x: any) => x.id === serverId) ?? null);

  const [level, setLevel] = useState<Level>('list');
  const [runId, setRunId] = useState<string | null>(null);
  const [workflows, setWorkflows] = useState<WorkflowSummary[]>([]);
  const [subs, setSubs] = useState<SubagentRun[]>([]);
  const [subError, setSubError] = useState('');
  const [wfLoading, setWfLoading] = useState(false);
  const [detail, setDetail] = useState<WorkflowDetail | null>(null);
  const [agentDetail, setAgentDetail] = useState<WorkflowAgentDetail | null>(null);
  const [agentLoading, setAgentLoading] = useState(false);
  const [subAgentId, setSubAgentId] = useState<string | null>(null);
  const [subTitle, setSubTitle] = useState('');
  const genWf = useRef(0);
  const genAgent = useRef(0);

  const hasPlan = !!planBadge(session);

  const fetchAll = useCallback(async () => {
    setWfLoading(true);
    try {
      const w = await getWorkflows(name);
      setWorkflows(w);
    } catch {
      // workflow sem erro visível separado; segue vazio
    } finally {
      setWfLoading(false);
    }
    try {
      const s = await getSubagents(name);
      setSubs(s);
      setSubError('');
    } catch {
      setSubError(m.atividade_erro_subagentes());
    }
  }, [name]);

  useEffect(() => {
    setLevel('list');
    setRunId(null);
    setDetail(null);
    setAgentDetail(null);
    setSubAgentId(null);
    setSubTitle('');
    void fetchAll();
  }, [fetchAll, name, serverId]);

  // helpers de matching (casamento por prompt, como no frontend)
  const norm = (t: string) => t.replace(/\s+/g, ' ').trim().toLowerCase();
  const matchSub = useCallback(
    (prompt?: string): SubagentRun | null => {
      if (!prompt) return null;
      const full = norm(prompt);
      const exact = subs.find((s) => s.prompt && norm(s.prompt) === full);
      if (exact) return exact;
      const head = full.slice(0, 400);
      return subs.find((s) => s.prompt && norm(s.prompt).startsWith(head)) ?? null;
    },
    [subs],
  );

  const orfaos = subs.filter((s2) => !activity.agents.some((a: any) => matchSub(a.prompt)?.agentId === s2.agentId));
  const runningAgents = activity.agents.filter((a: any) => a.kind === 'agent' && a.running);

  function tituloDoSub(s2: SubagentRun): string {
    const linhas = (s2.prompt ?? '').split('\n').map((l) => l.trim()).filter(Boolean);
    const util = linhas.find((l) => !/^base directory for this skill:/i.test(l) && !l.startsWith('#'));
    return (util ?? linhas[0] ?? s2.agentId).slice(0, 90);
  }

  async function openWorkflow(rid: string) {
    const g = ++genWf.current;
    setRunId(rid);
    setLevel('workflow');
    setDetail(null);
    setWfLoading(true);
    try {
      const d = await getWorkflow(name, rid);
      if (g !== genWf.current) return;
      setDetail(d);
    } catch {
      if (g !== genWf.current) return;
      setDetail(null);
    } finally {
      if (g === genWf.current) setWfLoading(false);
    }
  }

  async function openAgent(agentId: string | null) {
    if (!runId || !agentId) return;
    const g = ++genAgent.current;
    setLevel('agent');
    setAgentDetail(null);
    setAgentLoading(true);
    try {
      const d = await getWorkflowAgent(name, runId, agentId);
      if (g !== genAgent.current) return;
      setAgentDetail(d);
    } catch {
      if (g !== genAgent.current) return;
      setAgentDetail(null);
    } finally {
      if (g === genAgent.current) setAgentLoading(false);
    }
  }

  function abrirDoDisco(s2: SubagentRun) {
    setSubTitle(tituloDoSub(s2));
    setSubAgentId(s2.agentId);
    setLevel('subagent');
  }

  function openSubagent(prompt: string | undefined, title: string) {
    const match = matchSub(prompt);
    if (!match) return;
    setSubTitle(title);
    setSubAgentId(match.agentId);
    setLevel('subagent');
  }

  function back() {
    if (level === 'subagent') {
      setSubAgentId(null);
      setSubTitle('');
      setLevel('list');
      return;
    }
    if (level === 'agent') {
      setLevel('workflow');
      setAgentDetail(null);
      return;
    }
    if (level === 'workflow') {
      setLevel('list');
      setDetail(null);
      setRunId(null);
    }
  }

  const headerTitle = level === 'list' ? m.ctx_atividade() : level === 'subagent' ? subTitle || m.atividade_subagente() : level === 'workflow' ? (detail?.name ?? m.atividade_workflow()) : (agentDetail?.label ?? m.atividade_agente());
  const countBadge = level === 'list' && activity.total > 0 ? `${activity.done}/${activity.total}` : null;
  const wfBadge = level === 'workflow' && detail ? detail.status : null;

  if (!server) {
    return (
      <>
        <Stack.Screen options={{ headerShown: false, sheetAllowedDetents: [0.92], sheetGrabberVisible: true, contentStyle: { backgroundColor: 'transparent' } }} />
        <View style={[styles.root, { backgroundColor: theme.tokens.bg.base }]}>
          <Text style={[styles.error, { color: theme.tokens.status.error }]}>{m.compare_servidor_nao_encontrado()}</Text>
        </View>
      </>
    );
  }

  return (
    <>
      <Stack.Screen options={{ headerShown: false, sheetAllowedDetents: [0.92], sheetGrabberVisible: true, contentStyle: { backgroundColor: 'transparent' } }} />
      <View style={[styles.root, { backgroundColor: theme.tokens.bg.base }]}>
        <View style={[styles.header, { borderBottomColor: theme.tokens.border.subtle }]}>
          {level !== 'list' ? (
            <Pressable onPress={back} style={styles.headBtn} accessibilityLabel={m.comum_voltar()} accessibilityRole="button">
              <Text style={[styles.headIcon, { color: theme.tokens.text.secondary }]}>‹</Text>
            </Pressable>
          ) : null}
          <Text style={[styles.title, { color: theme.tokens.text.primary }]} numberOfLines={1}>
            {headerTitle}
          </Text>
          {countBadge ? <Text style={[styles.count, { color: theme.tokens.text.secondary }]}>{countBadge}</Text> : null}
          {wfBadge ? <Text style={[styles.wfStatus, wfBadge === 'completed' ? { color: theme.tokens.status.success } : wfBadge === 'running' ? { color: theme.tokens.accent.base } : { color: theme.tokens.text.secondary }]}>{wfBadge}</Text> : null}
        </View>

        {level === 'list' ? (
          <ScrollView contentContainerStyle={styles.body} showsVerticalScrollIndicator={false}>
            {hasPlan ? (
              <View style={styles.section}>
                <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.ctx_plano()}</Text>
                <PlanPanel session={session} name={name} />
              </View>
            ) : null}

            {workflows.length > 0 ? <WorkflowList workflows={workflows} onSelect={openWorkflow} /> : null}
            {wfLoading && workflows.length === 0 ? (
              <View style={styles.center}>
                <ActivityIndicator color={theme.tokens.text.muted} />
                <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
              </View>
            ) : null}

            {runningAgents.length > 0 ? (
              <View style={styles.section}>
                <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.atividade_rodando_agora()}</Text>
                {runningAgents.map((a) => {
                  const sub = matchSub(a.prompt);
                  const isButton = !!sub;
                  const Row = isButton ? Pressable : View;
                  return (
                    // @ts-ignore — Pressable/View condicionais
                    <Row
                      key={a.id}
                      onPress={isButton ? () => openSubagent(a.prompt, a.description) : undefined}
                      style={[styles.agentRow, isButton && styles.agentOpenable]}
                      accessibilityRole={isButton ? 'button' : undefined}
                    >
                      <Text style={[styles.spin, { color: theme.tokens.accent.base }]}>◐</Text>
                      <View style={styles.agentBody}>
                        <View style={styles.agentHead}>
                          <Text style={[styles.agentDesc, { color: theme.tokens.text.primary }]} numberOfLines={1}>
                            {a.description}
                          </Text>
                          {a.subagentType ? <Text style={[styles.tag, { color: theme.tokens.text.secondary, backgroundColor: theme.tokens.bg.elevated }]}>{a.subagentType}</Text> : null}
                          {a.model ? <Text style={[styles.tag, styles.tagMuted, { color: theme.tokens.text.muted }]}>{a.model}</Text> : null}
                        </View>
                        {sub ? (
                          <Text style={[styles.agentNow, { color: theme.tokens.text.muted }]} numberOfLines={1}>
                            {m.atividade_chamadas({ n: sub.toolCalls })}
                            {sub.recent.length ? ` · ${sub.recent[sub.recent.length - 1]?.name ?? ''}` : ''}
                          </Text>
                        ) : null}
                      </View>
                      {sub ? <Text style={[styles.chevron, { color: theme.tokens.text.muted }]}>›</Text> : null}
                    </Row>
                  );
                })}
              </View>
            ) : null}

            {orfaos.length > 0 ? (
              <View style={styles.section}>
                <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.atividade_subagentes()}</Text>
                {orfaos.map((s2) => (
                  <Pressable key={s2.agentId} onPress={() => abrirDoDisco(s2)} style={[styles.agentRow, styles.agentOpenable]} accessibilityRole="button">
                    <View style={styles.agentBody}>
                      <View style={styles.agentHead}>
                        <Text style={[styles.agentDesc, { color: theme.tokens.text.primary }]} numberOfLines={1}>
                          {tituloDoSub(s2)}
                        </Text>
                        {s2.agentType ? <Text style={[styles.tag, { color: theme.tokens.text.secondary, backgroundColor: theme.tokens.bg.elevated }]}>{s2.agentType}</Text> : null}
                      </View>
                      <Text style={[styles.agentNow, { color: theme.tokens.text.muted }]} numberOfLines={1}>
                        {m.atividade_chamadas({ n: s2.toolCalls })}
                        {s2.recent.length ? ` · ${s2.recent[s2.recent.length - 1]?.name ?? ''}` : ''}
                      </Text>
                    </View>
                    <Text style={[styles.chevron, { color: theme.tokens.text.muted }]}>›</Text>
                  </Pressable>
                ))}
              </View>
            ) : null}

            {activity.tasks.length > 0 ? <TaskRows tasks={activity.tasks} /> : null}
            {subError ? <Text style={[styles.err, { color: theme.tokens.status.warning }]}>⚠ {subError}</Text> : null}
            {workflows.length === 0 && activity.tasks.length === 0 && runningAgents.length === 0 && orfaos.length === 0 && !subError ? <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.atividade_vazio()}</Text> : null}
          </ScrollView>
        ) : level === 'workflow' ? (
          <WorkflowDetailView detail={detail} loading={wfLoading} onAgent={openAgent} />
        ) : level === 'agent' ? (
          <AgentDetailView detail={agentDetail} loading={agentLoading} />
        ) : level === 'subagent' && subAgentId ? (
          <SubagentLive sessionName={name} agentId={subAgentId} />
        ) : (
          <View style={styles.center}>
            <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.atividade_sem_transcript()}</Text>
          </View>
        )}
      </View>
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: { flex: 1 },
  header: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], borderBottomWidth: 1 },
  headBtn: { width: 32, height: 32, alignItems: 'center', justifyContent: 'center' },
  headIcon: { fontSize: 22, lineHeight: 22 },
  title: { flex: 1, fontSize: theme.base.text.base, fontWeight: '600', minWidth: 0 },
  count: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.sm, fontVariant: ['tabular-nums'] },
  wfStatus: { fontSize: theme.base.text.xs, fontWeight: '600', paddingHorizontal: theme.base.space[2], paddingVertical: 2, borderRadius: theme.base.radius.full, overflow: 'hidden' },
  body: { padding: theme.base.space[4], gap: theme.base.space[4], paddingBottom: theme.base.space[6] },
  section: { gap: theme.base.space[2] },
  label: { fontSize: theme.base.text.xs, textTransform: 'uppercase', letterSpacing: 0.6, fontWeight: '600' },
  center: { alignItems: 'center', gap: theme.base.space[2], paddingVertical: theme.base.space[4] },
  muted: { fontSize: theme.base.text.sm, textAlign: 'center' },
  empty: { fontSize: theme.base.text.sm, textAlign: 'center', paddingVertical: theme.base.space[4] },
  err: { fontSize: theme.base.text.xs, marginTop: theme.base.space[2] },
  agentRow: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[2], paddingVertical: theme.base.space[1] },
  agentOpenable: { paddingHorizontal: theme.base.space[1], marginHorizontal: -theme.base.space[1], borderRadius: theme.base.radius.sm },
  spin: { fontSize: theme.base.text.sm, marginTop: 2 },
  agentBody: { flex: 1, gap: 3, minWidth: 0 },
  agentHead: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minWidth: 0 },
  agentDesc: { fontSize: theme.base.text.sm, minWidth: 0, flexShrink: 1 },
  tag: { fontFamily: theme.base.fontMono, fontSize: 10, paddingHorizontal: 6, paddingVertical: 1, borderRadius: theme.base.radius.full, overflow: 'hidden' },
  tagMuted: {},
  agentNow: { fontFamily: theme.base.fontMono, fontSize: 10 },
  chevron: { fontSize: theme.base.text.base, lineHeight: theme.base.text.base },
  error: { fontSize: theme.base.text.sm, textAlign: 'center', padding: theme.base.space[4] },
}));
