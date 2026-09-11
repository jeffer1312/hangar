import { useEffect, useRef, useState } from 'react';
import { ScrollView, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { chatStore } from '../../../../src/stores/chat';
import { useServers } from '../../../../src/stores/servers';
import { useSessions } from '../../../../src/stores/sessions';
import { Screen } from '../../../../src/ui/Screen';
import { ChatHeader } from '../../../../src/chat/ChatHeader';
import { LoopChip } from '../../../../src/chat/LoopChip';
import { PlanChip } from '../../../../src/features/plan/PlanChip';
import { MessageList } from '../../../../src/chat/MessageList';
import { pararTts } from '../../../../src/chat/BubbleActions';
import { Composer } from '../../../../src/chat/Composer';
import { TuiPill } from '../../../../src/chat/TuiPill';
import { MoreSheet } from '../../../../src/chat/MoreSheet';
import { OptionButtons } from '../../../../src/chat/OptionButtons';
import { StatsStrip } from '../../../../src/chat/StatsStrip';
import { SessionPickerSheet } from '../../../../src/chat/SessionPickerSheet';
import { pendingAskFromEvents, askPayloadFromToolUse, fetchSessionsForServer, parseStatusLine, selectOption, interrupt } from '@hangar/core';
import type { Provider, SessionInfo } from '@hangar/core';
import * as m from '../../../../src/paraglide/messages';

// Tela de chat de uma sessão: histórico janelado + SSE ao vivo (store chat.ts).
// O composer real entra na Task 9; aqui só o placeholder sticky de 56px.
export default function ChatScreen() {
  const router = useRouter();
  const params = useLocalSearchParams<{ server: string; name: string }>();
  const serverId = Array.isArray(params.server) ? params.server[0] : (params.server ?? '');
  const name = Array.isArray(params.name) ? params.name[0] : (params.name ?? '');

  const chat = chatStore(serverId, name);
  const [servidorSumiu, setServidorSumiu] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const existe = useServers((s) => s.servers.some((x) => x.id === serverId));
  const ready = useServers((s) => s.ready);
  const retido = useRef(false); // só quem reteve solta; zera nos dois caminhos
  useEffect(() => {
    if (!ready) return; // SecureStore ainda não respondeu: nem julga, nem retém (final-r2, reg. 2)
    if (!useServers.getState().ensureActive(serverId)) {
      setServidorSumiu(true);
      return;
    }
    setServidorSumiu(false);
    chat.retain();
    retido.current = true;
    return () => {
      if (retido.current) {
        chat.release();
        retido.current = false;
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, serverId, name]);
  useEffect(() => {
    if (!ready || existe) return;
    setServidorSumiu(true);
    if (retido.current) {
      chat.release();
      retido.current = false;
    } // solta o SSE: senão ele reconecta contra o ativo NOVO (final-r2, reg. 1)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, existe]);

  const events = chat.use((s) => s.events);
  const stateEvent = chat.use((s) => s.stateEvent);
  const preview = chat.use((s) => s.preview);
  const previewMd = chat.use((s) => s.previewMd);
  const previewFull = chat.use((s) => s.previewFull);
  const statusLine = chat.use((s) => s.statusLine);
  const stats = chat.use((s) => s.stats);
  const loading = chat.use((s) => s.loading);
  const error = chat.use((s) => s.error);
  const olderFailed = chat.use((s) => s.olderFailed);
  const sseRecusado = chat.use((s) => s.sseRecusado);
  const pending = chat.use((s) => s.pending);
  const askOpen = chat.use((s) => s.askOpen);
  const askPayload = chat.use((s) => s.askPayload);
  const askPiId = chat.use((s) => s.askPiId);
  const askPiDismissed = chat.use((s) => s.askPiDismissed);

  // draft devolvido pelo cancelar do picker (Task 3)
  const [draft, setDraft] = useState<string | undefined>(undefined);
  const [aviso, setAviso] = useState('');
  const avisoTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  function mostrarAviso(e: unknown) {
    const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : m.comum_falha_envio_opcao();
    setAviso(msg);
    if (avisoTimer.current) clearTimeout(avisoTimer.current);
    avisoTimer.current = setTimeout(() => setAviso(''), 8000);
  }

  // Sem SSE adicional: acompanha a abertura e o Git do Codex pela lista.
  const rowsProvider = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.provider ?? null) as Provider | null;
  const [fetchedSession, setFetchedSession] = useState<SessionInfo | null | undefined>(undefined);
  const planSession = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name) ?? null);
  const currentSession = fetchedSession === undefined ? planSession : fetchedSession;
  const codexPreThread = currentSession?.provider === 'codex' && currentSession.tracked === false;
  useEffect(() => {
    if (!ready || servidorSumiu) return;
    setFetchedSession(undefined);
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      const server = useServers.getState().servers.find((s) => s.id === serverId);
      if (!server) return;
      try {
        const all = await fetchSessionsForServer(server);
        if (!alive) return;
        const hit = all.find((s) => s.name === name);
        setFetchedSession(hit ?? null);
        if (hit?.provider === 'codex') timer = setTimeout(refresh, hit.tracked === false ? 2000 : 5000);
      } catch {
        if (alive) {
          console.warn('fetchSessionsForServer falhou');
          timer = setTimeout(refresh, 5000);
        }
      }
    };
    void refresh();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [ready, servidorSumiu, serverId, name]);
  const provider: Provider | null = codexPreThread ? 'codex' : rowsProvider ?? fetchedSession?.provider ?? null;
  const wasPreThread = useRef(false);
  useEffect(() => {
    if (wasPreThread.current && currentSession?.tracked) chat.retry();
    wasPreThread.current = codexPreThread;
  }, [codexPreThread, currentSession?.tracked, chat]);

  // O "Ouvir" das bolhas toca num player de módulo; sair da conversa cala a voz.
  useEffect(() => () => pararTts(), []);

  // abrir a folha quando o store pedir
  useEffect(() => {
    if (askOpen) router.push(`/s/${serverId}/${name}/ask` as never);
  }, [askOpen, serverId, name, router]);

  // Pi/Kimi: pergunta vem como tool_use, não como SSE ask_question
  useEffect(() => {
    const q = provider ? pendingAskFromEvents(events, provider) : null;
    if (!q) {
      if (askPiId) chat.markAskDismissed();
      return;
    }
    if (askOpen || askPiDismissed === q.id) return;
    const payload = askPayloadFromToolUse(q, provider as Provider);
    if (!payload) {
      console.warn('ask: payload inesperado', q.tool_input);
      return;
    }
    chat.openAsk(payload, q.id);
  }, [events, provider, askOpen, askPiId, askPiDismissed, chat]);

  // quem responde: OptionButtons quando awaiting_input com question/options e sem stepper aberto
  const showOptions = !!(!askOpen && stateEvent?.state === 'awaiting_input' && stateEvent.question && stateEvent.options?.length);
  const handleSelectOption = (n: number) => {
    void selectOption(name, n).catch((e) => mostrarAviso(e));
  };
  const handleCancelOptions = () => {
    const cur = chat.use.getState().pending;
    const last = cur.length ? cur[cur.length - 1] : null;
    if (last) {
      setDraft(last.text);
      chat.use.setState({ pending: cur.filter((p) => p.id !== last.id) });
      void interrupt(name, true).catch((e) => mostrarAviso(e));
    } else {
      void interrupt(name, false).catch((e) => mostrarAviso(e));
    }
  };
  const optionsSlot = showOptions ? (
    <View>
      <OptionButtons question={stateEvent!.question!} options={stateEvent!.options!} onSelect={handleSelectOption} onCancel={handleCancelOptions} />
      {aviso ? <Text style={styles.aviso}>{aviso}</Text> : null}
    </View>
  ) : aviso ? (
    <Text style={styles.aviso}>{aviso}</Text>
  ) : undefined;

  return (
    <Screen>
      <ChatHeader
        name={name}
        state={codexPreThread ? currentSession.state : stateEvent?.state ?? null}
        onBack={() => {
          if (router.canGoBack()) router.back();
          else router.replace('/');
        }}
        onMore={() => setMoreOpen(true)}
        onTitlePress={() => setPickerOpen(true)}
        onTerminal={() => router.push(`/s/${serverId}/${name}/terminal` as never)}
        contextPct={parseStatusLine(statusLine)?.ctxPct ?? null}
        chipPlan={planSession ? <PlanChip session={planSession} onPress={() => router.push(`/s/${serverId}/${name}/activity` as never)} /> : null}
        chipLoop={
          stateEvent?.loop_status ? (
            <LoopChip
              status={stateEvent.loop_status}
              iter={stateEvent.loop_iter}
              max={stateEvent.loop_max}
              onPress={() => router.push(`/s/${serverId}/${name}/loop` as never)}
            />
          ) : null
        }
      />
      <MoreSheet open={moreOpen} onClose={() => setMoreOpen(false)} serverId={serverId} name={name} />
      <SessionPickerSheet open={pickerOpen} onClose={() => setPickerOpen(false)} atual={name} />
      {/* Lista e Composer dentro do mesmo KAV: ambos sobem com o teclado e a lista termina acima do composer */}
      <KeyboardAvoidingView behavior="padding" style={styles.body}>
        {stateEvent?.codex_buffering ? (
          <Text style={styles.notice} accessibilityLiveRegion="polite">{m.chat_codex_buffering()}</Text>
        ) : null}
        <View style={styles.inner}>
          {servidorSumiu ? (
            <View style={styles.erro}>
              <Text style={styles.hint}>{m.chat_servidor_removido()}</Text>
              <Text
                style={styles.retry}
                onPress={() => {
                  if (router.canGoBack()) router.back();
                  else router.replace('/');
                }}
                accessibilityRole="button"
              >
                {m.comum_voltar()}
              </Text>
            </View>
          ) : fetchedSession === null ? (
            <View style={styles.erro}>
              <Text style={styles.hint}>{m.chat_sessao_encerrada()}</Text>
              <Text style={styles.retry} onPress={() => router.replace('/')} accessibilityRole="button">
                {m.chat_voltar_sessoes()}
              </Text>
            </View>
          ) : codexPreThread ? (
            <View style={styles.erro}>
              <Text style={styles.hint}>{m.chat_sem_thread_codex()}</Text>
              {currentSession.startup_steps?.length ? (
                <ScrollView style={styles.steps} accessibilityLiveRegion="polite">
                  {currentSession.startup_steps.map((step, index, steps) => (
                    <Text key={index} style={[styles.step,
                      index === steps.length - 1 && currentSession.state === 'working' && styles.currentStep]}>
                      {index + 1}. {step}
                    </Text>
                  ))}
                </ScrollView>
              ) : (
              <Text style={styles.hint} accessibilityLiveRegion="polite">
                {currentSession.label || currentSession.question || m.chat_sem_thread_codex_hint()}
              </Text>
              )}
              <Text style={styles.retry} accessibilityRole="button"
                onPress={() => router.push(`/s/${serverId}/${name}/terminal` as never)}>
                {m.chat_abrir_terminal_codex()}
              </Text>
            </View>
          ) : loading && !error ? (
            <Text style={styles.hint}>{m.chat_carregando_historico()}</Text>
          ) : error ? (
            <View style={styles.erro}>
              <Text style={styles.hint}>{error}</Text>
              <Text style={styles.retry} onPress={chat.retry} accessibilityRole="button">
                {m.lista_tentar_novamente()}
              </Text>
            </View>
          ) : (
            <MessageList
              events={events}
              preview={preview}
              previewMd={previewMd}
              previewFull={previewFull}
              statusLine={statusLine}
              session={currentSession}
              olderFailed={olderFailed}
              onLoadOlder={chat.loadOlder}
              pending={pending}
              optionsSlot={optionsSlot}
              sessionName={name}
              serverId={serverId}
            />
          )}
        </View>
        {sseRecusado && !servidorSumiu && !codexPreThread ? (
          <View style={styles.sseRecusado} accessibilityRole="alert">
            <Text style={styles.sseRecusadoTexto}>{m.chat_sse_recusado()}</Text>
            <Text style={styles.retry} onPress={chat.retry} accessibilityRole="button">
              {m.chat_sse_tentar()}
            </Text>
          </View>
        ) : null}
        {!servidorSumiu && !codexPreThread ? <StatsStrip stats={stats} /> : null}
        {!servidorSumiu && !askOpen && askPayload?.provider === 'codex' ? (
          <Text style={styles.retry} onPress={() => chat.openAsk(askPayload)} accessibilityRole="button">
            {m.ask_perguntas()}
          </Text>
        ) : null}
        {!servidorSumiu ? <TuiPill serverId={serverId} name={name} overlay={!!stateEvent?.overlay} login={!!stateEvent?.login} /> : null}
        {!servidorSumiu && !codexPreThread && fetchedSession !== null ? <Composer serverId={serverId} name={name} draft={draft} sessionProvider={provider} /> : null}
      </KeyboardAvoidingView>
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  body: {
    flex: 1,
  },
  inner: {
    flex: 1,
  },
  hint: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.text.muted,
    textAlign: 'center',
    padding: theme.base.space[6],
  },
  erro: {
    flex: 1,
    justifyContent: 'center',
    gap: theme.base.space[2],
  },
  steps: {
    maxHeight: 320,
    marginHorizontal: theme.base.space[6],
  },
  step: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.text.muted,
    marginBottom: theme.base.space[2],
  },
  currentStep: {
    color: theme.tokens.text.primary,
    fontWeight: '600',
  },
  // Faixa, não tela cheia: a conversa já carregada continua legível — o que parou foi só a
  // atualização ao vivo.
  sseRecusado: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    gap: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    paddingHorizontal: theme.base.space[4],
    backgroundColor: theme.tokens.bg.base,
  },
  sseRecusadoTexto: {
    flexShrink: 1,
    fontSize: theme.base.text.sm,
    color: theme.tokens.text.muted,
  },
  retry: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.accent.base,
    textAlign: 'center',
    minHeight: 44,
    lineHeight: 44,
  },
  aviso: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.status.error,
    textAlign: 'center',
    paddingVertical: theme.base.space[1],
  },
  notice: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.text.secondary,
    paddingVertical: theme.base.space[2],
    paddingHorizontal: theme.base.space[4],
  },
}));
