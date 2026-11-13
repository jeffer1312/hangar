import { useCallback, useEffect, useMemo, useReducer, useRef, useState, type ReactElement } from 'react';
import { AccessibilityInfo, ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { codexSubagentActivityKey, codexSubagentState, formatElapsed, getSubagents, getWorkflow, getWorkflowAgent, getWorkflows, projectSubagentActivity, projectCodexSubagent, isCodexAgentSpawn, stopTask } from '@hangar/core';
import type { AgentRun, CodexCommand, ShellVivo, SubagentRun, WorkflowDetail, WorkflowAgentDetail, WorkflowSummary } from '@hangar/core';
import { chatStore } from '../../stores/chat';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { useActivity } from './useActivity';
import { ActivityTimeout, useActivityRequest } from './useActivityRequest';
import { TaskRows } from './TaskRows';
import { WorkflowList } from './WorkflowList';
import { WorkflowDetailView } from './WorkflowDetailView';
import { AgentDetailView } from './AgentDetailView';
import { agentIdentities, agentModel, matchAgentSubagent } from './agentModel';
import { SubagentLive, subagentProblemText, subagentStatus } from './SubagentLive';
import * as m from '../../paraglide/messages';
import { superficie } from '../../theme/superficie';
import { HeaderBack } from '../../ui/glassHeader';
import { Icon, type IconName } from '../../ui/Icon';
import { useAparencia } from '../../stores/aparencia';
import { haptic } from '../../ui/haptics';
import { toast } from '../../ui/Toast';
import { CodexCommands } from './CodexCommands';

type Level = 'list' | 'workflow' | 'agent' | 'subagent';
const EMPTY_SHELLS: ShellVivo[] = [];
const EMPTY_COMMANDS: CodexCommand[] = [];
// O 202 do servidor não prova a parada: só a tarefa sumir da lista. Sem isso neste prazo, a linha volta.
const STOP_CONFIRM_MS = 15_000;

interface Props {
  serverId: string;
  name: string;
  focused: boolean;
  onBackRequest?: (handler: () => boolean) => void;
  onClose?: () => void;
}

export function ActivityPanel({ serverId, name, focused, onBackRequest, onClose }: Props) {
  const { theme } = useUnistyles();
  const insets = useSafeAreaInsets();
  const readActivity = useActivityRequest();
  const listKey = JSON.stringify([serverId, name]);

  const chat = chatStore(serverId, name);
  const events = chat.use((s) => s.events);
  const eventsRevision = chat.use((s) => s.eventsRevision);
  // Processos vivos lidos do sistema pelo backend (vêm no `state` do stream que o chat já mantém).
  const processos = chat.use((s) => s.stateEvent?.shells) ?? EMPTY_SHELLS;
  const commands = chat.use((s) => s.stateEvent?.codex_commands) ?? EMPTY_COMMANDS;
  const nativeStates = chat.use((s) => s.stateEvent?.codex_subagents);
  const readOnly = useSessions(state => state.rows.find(row => row.serverId === serverId && row.name === name)?.guest_kind === 'pair');
  const provider = useSessions(state => state.rows.find(row => row.serverId === serverId && row.name === name)?.provider);
  const transcriptActivity = useActivity(events, eventsRevision);
  const agentIds = useMemo(() => agentIdentities(events), [events, eventsRevision]);
  // Relógio do "há N" dos shells enquanto o painel está visível.
  const [agora, setAgora] = useState(Date.now() / 1000);
  useEffect(() => {
    if (!focused) return;
    const t = setInterval(() => setAgora(Date.now() / 1000), 20_000);
    return () => clearInterval(t);
  }, [focused]);

  const server = useServers((s: any) => s.servers.find((x: any) => x.id === serverId) ?? null);

  const [level, setLevel] = useState<Level>('list');
  const [runId, setRunId] = useState<string | null>(null);
  const [list, setList] = useState<{ key: string; workflows: WorkflowSummary[]; subs: SubagentRun[]; workflowLoading: boolean; subLoading: boolean; workflowError: string; subError: string }>({ key: listKey, workflows: [], subs: [], workflowLoading: true, subLoading: true, workflowError: '', subError: '' });
  const currentList = list.key === listKey;
  const workflows = currentList ? list.workflows : [];
  const subs = useMemo(() => currentList ? list.subs.map(run => projectCodexSubagent(run, nativeStates)) : [], [currentList, list.subs, nativeStates]);
  const nativeSubagents = nativeStates !== undefined || events.some(event => event.kind === 'tool_use' && isCodexAgentSpawn(event.tool_name ?? ''))
    || subs.some(sub => !!sub.agentPath);
  const nativeIds = codexSubagentActivityKey(nativeStates);
  const activity = useMemo(() => projectSubagentActivity(transcriptActivity, subs, agentIds, nativeStates), [transcriptActivity, subs, agentIds, nativeStates]);
  const listLoading = !currentList || ((list.workflowLoading || list.subLoading) && workflows.length === 0 && subs.length === 0);
  const workflowListError = currentList ? list.workflowError : '';
  const subError = currentList ? list.subError : '';
  const [olderOpen, setOlderOpen] = useState(false);
  // Claude Desktop: subagentes e shells numa seção só, e os terminados atrás de uma linha.
  const desktop = useAparencia((s) => s.ferramentas) === 'claudeDesktop';
  const [finishedOpen, setFinishedOpen] = useState(false);
  // Uma tarefa está parando enquanto tem prazo de confirmação; o mapa é a fonte, e mudá-lo pede um render.
  const stopTimers = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  const [, rerender] = useReducer((n: number) => n + 1, 0);
  const [wfLoading, setWfLoading] = useState(false);
  const [detail, setDetail] = useState<WorkflowDetail | null>(null);
  const [agentDetail, setAgentDetail] = useState<WorkflowAgentDetail | null>(null);
  const [agentLoading, setAgentLoading] = useState(false);
  // Separado de `detail === null`: sem isto, "a rede caiu" e "esse run não existe" mostravam a
  // mesma frase, e quem estava com a conexão instável ficava lendo que o run tinha sumido.
  const [wfError, setWfError] = useState('');
  const [agentError, setAgentError] = useState('');
  // Qual agente está aberto: o "tentar de novo" precisa saber o que recarregar, e `subAgentId` é
  // de outro nível da navegação (subagente), não deste.
  const [agentAberto, setAgentAberto] = useState<string | null>(null);
  const [subAgentId, setSubAgentId] = useState<string | null>(null);
  const [subTitle, setSubTitle] = useState('');
  const genWf = useRef(0);
  const genAgent = useRef(0);
  const genList = useRef(0);
  const genSubs = useRef(0);
  const subsTimedOut = useRef(false);

  const subRequest = useRef<{ generation: number; rerun: boolean } | null>(null);

  const refreshSubs = useCallback(async (invalidate = true): Promise<void> => {
    if (!server) return;
    if (!invalidate && subsTimedOut.current) return;
    if (invalidate) subsTimedOut.current = false;
    const generation = genList.current;
    if (subRequest.current?.generation === generation) {
      if (invalidate) {
        genSubs.current += 1;
        subRequest.current.rerun = true;
      }
      return;
    }
    const request = { generation, rerun: false };
    subRequest.current = request;
    const subGeneration = ++genSubs.current;
    try {
      const subs = await readActivity(getSubagents(name, server));
      if (generation !== genList.current || subGeneration !== genSubs.current) return;
      setList((current) => ({ ...current, subs, subLoading: false, subError: '' }));
    } catch (error) {
      if (generation !== genList.current || subGeneration !== genSubs.current) return;
      subsTimedOut.current = error instanceof ActivityTimeout;
      setList((current) => ({ ...current, subLoading: false, subError: error instanceof ActivityTimeout ? error.message : m.atividade_erro_subagentes() }));
    } finally {
      if (subRequest.current === request) {
        subRequest.current = null;
        if (request.rerun && generation === genList.current) void refreshSubs();
      }
    }
  }, [listKey, name, server, readActivity]);

  const workflowRequest = useRef<number | null>(null);
  const fetchAll = useCallback(async () => {
    // Sem o servidor da rota (lista ainda carregando ou máquina removida) não pergunta ao ativo.
    if (!server) return;
    const generation = genList.current;
    const subRefresh = refreshSubs();
    if (workflowRequest.current === generation) return subRefresh;
    workflowRequest.current = generation;
    await Promise.all([
      readActivity(getWorkflows(name, server)).then(
        (workflows) => {
          if (generation === genList.current) setList((current) => ({ ...current, workflows, workflowLoading: false, workflowError: '' }));
        },
        (error) => {
          if (generation === genList.current) setList((current) => ({ ...current, workflowLoading: false, workflowError: error instanceof ActivityTimeout ? error.message : m.atividade_erro_run() }));
        },
      ).finally(() => {
        if (workflowRequest.current === generation) workflowRequest.current = null;
      }),
      subRefresh,
    ]);
  }, [name, refreshSubs, server, readActivity]);

  useEffect(() => {
    genList.current += 1;
    setList({ key: listKey, workflows: [], subs: [], workflowLoading: true, subLoading: true, workflowError: '', subError: '' });
    setLevel('list');
    setRunId(null);
    setDetail(null);
    setAgentDetail(null);
    setSubAgentId(null);
    setSubTitle('');
    setOlderOpen(false);
    void fetchAll();
    return () => { genList.current += 1; };
  }, [fetchAll, name, serverId]);

  const agentSignature = JSON.stringify(transcriptActivity.agents.filter((agent) => agent.kind === 'agent').map((agent) => [agent.id, agent.running]));
  const previousAgents = useRef({ key: listKey, signature: agentSignature });
  useEffect(() => {
    const previous = previousAgents.current;
    previousAgents.current = { key: listKey, signature: agentSignature };
    if (previous.key === listKey && previous.signature !== agentSignature) void refreshSubs();
  }, [agentSignature, listKey, refreshSubs]);

  const previousNativeIds = useRef(nativeIds);
  useEffect(() => {
    const changed = previousNativeIds.current !== nativeIds;
    previousNativeIds.current = nativeIds;
    if (changed && nativeSubagents) void refreshSubs();
  }, [nativeIds, nativeSubagents, refreshSubs]);
  useEffect(() => {
    if (!focused || level !== 'list' || nativeSubagents) return;
    const timer = setInterval(() => void refreshSubs(false), 5_000);
    return () => clearInterval(timer);
  }, [focused, level, nativeSubagents, refreshSubs]);

  // Casar agente e subagente compara prompts inteiros: uma vez por agente, e o id da tarefa junto.
  const agentInfo = useMemo(() => {
    const byAgent = new Map<AgentRun, { sub: SubagentRun | undefined; taskId: string | undefined }>();
    const parentRunning = new Map<string, boolean>();
    for (const a of activity.agents) {
      const sub = matchAgentSubagent(subs, a, agentIds);
      byAgent.set(a, { sub, taskId: a.agentId ?? agentIds.get(a.id) ?? sub?.agentId });
      if (sub && !parentRunning.has(sub.agentId)) parentRunning.set(sub.agentId, a.running);
    }
    return { byAgent, parentRunning };
  }, [activity, subs, agentIds]);

  const { subRows, runningAgents, runningSubs } = useMemo(() => {
    const subRows = subs.map((run) => ({
      run,
      status: subagentStatus(run, agentInfo.parentRunning.get(run.agentId) ?? null, agora * 1000),
      at: Date.parse(run.endedAt ?? '') || Date.parse(run.updatedAt) || run.mtime * 1000,
    }));
    const runningAgents = activity.agents.filter((a) => {
      if (a.kind !== 'agent' || !a.running) return false;
      const sub = agentInfo.byAgent.get(a)?.sub;
      if (nativeSubagents && codexSubagentState(nativeStates, a.agentId ?? agentIds.get(a.id), a.agentPath)?.state !== 'running' && sub?.state !== 'running') return false;
      return !sub || subRows.find((row) => row.run.agentId === sub.agentId)?.status === 'running';
    });
    const shown = new Set(runningAgents.map((a) => agentInfo.byAgent.get(a)?.sub?.agentId));
    const runningSubs = subRows.filter((row) => row.status === 'running' && !shown.has(row.run.agentId));
    return { subRows, runningAgents, runningSubs };
  }, [subs, activity, agentInfo, agora, nativeSubagents, nativeStates, agentIds]);
  const uncertainSubs = subRows.filter((row) => row.status === 'silent' || row.status === 'unknown');
  const finishedSubs = subRows.filter((row) => row.status === 'finished' || row.status === 'failed');
  const midnight = new Date(agora * 1000).setHours(0, 0, 0, 0);
  const olderSubs = finishedSubs.filter((row) => row.at < midnight);
  const recentSubs = finishedSubs.filter((row) => !(row.at < midnight));
  const runningCount = runningAgents.length + runningSubs.length;
  const shellsVivos = useMemo(() => activity.shells.filter((s2) => s2.running), [activity]);
  const ha = (desde: number) => m.atividade_shell_ha({ t: formatElapsed(agora - desde) });
  const paiRodando = subAgentId ? agentInfo.parentRunning.get(subAgentId) ?? null : null;

  function tituloDoSub(s2: SubagentRun): string {
    const linhas = (s2.prompt ?? '').split('\n').map((l) => l.trim()).filter(Boolean);
    const util = linhas.find((l) => !/^base directory for this skill:/i.test(l) && !l.startsWith('#'));
    return (util ?? linhas[0] ?? s2.agentPath?.split('/').filter(Boolean).at(-1) ?? s2.nickname ?? s2.agentId).slice(0, 90);
  }

  function renderSub({ run, status, at }: (typeof subRows)[number]) {
    const ended = status === 'finished' || status === 'failed';
    const date = new Date(at);
    const clock = Number.isFinite(at) ? `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}` : null;
    let endedLabel = '';
    if (status === 'failed') {
      endedLabel = clock ? m.atividade_parou({ h: clock }) : m.formato_tool_falhou();
    } else if (status === 'finished') {
      endedLabel = clock ? m.atividade_terminou({ h: clock }) : m.atividade_sub_concluido();
    }
    return withStop(
      <Pressable key={run.agentId} onPress={() => abrirDoDisco(run)} style={[styles.agentRow, styles.agentOpenable, ended && styles.concluido]} accessibilityRole="button">
        <View style={styles.agentBody}>
          <View style={styles.agentHead}>
            <Text style={[styles.agentDesc, { color: theme.tokens.text.primary }]} numberOfLines={1}>{tituloDoSub(run)}</Text>
            {run.agentType ? <Text style={[styles.tag, { color: theme.tokens.text.secondary, backgroundColor: superficie(theme, 0.8) }]}>{run.agentType}</Text> : null}
          </View>
          {run.model ? <Text style={[styles.model, { color: theme.tokens.text.muted }]} numberOfLines={1}>{run.model}</Text> : null}
          <Text style={[styles.agentNow, { color: theme.tokens.text.muted }]} numberOfLines={1}>
            {ended ? `${endedLabel} · ` : ''}
            {status === 'silent' ? `${m.atividade_sem_noticia({ t: formatElapsed(agora - at / 1000) })} · ` : ''}
            {run.ilegivel ? m.atividade_sub_ilegivel() : run.problema ? '' : m.atividade_chamadas({ n: run.toolCalls })}
            {!run.ilegivel && !run.problema && run.recent.length ? ` · ${run.recent[run.recent.length - 1]?.name ?? ''}` : ''}
          </Text>
          {run.problema ? <Text style={[styles.err, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{subagentProblemText(run.problema)}</Text> : null}
        </View>
        <Icon name="ChevronRight" size={18} color={theme.tokens.text.muted} />
      </Pressable>,
      run.agentId, status === 'running' || stopTimers.current.has(run.agentId) ? run.agentId : undefined, tituloDoSub(run),
    );
  }

  // ■ só no Claude: o servidor recusa o resto (`erro_parar_so_claude`), e o Codex tem os comandos dele.
  const canStop = !readOnly && !nativeSubagents && (!provider || provider === 'claude');
  const liveTaskIds = useMemo(() => new Set([
    ...runningAgents.map((a) => agentInfo.byAgent.get(a)?.taskId),
    ...runningSubs.map(({ run }) => run.agentId),
    ...shellsVivos.map((s2) => s2.taskId),
  ].filter((id): id is string => typeof id === 'string')), [runningAgents, runningSubs, shellsVivos, agentInfo]);
  // Lista recarregando vem vazia: a tarefa sumir dela só prova a parada com os subagentes lidos.
  // Lista que falhou também vem vazia: sumir dela não prova que a tarefa parou.
  const subsReady = currentList && !list.subLoading && !list.subError;

  const endStop = useCallback((taskId: string) => {
    if (!stopTimers.current.has(taskId)) return;
    clearTimeout(stopTimers.current.get(taskId));
    stopTimers.current.delete(taskId);
    rerender();
  }, []);
  // A tarefa terminou ou saiu da lista: a parada se confirmou e a linha some com ela.
  useEffect(() => {
    for (const taskId of [...stopTimers.current.keys()]) {
      if (liveTaskIds.has(taskId)) continue;
      // Shell: o fim lido no transcript basta, sem depender da lista de subagentes.
      if (activity.shells.some((s2) => s2.taskId === taskId && !s2.running)) { endStop(taskId); continue; }
      if (!subsReady) continue;
      // Subagente que só ficou silencioso ou ilegível segue listado e não parou.
      const row = subRows.find((item) => item.run.agentId === taskId);
      if (!row || row.status === 'finished' || row.status === 'failed') endStop(taskId);
    }
  }, [liveTaskIds, subsReady, subRows, activity, endStop]);
  useEffect(() => () => {
    for (const timer of stopTimers.current.values()) clearTimeout(timer);
    stopTimers.current.clear();
  }, [listKey]);

  async function stop(taskId: string) {
    if (!server || stopTimers.current.has(taskId)) return;
    haptic.selection();
    const timer = setTimeout(() => {
      endStop(taskId);
      toast.erro(m.atividade_parar_sem_confirmacao());
    }, STOP_CONFIRM_MS);
    stopTimers.current.set(taskId, timer);
    rerender();
    // A região ao vivo não anuncia texto recém-montado com garantia; o anúncio explícito vale nos dois sistemas.
    AccessibilityInfo.announceForAccessibility(m.atividade_parando());
    try {
      await stopTask(name, taskId, server);
    } catch (error) {
      // O prazo é a identidade da tentativa: recusa atrasada de uma anterior não encerra a atual.
      if (stopTimers.current.get(taskId) !== timer) return;
      endStop(taskId);
      toast.erro(error instanceof Error ? error.message : String(error));
    }
  }

  function taskRow(key: string, icon: IconName, title: string, since: number | null, onPress?: () => void, taskId?: string) {
    const parando = !!taskId && stopTimers.current.has(taskId);
    return (
      <View key={key} style={styles.taskRow}>
        <Pressable onPress={onPress} disabled={!onPress} style={styles.taskOpen}
          accessibilityRole={onPress ? 'button' : undefined} accessibilityLabel={title}
          accessibilityState={parando ? { busy: true } : undefined}>
          <Icon name={icon} size={16} color={theme.tokens.text.muted} />
          <Text style={[styles.taskName, { color: theme.tokens.text.primary }]} numberOfLines={1}>{title}</Text>
          {parando ? <Text style={[styles.taskTime, { color: theme.tokens.text.muted }]} accessibilityLiveRegion="polite">{m.atividade_parando()}</Text>
            : since !== null && Number.isFinite(since) ? <Text style={[styles.taskTime, { color: theme.tokens.text.muted }]}>{formatElapsed(agora - since)}</Text> : null}
        </Pressable>
        {stopButton(taskId, title)}
      </View>
    );
  }

  // ■ de uma tarefa rodando; parando, nenhum: quem diz "Parando…" é a linha.
  function stopButton(taskId: string | undefined, title: string) {
    if (!canStop || !taskId || stopTimers.current.has(taskId)) return null;
    return (
      <Pressable onPress={() => void stop(taskId)} style={styles.stop} accessibilityRole="button"
        accessibilityLabel={`${m.atividade_desktop_parar()}: ${title}`}>
        <View style={[styles.stopGlyph, { backgroundColor: theme.tokens.text.primary }]} />
      </Pressable>
    );
  }

  // Linha das seções de sempre com o ■ ou o "Parando…" à direita; sem tarefa a parar, fica como era.
  function withStop(row: ReactElement, key: string, taskId: string | undefined, title: string) {
    const parando = !!taskId && stopTimers.current.has(taskId);
    const button = stopButton(taskId, title);
    if (!parando && !button) return row;
    return (
      <View key={key} style={styles.taskRow} accessibilityState={parando ? { busy: true } : undefined}>
        <View style={styles.grow}>{row}</View>
        {parando ? <Text style={[styles.taskTime, { color: theme.tokens.text.muted }]} accessibilityLiveRegion="polite">{m.atividade_parando()}</Text> : button}
      </View>
    );
  }

  const finishedList = (
    <>
      {recentSubs.map(renderSub)}
      {olderSubs.length > 0 ? (
        <>
          <Pressable onPress={() => setOlderOpen((value) => !value)} accessibilityRole="button" accessibilityState={{ expanded: olderOpen }} style={styles.olderToggle}>
            <Icon name={olderOpen ? 'ChevronDown' : 'ChevronRight'} size={18} color={theme.tokens.text.muted} />
            <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.atividade_grupo_antigos({ n: olderSubs.length })}</Text>
          </Pressable>
          {olderOpen ? olderSubs.map(renderSub) : null}
        </>
      ) : null}
    </>
  );

  async function openWorkflow(rid: string) {
    const g = ++genWf.current;
    setRunId(rid);
    setLevel('workflow');
    setDetail(null);
    setWfLoading(true);
    setWfError('');
    try {
      const d = await readActivity(getWorkflow(name, rid, server));
      if (g !== genWf.current) return;
      setDetail(d);
    } catch (error) {
      if (g !== genWf.current) return;
      setDetail(null);
      setWfError(error instanceof ActivityTimeout ? error.message : m.atividade_erro_run());
    } finally {
      if (g === genWf.current) setWfLoading(false);
    }
  }

  async function openAgent(agentId: string | null) {
    if (!runId || !agentId) return;
    const g = ++genAgent.current;
    setLevel('agent');
    setAgentAberto(agentId);
    setAgentDetail(null);
    setAgentLoading(true);
    setAgentError('');
    try {
      const d = await readActivity(getWorkflowAgent(name, runId, agentId, server));
      if (g !== genAgent.current) return;
      setAgentDetail(d);
    } catch (error) {
      if (g !== genAgent.current) return;
      setAgentDetail(null);
      setAgentError(error instanceof ActivityTimeout ? error.message : m.atividade_erro_run());
    } finally {
      if (g === genAgent.current) setAgentLoading(false);
    }
  }

  function abrirDoDisco(s2: SubagentRun) {
    setSubTitle(tituloDoSub(s2));
    setSubAgentId(s2.agentId);
    setLevel('subagent');
  }

  function openSubagent(sub: SubagentRun, title: string) {
    setSubTitle(title);
    setSubAgentId(sub.agentId);
    setLevel('subagent');
  }

  const back = useCallback(() => {
    if (level === 'subagent') {
      setSubAgentId(null);
      setSubTitle('');
      setLevel('list');
      if (!nativeSubagents) void refreshSubs();
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
  }, [level, refreshSubs, nativeSubagents]);

  useEffect(() => {
    onBackRequest?.(() => {
      if (level === 'list') return false;
      back();
      return true;
    });
    return () => onBackRequest?.(() => false);
  }, [back, level, onBackRequest]);

  const headerTitle = level === 'list' ? m.ctx_atividade() : level === 'subagent' ? subTitle || m.atividade_subagente() : level === 'workflow' ? (detail?.name ?? m.atividade_workflow()) : (agentDetail?.label ?? m.atividade_agente());
  const countBadge = level === 'list' && activity.total > 0 ? `${activity.done}/${activity.total}` : null;
  const wfBadge = level === 'workflow' && detail ? detail.status : null;
  const localHeader = (
    <View collapsable={false} style={styles.header}>
      {level !== 'list' ? <HeaderBack onPress={back} /> : null}
      <Text style={[styles.title, { color: theme.tokens.text.primary }]} numberOfLines={1} accessibilityRole="header">{headerTitle}</Text>
      {countBadge ? <Text style={[styles.count, { color: theme.tokens.text.secondary }]}>{countBadge}</Text> : wfBadge ? (
        <Text style={[styles.wfStatus, { color: wfBadge === 'completed' ? theme.tokens.status.success : wfBadge === 'running' ? theme.tokens.accent.base : theme.tokens.text.secondary }]}>{wfBadge}</Text>
      ) : null}
      {onClose ? <Pressable accessibilityRole="button" accessibilityLabel={m.native_close()} onPress={onClose} style={styles.close}>
        <Icon name="X" size={20} color={theme.tokens.text.secondary} />
      </Pressable> : null}
    </View>
  );

  if (!server) {
    return (
      <>
        <View style={styles.root}>
          {localHeader}
          <Text style={[styles.error, { color: theme.tokens.status.error }]}>{m.compare_servidor_nao_encontrado()}</Text>
        </View>
      </>
    );
  }

  return (
    <>
      <View style={styles.root}>
        {localHeader}
        {level === 'list' ? (
          <ScrollView style={styles.scroll} contentContainerStyle={[styles.body, { paddingBottom: insets.bottom + theme.base.space[4] }]} contentInsetAdjustmentBehavior="never" showsVerticalScrollIndicator={false}>
            {workflows.length > 0 ? <WorkflowList workflows={workflows} onSelect={openWorkflow} /> : null}
            {listLoading ? (
              <View style={styles.center}>
                <ActivityIndicator color={theme.tokens.text.muted} />
                <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
              </View>
            ) : null}

            {desktop && (runningCount > 0 || shellsVivos.length > 0 || finishedSubs.length > 0) ? (
              <View style={styles.section}>
                <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.atividade_desktop_titulo()}</Text>
                {runningAgents.map((a) => {
                  const info = agentInfo.byAgent.get(a);
                  const sub = info?.sub;
                  return taskRow(a.id, 'Diamond', a.description, sub ? Date.parse(sub.startedAt) / 1000 : null, sub ? () => openSubagent(sub, a.description) : undefined,
                    info?.taskId);
                })}
                {runningSubs.map(({ run }) => taskRow(run.agentId, 'Diamond', tituloDoSub(run), Date.parse(run.startedAt) / 1000, () => abrirDoDisco(run), run.agentId))}
                {shellsVivos.map((s2) => taskRow(s2.id, 'Terminal', s2.description || s2.rotulo, s2.ts ?? null, undefined, s2.taskId))}
                {finishedSubs.length > 0 ? (
                  <Pressable onPress={() => setFinishedOpen((value) => !value)} style={styles.taskOpen} accessibilityRole="button"
                    accessibilityLabel={m.atividade_desktop_finalizadas({ n: finishedSubs.length })} accessibilityState={{ expanded: finishedOpen }}>
                    <Icon name="Diamond" size={16} color={theme.tokens.text.muted} />
                    <Text style={[styles.taskName, { color: theme.tokens.text.secondary }]}>{m.atividade_desktop_finalizadas({ n: finishedSubs.length })}</Text>
                  </Pressable>
                ) : null}
                {finishedOpen ? finishedList : null}
              </View>
            ) : null}

            {!desktop && (runningAgents.length > 0 || runningSubs.length > 0) ? (
              <View style={styles.section}>
                <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.atividade_grupo_rodando({ n: runningCount })}</Text>
                {runningAgents.map((a) => {
                  const info = agentInfo.byAgent.get(a);
                  const sub = info?.sub;
                  const model = agentModel(a, sub);
                  const isButton = !!sub;
                  const Row = isButton ? Pressable : View;
                  return withStop(
                    // @ts-ignore — Pressable/View condicionais
                    <Row
                      key={a.id}
                      onPress={sub ? () => openSubagent(sub, a.description) : undefined}
                      style={[styles.agentRow, isButton && styles.agentOpenable]}
                      accessibilityRole={isButton ? 'button' : undefined}
                    >
                      <Text style={[styles.spin, { color: theme.tokens.accent.base }]}>◐</Text>
                      <View style={styles.agentBody}>
                        <View style={styles.agentHead}>
                          <Text style={[styles.agentDesc, { color: theme.tokens.text.primary }]} numberOfLines={1}>
                            {a.description}
                          </Text>
                          {a.subagentType ? <Text style={[styles.tag, { color: theme.tokens.text.secondary, backgroundColor: superficie(theme, 0.8) }]}>{a.subagentType}</Text> : null}
                        </View>
                        {model ? <Text style={[styles.model, { color: theme.tokens.text.muted }]} numberOfLines={1}>{model}</Text> : null}
                        {sub ? (
                          <Text style={[styles.agentNow, { color: theme.tokens.text.muted }]} numberOfLines={1}>
                            {sub.ilegivel ? m.atividade_sub_ilegivel() : sub.problema ? '' : m.atividade_chamadas({ n: sub.toolCalls })}
                            {!sub.ilegivel && !sub.problema && sub.recent.length ? ` · ${sub.recent[sub.recent.length - 1]?.name ?? ''}` : ''}
                          </Text>
                        ) : null}
                        {sub?.problema ? <Text style={[styles.err, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{subagentProblemText(sub.problema)}</Text> : null}
                      </View>
                      {sub ? <Icon name="ChevronRight" size={18} color={theme.tokens.text.muted} /> : null}
                    </Row>,
                    a.id, info?.taskId, a.description,
                  );
                })}
                {runningSubs.map(renderSub)}
              </View>
            ) : null}

            {uncertainSubs.length > 0 ? (
              <View style={styles.section}>
                <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.atividade_subagentes()}</Text>
                {uncertainSubs.map(renderSub)}
              </View>
            ) : null}

            {!desktop && finishedSubs.length > 0 ? (
              <View style={styles.section}>
                <Text style={[styles.label, { color: theme.tokens.text.muted }]}>{m.atividade_grupo_terminados({ n: finishedSubs.length })}</Text>
                {finishedList}
              </View>
            ) : null}

            {/* Shells de fundo que ainda rodam: o terminal diz "N shells still running", aqui diz qual.
                Sem toque: a saída de cada um volta pro chat quando ele termina. */}
            {!desktop && shellsVivos.length > 0 ? (
              <View style={styles.section}>
                <Text style={[styles.label, { color: theme.tokens.text.muted }]}>
                  {m.atividade_shells()} <Text style={{ color: theme.tokens.accent.base }}>{m.atividade_shells_rodando({ n: shellsVivos.length })}</Text>
                </Text>
                {shellsVivos.map((s2) => withStop(
                  <View key={s2.id} style={styles.agentRow} accessible accessibilityLabel={s2.command}>
                    <ActivityIndicator size="small" color={theme.tokens.accent.base} />
                    <View style={styles.agentBody}>
                      <Text style={[styles.shellCmd, { color: theme.tokens.text.primary }]} numberOfLines={2}>{s2.rotulo}</Text>
                      {s2.description || s2.ts ? (
                        <Text style={[styles.agentNow, { color: theme.tokens.text.muted }]} numberOfLines={1}>
                          {[s2.description, s2.ts ? ha(s2.ts) : ''].filter(Boolean).join(' · ')}
                        </Text>
                      ) : null}
                    </View>
                  </View>,
                  s2.id, s2.taskId, s2.description || s2.rotulo,
                ))}
              </View>
            ) : null}

            {/* Processos vivos agora, lidos do sistema: pega o que o transcript não sabe (comando de
                subagente, de antes de um /clear). É o que explica a sessão parada com shell pendurado. */}
            {processos.length > 0 ? (
              <View style={styles.section}>
                <Text style={[styles.label, { color: theme.tokens.text.muted }]}>
                  {m.atividade_processos()} <Text style={{ color: theme.tokens.accent.base }}>{m.atividade_processos_n({ n: processos.length })}</Text>
                </Text>
                {processos.map((p) => (
                  <View key={p.pid} style={styles.agentRow} accessible accessibilityLabel={p.cmd}>
                    <ActivityIndicator size="small" color={theme.tokens.accent.base} />
                    <View style={styles.agentBody}>
                      <Text style={[styles.shellCmd, { color: theme.tokens.text.primary }]} numberOfLines={2}>{p.cmd}</Text>
                      <Text style={[styles.agentNow, { color: theme.tokens.text.muted }]} numberOfLines={1}>
                        pid {p.pid}{p.desde ? ` · ${ha(p.desde)}` : ''}
                      </Text>
                    </View>
                  </View>
                ))}
              </View>
            ) : null}

            <CodexCommands key={listKey} commands={commands} name={name} serverId={serverId} server={server} readOnly={readOnly} />
            {activity.tasks.length > 0 ? <TaskRows tasks={activity.tasks} /> : null}
            {workflowListError ? <Text style={[styles.err, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{workflowListError}</Text> : null}
            {subError ? <Text style={[styles.err, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{subError}</Text> : null}
            {workflowListError || subError ? (
              <Pressable onPress={() => void fetchAll()} style={styles.olderToggle} accessibilityRole="button">
                <Text style={[styles.muted, { color: theme.tokens.accent.base }]}>{m.sync_retry()}</Text>
              </Pressable>
            ) : null}
            {!listLoading && !workflowListError && !subError && workflows.length === 0 && activity.tasks.length === 0 && runningAgents.length === 0 && subs.length === 0 && shellsVivos.length === 0 && processos.length === 0 && commands.length === 0 ? <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.atividade_vazio()}</Text> : null}
          </ScrollView>
        ) : level === 'workflow' ? (
          <View style={[styles.detail, { paddingBottom: insets.bottom }]}>
            <WorkflowDetailView
              detail={detail}
              loading={wfLoading}
              onAgent={openAgent}
              erro={wfError}
              onTentar={() => runId && openWorkflow(runId)}
            />
          </View>
        ) : level === 'agent' ? (
          <View style={[styles.detail, { paddingBottom: insets.bottom }]}>
            <AgentDetailView
              detail={agentDetail}
              loading={agentLoading}
              erro={agentError}
              onTentar={() => openAgent(agentAberto)}
            />
          </View>
        ) : level === 'subagent' && subAgentId ? (
          <View style={[styles.detail, { paddingBottom: insets.bottom }]}>
            <SubagentLive key={`${serverId}::${name}::${subAgentId}`} server={server} sessionName={name} agentId={subAgentId} paiRodando={paiRodando} states={nativeStates} />
          </View>
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
  header: { flexDirection: 'row', alignItems: 'center', minHeight: 44, paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[2], gap: theme.base.space[2] },
  title: { flex: 1, fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold },
  close: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  count: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.sm, fontVariant: ['tabular-nums'] },
  wfStatus: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[0.5], borderRadius: theme.base.radius.full, overflow: 'hidden' },
  scroll: { flex: 1 },
  body: { padding: theme.base.space[4], gap: theme.base.space[4] },
  detail: { flex: 1, minHeight: 0, paddingHorizontal: theme.base.space[4] },
  section: { gap: theme.base.space[2] },
  olderToggle: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minHeight: 44, minWidth: 44 },
  label: { fontSize: theme.base.text.xs, textTransform: 'uppercase', letterSpacing: 0.6, fontWeight: theme.base.weight.semibold },
  center: { alignItems: 'center', gap: theme.base.space[2], paddingVertical: theme.base.space[4] },
  muted: { fontSize: theme.base.text.sm, textAlign: 'center' },
  empty: { fontSize: theme.base.text.sm, textAlign: 'center', paddingVertical: theme.base.space[4] },
  err: { fontSize: theme.base.text.xs, marginTop: theme.base.space[2] },
  agentRow: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[2], paddingVertical: theme.base.space[1] },
  agentOpenable: { minHeight: 44, paddingHorizontal: theme.base.space[1], marginHorizontal: -theme.base.space[1], borderRadius: theme.base.radius.sm },
  spin: { fontSize: theme.base.text.sm, marginTop: theme.base.space[0.5] },
  agentBody: { flex: 1, gap: theme.base.space[0.5], minWidth: 0 },
  agentHead: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minWidth: 0 },
  agentDesc: { fontSize: theme.base.text.sm, minWidth: 0, flexShrink: 1 },
  tag: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxxs, paddingHorizontal: theme.base.space[1.5], paddingVertical: 1, borderRadius: theme.base.radius.full, overflow: 'hidden' },
  tagMuted: {},
  model: { ...theme.type.caption, fontFamily: theme.base.fontMono },
  agentNow: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxxs },
  shellCmd: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs },
  concluido: { opacity: 0.6 },
  taskRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  taskOpen: { flex: 1, minWidth: 0, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 44 },
  grow: { flex: 1, minWidth: 0 },
  stop: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  stopGlyph: { width: 10, height: 10, borderRadius: theme.base.space[0.5] },
  taskName: { flex: 1, minWidth: 0, fontSize: theme.base.text.sm },
  taskTime: { fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'] },
  error: { fontSize: theme.base.text.sm, textAlign: 'center', padding: theme.base.space[4] },
}));
