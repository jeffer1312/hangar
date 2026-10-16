import { useEffect, useMemo, useRef, useState } from 'react';
import { AccessibilityInfo, ActivityIndicator, AppState, Platform, ScrollView, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { chatStore } from '../../../../src/stores/chat';
import { useServers } from '../../../../src/stores/servers';
import { useSessions } from '../../../../src/stores/sessions';
import { confirmFirstInput, readFirstInput, useNewConversation } from '../../../../src/stores/newConversation';
import { Screen } from '../../../../src/ui/Screen';
import { ChatHeader } from '../../../../src/chat/ChatHeader';
import { LoopChip } from '../../../../src/chat/LoopChip';
import { useActivity } from '../../../../src/features/activity/useActivity';
import { MessageList } from '../../../../src/chat/MessageList';
import { pararTts } from '../../../../src/chat/BubbleActions';
import { Composer } from '../../../../src/chat/Composer';
import { ComposerStatusLine } from '../../../../src/chat/ComposerStatusLine';
import { KeyboardBar } from '../../../../src/chat/KeyboardBar';
import { OrqFooter } from '../../../../src/chat/OrqFooter';
import { TuiPill } from '../../../../src/chat/TuiPill';
import { RecarregarPill } from '../../../../src/chat/RecarregarPill';
import { MoreSheet } from '../../../../src/chat/MoreSheet';
import { PluginUi } from '../../../../src/chat/PluginUi';
import { OptionButtons } from '../../../../src/chat/OptionButtons';
import { PendingPlan } from '../../../../src/chat/PendingPlan';
import { SessionProblem } from '../../../../src/chat/SessionProblem';
import { SessionPickerSheet } from '../../../../src/chat/SessionPickerSheet';
import { SessionsDrawer } from '../../../../src/features/sessions/SessionsDrawer';
import { ServerSheet } from '../../../../src/features/sessions/ServerSheet';
import { useLiveVoice } from '../../../../src/features/voice/liveVoiceStore';
import { PlanActions } from '../../../../src/chat/PlanActions';
import { pendingAskFromEvents, askPayloadFromToolUse, fetchSessionsForServer, isOrq, selectOptionForServer, submitSelectedForServer, interrupt, recarregarSessao, implementCodexPlan, pendingProposedPlan, pendingReplyPlan, setPermissionMode, currentQueuePanel } from '@hangar/core';
import type { Provider, SessionInfo } from '@hangar/core';
import * as m from '../../../../src/paraglide/messages';

// Tela de chat de uma sessão: histórico janelado + SSE ao vivo (store chat.ts).
// A conversa para antes da curva de baixo da caixa: senão o texto vaza pelos cantos arredondados.
const BORDA_DOCK = 20;

export default function ChatScreen() {
  const router = useRouter();
  const params = useLocalSearchParams<{ server: string; name: string; askFallback?: string }>();
  const serverId = Array.isArray(params.server) ? params.server[0] : (params.server ?? '');
  const name = Array.isArray(params.name) ? params.name[0] : (params.name ?? '');
  const rota = `${serverId}::${name}`;

  const chat = chatStore(serverId, name);
  const [servidorSumiu, setServidorSumiu] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [serversOpen, setServersOpen] = useState(false);
  // Sem mods, a conversa passa sob o vidro; com eles, a lista precisa parar antes da faixa.
  const [dockH, setDockH] = useState(0);
  const [modsH, setModsH] = useState(0);
  const overlayH = modsH > 0 ? 0 : dockH;
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
      // Conversa de fora usa a entrada do par como ativa; ao sair, a ativa volta a ser máquina própria.
      useServers.getState().ensureOwnActive();
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
  const bufferingAnnounced = useRef(false);
  useEffect(() => {
    const buffering = !!stateEvent?.codex_buffering;
    if (buffering && !bufferingAnnounced.current && Platform.OS === 'ios') {
      AccessibilityInfo.announceForAccessibilityWithOptions(m.chat_codex_buffering(), { queue: true });
    }
    bufferingAnnounced.current = buffering;
  }, [stateEvent?.codex_buffering]);
  const preview = chat.use((s) => s.preview);
  const previewMd = chat.use((s) => s.previewMd);
  const previewFull = chat.use((s) => s.previewFull);
  const pensamento = chat.use((s) => s.pensamento);
  const ferramenta = chat.use((s) => s.ferramenta);
  const turnSeen = chat.use((s) => s.turnSeen);
  // Mesma fonte do painel Atividade: um Agent em background devolve resultado na hora e só o fold
  // sabe que ele segue rodando.
  const activity = useActivity(events);
  const agentesRodando = useMemo(() => activity.agents.filter((a) => a.running), [activity]);
  const loading = chat.use((s) => s.loading);
  const error = chat.use((s) => s.error);
  const olderFailed = chat.use((s) => s.olderFailed);
  const sseRecusado = chat.use((s) => s.sseRecusado);
  const pending = chat.use((s) => s.pending);
  // O mesmo painel que o Composer desenha: as mensagens dele saem da conversa enquanto esperam.
  const serverQueue = chat.use((s) => s.queuePanel) ?? null;
  const queuePanel = useMemo(() => currentQueuePanel(serverQueue, events), [serverQueue, events]);
  const askOpen = chat.use((s) => s.askOpen);
  const askPayload = chat.use((s) => s.askPayload);
  const askPiId = chat.use((s) => s.askPiId);
  const askPiDismissed = chat.use((s) => s.askPiDismissed);

  // draft devolvido pelo cancelar do picker (Task 3)
  const [draft, setDraft] = useState<{ route: string; text: string } | null>(null);
  // Pendente devolvido pelo Parar/cancelar. Objeto novo a cada devolução: o mesmo texto duas vezes
  // ainda é adotado; o Composer avisa quando adotou e ele sai daqui.
  const [returned, setReturned] = useState<{ route: string; value: { text: string } } | null>(null);
  const firstAttempt = useNewConversation((s) => s.attempts[serverId]);
  const firstBusy = useNewConversation((s) => !!s.busy[serverId]);
  const firstIssue = useNewConversation((s) => s.issues[serverId]);
  const firstInput = firstAttempt?.sessionName === name ? firstAttempt : null;
  const handedOff = useRef<string | null>(null);
  const [handoffError, setHandoffError] = useState<{ route: string; text: string } | null>(null);
  useEffect(() => {
    if (!ready) return;
    try {
      const snapshot = readFirstInput(serverId, name);
      if (!snapshot) { handedOff.current = null; return; }
      if (snapshot.phase === 'sent') {
        setDraft((current) => current?.route === rota ? null : current);
        if (!firstBusy) confirmFirstInput(snapshot.id);
      } else if (handedOff.current !== `${rota}:${snapshot.id}`) {
        handedOff.current = `${rota}:${snapshot.id}`;
        setDraft({ route: rota, text: snapshot.text });
      }
      setHandoffError(null);
    } catch {
      setHandoffError({ route: rota, text: m.nova_conversa_resultado_salvar_erro() });
    }
  }, [ready, serverId, name, rota, firstAttempt, firstBusy]);
  const [aviso, setAviso] = useState('');
  const avisoTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  function mostrarAviso(e: unknown) {
    const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : m.comum_falha_envio_opcao();
    setAviso(msg);
    if (avisoTimer.current) clearTimeout(avisoTimer.current);
    avisoTimer.current = setTimeout(() => setAviso(''), 8000);
  }
  useEffect(() => {
    if (params.askFallback) mostrarAviso(m.native_ask_fallback());
    return () => { if (avisoTimer.current) clearTimeout(avisoTimer.current); };
  }, [params.askFallback, serverId, name]);

  // Sem SSE adicional: acompanha a abertura e o Git do Codex pela lista.
  const rowsProvider = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.provider ?? null) as Provider | null;
  const [fetchedSession, setFetchedSession] = useState<SessionInfo | null | undefined>(undefined);
  const listSession = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name) ?? null);
  const currentSession = fetchedSession === undefined ? listSession : fetchedSession;
  const codexPreThread = currentSession?.provider === 'codex' && currentSession.tracked === false;
  useEffect(() => {
    if (!ready || servidorSumiu) return;
    setFetchedSession(undefined);
    let alive = true;
    let active = (AppState.currentState ?? 'active') === 'active';
    let generation = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      if (!alive || !active) return;
      const server = useServers.getState().servers.find((s) => s.id === serverId);
      if (!server) return;
      const g = generation;
      try {
        const all = await fetchSessionsForServer(server);
        if (!alive || !active || g !== generation) return;
        const hit = all.find((s) => s.name === name);
        setFetchedSession(hit ?? null);
        if (hit?.provider === 'codex') timer = setTimeout(refresh, hit.tracked === false ? 2000 : 5000);
      } catch {
        if (alive && active && g === generation) {
          console.warn('fetchSessionsForServer falhou');
          timer = setTimeout(refresh, 5000);
        }
      }
    };
    const subscription = AppState.addEventListener('change', (state) => {
      const next = state === 'active';
      if (next === active) return;
      active = next;
      generation++;
      clearTimeout(timer);
      if (active) void refresh();
    });
    void refresh();
    return () => {
      alive = false;
      clearTimeout(timer);
      subscription.remove();
    };
  }, [ready, servidorSumiu, serverId, name]);
  const provider: Provider | null = codexPreThread ? 'codex' : rowsProvider ?? fetchedSession?.provider ?? null;
  const orq = isOrq({ provider });
  // Par em "ver e mandar recado": a conversa é de outra pessoa e só se acompanha.
  const soLeitura = currentSession?.guest_kind === 'pair';
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

  // A pergunta da lista (fora do quadro ou segurada pelo plugin) abre o passo a passo na hora, com o ID do
  // formulário: responde pelo `/answer` sem esperar o stream. Uma vez por formulário, para fechar não reabrir.
  const listForm = currentSession?.ask_form;
  const listFormOpened = useRef<string | null>(null);
  useEffect(() => {
    const id = listForm?.interaction_id;
    if (!listForm?.questions?.length || !id || listFormOpened.current === id) return;
    // Visto antes de decidir: o formulário que o stream já abriu (e foi respondido) não reabre pela lista atrasada.
    listFormOpened.current = id;
    if (askOpen || askPayload?.interaction_id === id) return;
    chat.openAsk(listForm);
  }, [listForm, askOpen, askPayload, chat]);

  // quem responde: OptionButtons quando awaiting_input com question/options e sem stepper aberto
  const showOptions = !!(!askOpen && stateEvent?.state === 'awaiting_input' && stateEvent.question && stateEvent.options?.length);
  const optionKey = JSON.stringify([serverId, name, showOptions, stateEvent?.interaction_id ?? [stateEvent?.question, stateEvent?.options]]);
  const optionScope = useRef({ key: optionKey, generation: 0 });
  if (optionScope.current.key !== optionKey) {
    optionScope.current = { key: optionKey, generation: optionScope.current.generation + 1 };
  }
  const optionRequest = optionScope.current;
  const optionsMounted = useRef(true);
  const liveOptionKey = () => {
    const live = chat.use.getState();
    return JSON.stringify([
      serverId, name, !live.askOpen && live.stateEvent?.state === 'awaiting_input'
        && !!live.stateEvent.question && !!live.stateEvent.options?.length,
      live.stateEvent?.interaction_id ?? [live.stateEvent?.question, live.stateEvent?.options],
    ]);
  };
  const optionCurrent = () => optionsMounted.current && optionScope.current === optionRequest && liveOptionKey() === optionKey;
  useEffect(() => {
    optionsMounted.current = true;
    const unsubscribe = chat.use.subscribe(() => {
      const key = liveOptionKey();
      if (optionScope.current.key !== key) {
        optionScope.current = { key, generation: optionScope.current.generation + 1 };
      }
    });
    return () => { unsubscribe(); optionsMounted.current = false; };
  }, [chat, serverId, name]);
  const optionError = (e: unknown) => {
    if (!optionCurrent()) return;
    const msg = e instanceof Error ? e.message : m.comum_falha_envio_opcao();
    mostrarAviso((e as { status?: number })?.status || /^\d{3}: /.test(msg) ? msg : `${m.native_action_uncertain()} ${msg}`);
  };
  // Cada ação copia o servidor da rota no toque: removido vira aviso, nunca cai no ativo.
  const destino = () => useServers.getState().servers.find((s) => s.id === serverId);
  const handleSelectOption = async (n: number, interactionId: string | null | undefined, text?: string) => {
    if (!optionCurrent()) return;
    const target = destino();
    if (!target) return mostrarAviso(m.chat_servidor_removido());
    try { await selectOptionForServer(target, name, n, interactionId, text); }
    catch (e) { optionError(e); }
  };
  // Escolha múltipla: as marcadas já foram para o terminal, aqui só confirma.
  const handleSubmitOptions = async (interactionId: string | null | undefined) => {
    if (!optionCurrent()) return;
    const target = destino();
    if (!target) return mostrarAviso(m.chat_servidor_removido());
    try { await submitSelectedForServer(target, name, interactionId); }
    catch (e) { optionError(e); }
  };
  // Resposta que chega depois de a rota trocar de conversa não mexe na conversa nova.
  const rotaAtual = useRef(rota);
  rotaAtual.current = rota;
  // Um objeto por toque: a resposta de um Parar antigo só libera a trava que ela mesma criou.
  const stopEmVoo = useRef<{ rota: string } | null>(null);
  const [stopVivo, setStopVivo] = useState<{ rota: string } | null>(null);
  const stopping = stopVivo?.rota === rota;
  const handleStop = () => {
    if (stopEmVoo.current?.rota === rota) return;
    const target = destino();
    if (!target) return mostrarAviso(m.chat_servidor_removido());
    const tok = { rota };
    stopEmVoo.current = tok;
    setStopVivo(tok);
    // O Claude Code mantém a mensagem enfileirada no campo dele ao interromper: ela volta para o
    // composer (editável) e o segundo Esc limpa o terminal. Sem pendente, interrupção simples.
    const cur = chat.use.getState().pending;
    const last = cur.length ? cur[cur.length - 1] : null;
    void interrupt(name, !!last, target)
      .then(() => {
        if (!last || !chat.use.getState().pending.some((p) => p.id === last.id)) return;
        chat.use.setState((live) => ({ pending: live.pending.filter((p) => p.id !== last.id) }));
        // Só depois do Esc confirmado: com falha a mensagem segue na fila e voltaria em dobro.
        setReturned({ route: rota, value: { text: last.text } });
      })
      .catch((e) => {
        if (rotaAtual.current === rota) mostrarAviso(e);
      })
      .finally(() => {
        if (stopEmVoo.current !== tok) return;
        stopEmVoo.current = null;
        setStopVivo(null);
      });
  };
  // Recarregar (Claude): recicla o processo na mesma conversa pra reler MCP/hooks/settings; com
  // terminal, o pane reabre com o comando de hoje. O motivo só existe sem terminal (vem do `state`);
  // sem motivo a ação fica só no "⋯".
  const recarregavel = currentSession?.provider === 'claude';
  const semTerminal = !!(stateEvent?.headless ?? currentSession?.headless);
  // Só o stream da sessão diz `dead`: aí não há para quem escrever, o composer vira o aviso.
  const dead = stateEvent?.state === 'dead';
  useEffect(() => {
    if (servidorSumiu) return;
    return useLiveVoice.getState().registerChat({ serverId, name,
      terminal: !!currentSession && !orq && !semTerminal && !soLeitura && !dead && !codexPreThread,
      controls: !orq && !soLeitura && !dead && !codexPreThread && fetchedSession !== null });
  }, [serverId, name, servidorSumiu, currentSession, orq, semTerminal, soLeitura, dead, codexPreThread, fetchedSession]);
  const [recarregando, setRecarregando] = useState(false);
  const recarregarBloqueado = stateEvent?.state !== 'idle' || recarregando;
  const recarregar = () => {
    if (recarregando) return;
    const target = destino();
    if (!target) return mostrarAviso(m.chat_servidor_removido());
    setRecarregando(true);
    void recarregarSessao(name, target).catch((e) => mostrarAviso(e)).finally(() => setRecarregando(false));
  };
  const handleCancelOptions = async () => {
    if (!optionCurrent()) return;
    const target = destino();
    if (!target) return mostrarAviso(m.chat_servidor_removido());
    const cur = chat.use.getState().pending;
    const last = cur.length ? cur[cur.length - 1] : null;
    try {
      await interrupt(name, !!last, target);
      if (last && chat.use.getState().pending.some((p) => p.id === last.id)) {
        chat.use.setState((live) => ({ pending: live.pending.filter((p) => p.id !== last.id) }));
        setReturned({ route: rota, value: { text: last.text } });
      }
    } catch (e) { optionError(e); }
  };
  const headless = !!(stateEvent?.headless ?? currentSession?.headless);
  // Plano à espera de decisão: o do Codex vem marcado na resposta; no Claude sem terminal em modo
  // plano ninguém pergunta "implementar?", então a última resposta do turno é o plano.
  const decidablePlan = useMemo(() => {
    if (provider === 'codex') return pendingProposedPlan(events);
    if (provider === 'claude' && headless && stateEvent?.claude_permission_mode === 'plan' && stateEvent.state === 'idle') {
      return pendingReplyPlan(events);
    }
    return null;
  }, [provider, headless, events, stateEvent?.claude_permission_mode, stateEvent?.state]);
  const implementPlan = async (plan: string) => {
    const live = chat.use.getState();
    if (live.stateEvent?.state !== 'idle' || plan !== decidablePlan?.plan) throw new Error(m.chat_plan_indisponivel());
    const target = destino();
    if (!target) throw new Error(m.chat_servidor_removido());
    if (provider === 'codex') {
      await implementCodexPlan(name, target);
      return;
    }
    await setPermissionMode(name, live.stateEvent?.claude_previous_non_plan || 'acceptEdits', target);
    try {
      await chat.send(m.chat_plan_pedido());
    } catch (err) {
      // Pedido não saiu: a sessão não pode ficar fora do modo plano sem ter implementado nada.
      try {
        await setPermissionMode(name, 'plan', target);
      } catch (revertErr) {
        console.warn('plan: revert to plan mode failed', name, revertErr);
        throw new Error(m.native_plan_send_failed_stuck({ reason: err instanceof Error ? err.message : String(err) }));
      }
      throw err;
    }
  };
  // O plano vem antes das ações: a pessoa lê o que vai aprovar. Sem ações, fica só o plano.
  const planPending = stateEvent?.claude_plan_pending;
  const optionsSlot = showOptions || planPending || decidablePlan || aviso ? (
    <View style={styles.slot}>
      {planPending ? (
        <PendingPlan key={planPending.tool_use_id ?? undefined} {...planPending}
                     {...(showOptions ? { options: stateEvent!.options!, question: stateEvent!.question!, interactionId: stateEvent!.interaction_id,
                                          onSelect: handleSelectOption, onCancel: handleCancelOptions } : {})} />
      ) : null}
      {decidablePlan && !planPending ? (
        <PlanActions key={decidablePlan.id} plan={decidablePlan.plan} onImplement={implementPlan}
                     disabled={stateEvent?.state !== 'idle' || pending.length > 0} />
      ) : null}
      {showOptions && !planPending ? (
        <OptionButtons key={optionRequest.generation} question={stateEvent!.question!} options={stateEvent!.options!} interactionId={stateEvent!.interaction_id} onSelect={handleSelectOption} onCancel={handleCancelOptions} onSubmit={handleSubmitOptions} />
      ) : null}
      {aviso ? <Text style={styles.aviso} accessibilityRole="alert">{aviso}</Text> : null}
    </View>
  ) : undefined;
  // Antes da thread o SSE da conversa ainda não publica o problema; a lista (repetida a cada 2 s) sim.
  const problem = codexPreThread ? currentSession.problema : stateEvent?.problema;
  const problemDetail = codexPreThread ? null : stateEvent?.problema_detalhe;

  // A lista sai deste chat antes de empurrar outro destino; Nova conversa pede o campo na inicial.
  return (
    <Screen edges={[]}>
      <SessionsDrawer onClose={() => router.dismissTo('/')} onOpenServers={() => setServersOpen(true)}
        onNewConversation={() => router.dismissTo({ pathname: '/', params: { compose: '1' } })}>
        <ChatHeader
          name={name}
          state={codexPreThread ? currentSession.state : stateEvent?.state ?? null}
          onBack={() => {
            if (router.canGoBack()) router.back();
            else router.replace('/');
          }}
          onMore={() => setMoreOpen(true)}
          onTitlePress={() => setPickerOpen(true)}
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
        <MoreSheet open={moreOpen} onClose={() => setMoreOpen(false)} serverId={serverId} name={name} orq={orq}
                   provider={provider} semTerminal={semTerminal} temPergunta={!!askPayload}
                   recarregar={recarregavel ? { bloqueado: recarregarBloqueado, onPress: recarregar } : undefined} />
        <SessionPickerSheet open={pickerOpen} onClose={() => setPickerOpen(false)} atual={name} serverId={serverId} />
        {/* Lista e Composer dentro do mesmo KAV: ambos sobem com o teclado e a lista termina acima do composer */}
        <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.body}>
          {stateEvent?.codex_buffering ? (
            <Text style={styles.notice} accessibilityLiveRegion="polite">{m.chat_codex_buffering()}</Text>
          ) : null}
          {handoffError?.route === rota || firstInput && firstInput.phase !== 'sent' && firstIssue ? (
            <Text style={styles.aviso} accessibilityRole="alert">
              {handoffError?.route === rota ? handoffError.text : firstIssue?.message}
            </Text>
          ) : firstInput?.phase === 'send_unknown' ? (
            <Text style={styles.aviso} accessibilityRole="alert">{m.nova_conversa_envio_incerto()}</Text>
          ) : null}
          {!servidorSumiu && problem ? (
            <View style={styles.problem}><SessionProblem problem={problem} detail={problemDetail} /></View>
          ) : null}
          <View testID="chat-viewport" style={[styles.inner, { marginBottom: -Math.max(0, overlayH - BORDA_DOCK) }]}>
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
              <View style={styles.carregando}>
                <ActivityIndicator />
                <Text style={styles.hint}>{m.chat_carregando_historico()}</Text>
              </View>
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
                session={currentSession}
                olderFailed={olderFailed}
                onLoadOlder={chat.loadOlder}
                pending={pending}
                optionsSlot={optionsSlot}
                sessionName={name}
                serverId={serverId}
                bottomInset={overlayH}
                stateEvent={stateEvent}
                turnSeen={turnSeen}
                pensamento={pensamento}
                ferramenta={ferramenta}
                agentesRodando={agentesRodando}
                onAbrirAgentes={() => router.push(`/s/${serverId}/${name}/activity` as never)}
                queuePanel={queuePanel}
                canQuote={!servidorSumiu && fetchedSession !== null && !dead && !orq && !soLeitura}
              />
            )}
          </View>
          <View testID="chat-dock" onLayout={(e) => setDockH(e.nativeEvent.layout.height)}>
          {sseRecusado && !servidorSumiu && !codexPreThread ? (
            <View style={styles.sseRecusado} accessibilityRole="alert">
              <Text style={styles.sseRecusadoTexto}>{m.chat_sse_recusado()}</Text>
              <Text style={styles.retry} onPress={chat.retry} accessibilityRole="button">
                {m.chat_sse_tentar()}
              </Text>
            </View>
          ) : null}
          {!servidorSumiu && !askOpen && askPayload?.provider === 'codex' ? (
            <Text style={styles.retry} onPress={() => chat.openAsk(askPayload)} accessibilityRole="button">
              {m.ask_perguntas()}
            </Text>
          ) : null}
          {!servidorSumiu ? <TuiPill serverId={serverId} name={name} overlay={!!stateEvent?.overlay} login={!!stateEvent?.login} /> : null}
          {!servidorSumiu && recarregavel && semTerminal ? <RecarregarPill motivo={stateEvent?.recarregar_motivo} bloqueado={recarregarBloqueado} onPress={recarregar} /> : null}
          <View testID="mods-dock" onLayout={(e) => setModsH(e.nativeEvent.layout.height)}>
            {!servidorSumiu && !codexPreThread && !dead && fetchedSession !== null
              ? <PluginUi key={rota} serverId={serverId} name={name} readOnly={soLeitura} /> : null}
          </View>
          {!servidorSumiu && !codexPreThread && fetchedSession !== null
            ? dead
              ? (
                <View style={styles.deadFooter}>
                  <Text style={styles.deadText}>{m.chat_sessao_encerrada()}</Text>
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
              )
              : orq
              ? <OrqFooter serverId={serverId} arbiter={currentSession?.orq_arbiter} />
              : soLeitura
              ? <Text style={styles.notice} accessibilityRole="text">{m.native_par_so_leitura()}</Text>
              : (
                <>
                  <Composer key={rota} serverId={serverId} name={name} draft={draft?.route === rota ? draft.text : undefined}
                            returned={returned?.route === rota ? returned.value : undefined}
                            onReturnedAdopted={() => setReturned((cur) => (cur?.route === rota ? null : cur))}
                            firstInputId={firstInput?.id} firstInputSent={firstInput?.phase === 'sent'} sessionProvider={provider}
                            headless={headless} onStop={handleStop} stopping={stopping} queuePanel={queuePanel} />
                </>
              )
            : null}
          </View>
          {/* Linha de status do app de PC, colada embaixo da caixa e fora da área que a conversa cobre. */}
          {!servidorSumiu && !codexPreThread && fetchedSession !== null && !orq && !soLeitura && !dead
            ? <ComposerStatusLine key={`status:${rota}`} serverId={serverId} name={name} />
            : null}
          <KeyboardBar />
        </KeyboardAvoidingView>
      </SessionsDrawer>
      <ServerSheet open={serversOpen} onFechar={() => setServersOpen(false)} />
    </Screen>
  );
}

const styles = StyleSheet.create((theme) => ({
  body: {
    flex: 1,
  },
  carregando: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 8 },
  inner: {
    flex: 1,
    overflow: 'hidden',
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
  // Ação de recuperação tem cara de botão: texto azul solto passava por link.
  retry: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
    color: theme.tokens.accent.base,
    textAlign: 'center',
    alignSelf: 'center',
    minHeight: 40,
    lineHeight: 38,
    paddingHorizontal: theme.base.space[4],
    marginVertical: theme.base.space[1],
    borderWidth: 1,
    borderColor: theme.tokens.accent.base,
    borderRadius: theme.base.radius.full,
    overflow: 'hidden',
  },
  problem: {
    paddingHorizontal: theme.base.space[3],
    paddingTop: theme.base.space[2],
  },
  slot: {
    gap: theme.base.space[2],
  },
  aviso: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.status.error,
    textAlign: 'center',
    paddingVertical: theme.base.space[1],
  },
  deadFooter: {
    alignItems: 'center',
    gap: theme.base.space[1],
    paddingVertical: theme.base.space[3],
    paddingHorizontal: theme.base.space[4],
  },
  deadText: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.text.muted,
    textAlign: 'center',
  },
  notice: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.text.secondary,
    paddingVertical: theme.base.space[2],
    paddingHorizontal: theme.base.space[4],
  },
}));
