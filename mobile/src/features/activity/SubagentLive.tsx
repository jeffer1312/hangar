import { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { codexSubagentState, formatElapsed, getSubagent, mensagemDeErro, prependOlder, projectCodexSubagent, sendSubagentMessage } from '@hangar/core';
import type { CodexSubagents, Server, SubagentRun } from '@hangar/core';
import { MessageList } from '../../chat/MessageList';
import { WorkingLine } from '../../chat/LiveWork';
import * as m from '../../paraglide/messages';
import { superficie } from '../../theme/superficie';
import { Icon } from '../../ui/Icon';
import { ActivityTimeout, useActivityRequest } from './useActivityRequest';

interface Props {
  server: Server;
  sessionName: string;
  agentId: string;
  // O que o transcript do PAI diz (Claude): true/false, ou null quando só o disco conhece o subagente.
  paiRodando?: boolean | null;
  states?: CodexSubagents;
}

export function subagentStatus(detail: SubagentRun, parentRunning: boolean | null, nowMs: number): 'running' | 'finished' | 'failed' | 'silent' | 'unknown' {
  if (detail.state === 'running' || detail.state === 'finished' || detail.state === 'failed') return detail.state;
  if (detail.failed) return 'failed';
  if (detail.problema || detail.ilegivel) return 'unknown';
  if (detail.finished || parentRunning === false) return 'finished';
  if (parentRunning === true) return 'running';
  const updated = Date.parse(detail.updatedAt) || detail.mtime * 1000;
  return nowMs - updated > 600000 ? 'silent' : 'running';
}

export function subagentProblemText(code: string): string {
  switch (code) {
    case 'erro_subagente_estado_indisponivel': return m.erro_subagente_estado_indisponivel();
    case 'erro_subagente_mensagem_sem_resposta': return m.erro_subagente_mensagem_sem_resposta();
    case 'erro_subagente_mensagem_sem_plugin': return m.erro_subagente_mensagem_sem_plugin();
    case 'erro_subagente_mensagem_plugin_antigo': return m.erro_subagente_mensagem_plugin_antigo();
    case 'erro_subagente_mensagem_sem_terminal': return m.erro_subagente_mensagem_sem_terminal();
    case 'erro_subagente_mensagem_vinculo_desatualizado': return m.erro_subagente_mensagem_vinculo_desatualizado();
    default: return mensagemDeErro(code) ?? code;
  }
}

export function SubagentLive(props: Props) {
  return <SubagentConversation key={JSON.stringify([props.server.id, props.sessionName, props.agentId])} {...props} />;
}

function SubagentConversation({ server, sessionName, agentId, paiRodando = null, states }: Props) {
  const { theme } = useUnistyles();
  const readActivity = useActivityRequest();
  const [rawDetail, setDetail] = useState<SubagentRun | null>(null);
  const detail = rawDetail ? projectCodexSubagent(rawDetail, states) : null;
  const nativeState = codexSubagentState(states, agentId, detail?.agentPath);
  const lifecycle = JSON.stringify([nativeState?.state, nativeState?.ended_at]);
  const latestStates = useRef(states);
  latestStates.current = states;
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [retry, setRetry] = useState(0);
  const [limit, setLimit] = useState(200);
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState('');
  const alive = useRef(true);
  const sendPending = useRef(false);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; };
  }, []);

  useEffect(() => {
    setLoading(true);
    setError('');
    let active = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let stopped = false;
    let gen = 0;
    let failCount = 0;

    async function tick() {
      const myGen = ++gen;
      try {
        const d = await readActivity(getSubagent(sessionName, agentId, limit, server));
        if (!active || myGen !== gen) return;
        if (!d.ilegivel && d.events === undefined) {
          setDetail(previous => ({ ...d, events: previous?.events }));
          setError(m.atividade_erro_transcript());
          setLoading(false);
          stopped = true;
          return;
        }
        setDetail(previous => ({ ...d, events: d.events?.length ? prependOlder(previous?.events ?? [], d.events) ?? d.events : d.events }));
        setError('');
        failCount = 0;
        setLoading(false);
        const status = subagentStatus(projectCodexSubagent(d, latestStates.current), paiRodando, Date.now());
        stopped = status === 'finished' || status === 'failed'
          || codexSubagentState(latestStates.current, agentId, d.agentPath)?.state === null;
      } catch (e) {
        if (!active || myGen !== gen) return;
        failCount += 1;
        if (e instanceof ActivityTimeout || (e && typeof e === 'object' && 'status' in e && e.status === 404) || failCount >= 3) {
          stopped = true;
          setError(e instanceof ActivityTimeout ? e.message : m.atividade_erro_vivo());
          setLoading(false);
        }
      }
    }

    // Espera a leitura anterior para não empilhar pedidos em servidor lento.
    async function loop() {
      await tick();
      if (active && !stopped) timer = setTimeout(loop, 2500);
    }

    void loop();
    return () => {
      active = false;
      if (timer) clearTimeout(timer);
    };
  }, [server, sessionName, agentId, paiRodando, lifecycle, retry, limit, readActivity]);

  async function send() {
    if (!detail?.canMessage || !text.trim() || sendPending.current) return;
    sendPending.current = true;
    setSending(true);
    setSendError('');
    try {
      await sendSubagentMessage(sessionName, agentId, text, server);
      if (!alive.current) return;
      setText('');
      setRetry(value => value + 1);
    } catch (e) {
      if (alive.current) {
        const failure = e as { code?: string; envelope?: { params?: Record<string, unknown> } };
        const code = failure?.code;
        if (code === 'erro_subagente_mensagem_recusada') {
          setSendError(m.erro_subagente_mensagem_recusada({ reason: String(failure.envelope?.params?.reason ?? '') }));
        } else {
          const translatedError = code ? subagentProblemText(code) : '';
          setSendError(code && translatedError !== code ? translatedError
            : e instanceof Error ? e.message : String(e));
        }
      }
    } finally {
      if (alive.current) {
        sendPending.current = false;
        setSending(false);
      }
    }
  }

  const errorNotice = error ? (
    <View style={styles.notice}>
      <Text style={[styles.err, { color: theme.tokens.status.error }]} accessibilityRole="alert">{error}</Text>
      <Pressable style={styles.retry} accessibilityRole="button" onPress={() => setRetry(value => value + 1)}>
        <Text style={[styles.muted, { color: theme.tokens.accent.base }]}>{m.sync_retry()}</Text>
      </Pressable>
    </View>
  ) : null;

  if (loading && !detail) {
    return (
      <View style={styles.center}>
        <ActivityIndicator color={theme.tokens.text.muted} />
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
      </View>
    );
  }

  if (!detail) {
    return <View style={styles.center}>{errorNotice ?? <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.atividade_sem_transcript()}</Text>}</View>;
  }

  const status = subagentStatus(detail, paiRodando, Date.now());
  const running = status === 'running';
  const problem = detail.problema ? subagentProblemText(detail.problema) : '';
  const blocked = detail.messageBlocked ? subagentProblemText(detail.messageBlocked) : '';
  const blockedLabel = blocked && blocked !== detail.messageBlocked ? blocked : m.native_subagent_message_unavailable();
  const statusLabel = status === 'finished' ? m.atividade_sub_concluido()
    : status === 'failed' ? m.subagente_card_falhou()
      : status === 'silent' ? m.atividade_sem_noticia({ t: formatElapsed((Date.now() - (Date.parse(detail.updatedAt) || detail.mtime * 1000)) / 1000) })
        : status === 'unknown' ? m.atividade_sub_ilegivel() : m.atividade_rodando();

  return (
    <KeyboardAvoidingView style={styles.wrap} behavior="padding" automaticOffset>
      <View style={styles.meta}>
        <Text style={[styles.metaTxt, { color: status === 'failed' ? theme.tokens.status.error : theme.tokens.text.muted }]}>
          {statusLabel}
          {!detail.ilegivel ? ` · ${m.atividade_chamadas({ n: detail.toolCalls })}` : ''}
          {detail.agentType ? ` · ${detail.agentType}` : ''}
          {detail.recent.length ? ` · ${detail.recent[detail.recent.length - 1]?.name ?? ''}` : ''}
        </Text>
        {problem ? <Text style={[styles.err, { color: theme.tokens.status.error }]} accessibilityRole="alert">{problem}</Text> : null}
        {errorNotice}
      </View>
      {detail.events?.length ? (
        <View style={[styles.chatBox, { backgroundColor: superficie(theme), borderColor: theme.tokens.border.subtle }]}>
          <MessageList
            preserveToolGroupPosition
            nestedScrollEnabled
            alignItemsAtEnd={false}
            events={detail.events}
            preview=""
            olderFailed=""
            onLoadOlder={() => { if (!loading && detail.events!.length >= limit) setLimit(limit + 200); }}
            pending={[]}
            sessionName={sessionName}
            serverId={server.id}
            stateEvent={null}
          />
        </View>
      ) : (
        <View style={styles.center}>
          <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>
            {detail.ilegivel ? m.atividade_sub_ilegivel()
              : status !== 'running' || problem ? problem || statusLabel
                : detail.toolCalls > 0 ? m.atividade_sem_transcript() : m.atividade_pensando()}
          </Text>
        </View>
      )}
      {running ? <WorkingLine since={Date.parse(detail.startedAt) || null} /> : null}
      {detail.tools.length > 0 ? (
        <ScrollView style={styles.toolsRow} keyboardShouldPersistTaps="handled">
          <Text style={[styles.rotulo, { color: theme.tokens.text.muted }]}>{m.atividade_ferramentas_chamadas({ n: detail.toolCalls })}</Text>
          <View style={styles.chips}>
            {detail.tools.map((tool) => (
              <View key={tool.name} style={[styles.chip, { backgroundColor: superficie(theme, 0.8), borderColor: theme.tokens.border.subtle }]}>
                <Text style={[styles.chipTxt, { color: theme.tokens.text.secondary }]}>{tool.name} ×{tool.count}</Text>
              </View>
            ))}
          </View>
        </ScrollView>
      ) : null}
      {sendError ? <Text accessibilityRole="alert" style={[styles.err, { color: theme.tokens.status.error }]}>{sendError}</Text> : null}
      {detail.canMessage ? (
        <View style={styles.composerArea}>
          <View style={styles.composer}>
            <TextInput
              style={[styles.input, { color: theme.tokens.text.primary, borderColor: theme.tokens.border.subtle }]}
              value={text}
              onChangeText={setText}
              editable={!sending}
              multiline
              placeholder={m.native_subagent_composer({ agent: agentId })}
              placeholderTextColor={theme.tokens.text.muted}
              accessibilityLabel={m.native_subagent_composer({ agent: agentId })}
            />
            <Pressable style={styles.send} accessibilityRole="button" accessibilityLabel={sending ? m.native_sending() : m.native_send()} disabled={sending || !text.trim()} onPress={() => void send()}>
              <Icon name="Send" color={theme.tokens.accent.base} />
            </Pressable>
          </View>
        </View>
      ) : <Text style={[styles.rodape, { color: theme.tokens.text.muted, borderTopColor: theme.tokens.border.subtle }]}>{blockedLabel}</Text>}
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create((theme) => ({
  center: { flex: 1, alignItems: 'center', justifyContent: 'center', gap: theme.base.space[2], padding: theme.base.space[4] },
  muted: { fontSize: theme.base.text.sm, textAlign: 'center' },
  err: { fontSize: theme.base.text.sm, textAlign: 'center' },
  wrap: { flex: 1, minHeight: 0, gap: theme.base.space[3] },
  meta: { gap: theme.base.space[1] },
  metaTxt: { fontSize: theme.base.text.xs },
  notice: { alignItems: 'center', gap: theme.base.space[2] },
  retry: { minHeight: 44, minWidth: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[3] },
  chatBox: { flex: 1, minHeight: 0, flexShrink: 1, maxHeight: 520, borderWidth: 1, borderRadius: theme.base.radius.md, overflow: 'hidden' },
  toolsRow: { maxHeight: 96, flexShrink: 1 },
  rotulo: { fontSize: theme.base.text.xxxs, letterSpacing: 0.8, textTransform: 'uppercase', fontWeight: theme.base.weight.semibold },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[1] },
  chip: { paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[0.5], borderRadius: theme.base.radius.sm, borderWidth: 1 },
  chipTxt: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxs },
  rodape: { textAlign: 'center', fontSize: theme.base.text.xxs, fontStyle: 'italic', borderTopWidth: 1, paddingTop: theme.base.space[2] },
  composerArea: { gap: theme.base.space[2] },
  composer: { flexDirection: 'row', alignItems: 'flex-end', gap: theme.base.space[2] },
  input: { flex: 1, minHeight: 44, maxHeight: 120, borderWidth: 1, borderRadius: theme.base.radius.md, padding: theme.base.space[2], fontSize: theme.base.text.sm },
  send: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center' },
}));
