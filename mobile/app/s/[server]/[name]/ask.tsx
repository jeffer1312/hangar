import { useEffect, useRef, useState } from 'react';
import { AccessibilityInfo, Text, View } from 'react-native';
import type { NativeStackNavigationProp } from 'expo-router';
import { StyleSheet } from 'react-native-unistyles';
import { useLocalSearchParams, useNavigation, useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { chatStore } from '../../../../src/stores/chat';
import { useServers } from '../../../../src/stores/servers';
import { AskStepper } from '../../../../src/features/ask/AskStepper';
import { answerQuestions, skipQuestion } from '@hangar/core';
import type { AnswerItem, AskQuestionPayload } from '@hangar/core';
import * as m from '../../../../src/paraglide/messages';
import { answerWithDelivery } from '../../../../src/chat/queueDelivery';
import { useDeliveryMode, useQueueDelivery } from '../../../../src/chat/useQueueDelivery';

const askKey = (payload: AskQuestionPayload) => JSON.stringify([
  payload.interaction_id, payload.request_id, payload.request_id == null ? payload.questions : null,
]);

export default function AskSheet() {
  const router = useRouter();
  const navigation = useNavigation<NativeStackNavigationProp<Record<string, object | undefined>>>();
  const headingRef = useRef<Text>(null);
  const presented = useRef(false);
  const { server, name } = useLocalSearchParams<{ server: string; name: string }>();
  const serverId = Array.isArray(server) ? server[0] : (server ?? '');
  const sessionName = Array.isArray(name) ? name[0] : (name ?? '');
  const chat = chatStore(serverId, sessionName);

  const payload = chat.use((s) => s.askPayload);
  const askOpen = chat.use((s) => s.askOpen);
  const deliveryMode = useDeliveryMode((s) => s.mode);
  const askServer = useServers((s) => s.servers.find((x) => x.id === serverId));
  const delivery = useQueueDelivery(askServer, deliveryMode === 'jev' && !!payload?.is_async && payload.provider === 'codex');
  const [routeError, setRouteError] = useState('');
  const navegando = useRef(false);
  const scope = useRef({ chat, key: payload ? askKey(payload) : null, generation: 0 });
  if (scope.current.chat !== chat || (payload && scope.current.key !== askKey(payload))) {
    scope.current = { chat, key: payload ? askKey(payload) : null, generation: scope.current.generation + 1 };
  }
  const request = scope.current;
  useEffect(() => navigation.addListener('transitionEnd', ({ data }) => {
    presented.current = !data.closing && navigation.isFocused();
    if (presented.current && headingRef.current) AccessibilityInfo.sendAccessibilityEvent(headingRef.current, 'focus');
  }), [navigation]);
  useEffect(() => {
    if (presented.current && navigation.isFocused() && headingRef.current) {
      AccessibilityInfo.sendAccessibilityEvent(headingRef.current, 'focus');
    }
  }, [request, navigation]);
  const inFlight = useRef<typeof request | null>(null);
  const [pendingRequest, setPendingRequest] = useState<typeof request | null>(null);
  const mounted = useRef(true);
  const sameRequest = () => {
    const live = chat.use.getState();
    return mounted.current && scope.current === request
      && (!live.askPayload || askKey(live.askPayload) === request.key);
  };
  const current = () => {
    const live = chat.use.getState();
    return sameRequest() && live.askOpen && live.askPayload && askKey(live.askPayload) === request.key;
  };

  useEffect(() => { setRouteError(''); }, [request]);
  useEffect(() => {
    let previous = chat.use.getState().askPayload;
    return chat.use.subscribe((live) => {
      if (live.askPayload && (!previous || askKey(live.askPayload) !== askKey(previous))) {
        scope.current = { chat, key: askKey(live.askPayload), generation: scope.current.generation + 1 };
      }
      if (previous && !live.askPayload && !inFlight.current) setRouteError('');
      previous = live.askPayload;
    });
  }, [chat]);

  // desmontou por gesto/back = fechou; senão askOpen ficaria true e re-empurraria a rota
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      chat.closeAsk();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [chat]);

  // resposta dada pelo terminal, /clear, ou sucesso -> store fecha e folha sai da pilha
  useEffect(() => {
    if (!askOpen && !pendingRequest && !routeError && !navegando.current && router.canGoBack()) {
      router.back();
    }
  }, [askOpen, pendingRequest, routeError, router]);

  if (!payload) {
    return (
      <View style={styles.empty} accessibilityViewIsModal>
        <Text style={styles.hint} accessibilityLiveRegion="polite" accessibilityState={{ busy: !!pendingRequest }}>
          {pendingRequest ? m.askq_enviando() : m.askq_sua_resposta()}
        </Text>
        {routeError ? <Text style={styles.error} accessibilityRole="alert">{routeError}</Text> : null}
      </View>
    );
  }

  // Cancelar explícito, não o gesto de voltar: pergunta assíncrona do Codex só some pelo backend.
  const handleCancel = (displayed: AskQuestionPayload) => {
    if (!current() || askKey(displayed) !== request.key || inFlight.current === request) return;
    const id = displayed.request_id;
    const interactionId = displayed.interaction_id;
    if (!displayed.is_async || typeof id !== 'string') { chat.closeAsk(); return; }
    const target = useServers.getState().servers.find((s) => s.id === serverId);
    if (!target) { setRouteError(m.chat_servidor_removido()); return; }
    inFlight.current = request;
    setPendingRequest(request);
    setRouteError('');
    skipQuestion(sessionName, id, interactionId, target)
      .then((result) => {
        if (!sameRequest()) return;
        if (result.ok) chat.closeAsk();
        else setRouteError(m.native_action_uncertain());
      })
      .catch((e) => { if (sameRequest()) setRouteError(actionError(e)); })
      .finally(release);
  };

  function actionError(e: unknown) {
    const message = e instanceof Error ? e.message : m.askq_erro_envio();
    return (e as { status?: number })?.status ? message : `${m.native_action_uncertain()} ${message}`;
  }
  function release() {
    if (inFlight.current !== request) return;
    inFlight.current = null;
    if (mounted.current) setPendingRequest(null);
  }

  const handleSubmit = async (answers: AnswerItem[], displayed: AskQuestionPayload) => {
    if (!current() || askKey(displayed) !== request.key || inFlight.current === request) return;
    const target = useServers.getState().servers.find((s) => s.id === serverId);
    if (!target) { setRouteError(m.chat_servidor_removido()); return; }
    inFlight.current = request;
    setPendingRequest(request);
    setRouteError('');
    const requestId = displayed.request_id;
    try {
      const routed = displayed.is_async && displayed.provider === 'codex';
      // No Codex "Na conversa" é o `turn/steer`, que não corta o turno: a resposta entra na hora, como no Direto.
      const steer = routed && (delivery.mode === 'direct' || delivery.mode === 'conversation' || (delivery.mode === 'jev' && delivery.status === 'ready'));
      // Como no composer: Jev escolhido e indisponível responde pela fila, e quem escolheu precisa saber.
      const fallbackNotice = routed && delivery.mode === 'jev' && !steer && delivery.status !== 'loading'
        ? { text: m.mobile_queue_jev_fallback({ reason: delivery.help.replace(/[\s.…]+$/, '') }), warning: true } : null;
      const result = steer ? await answerWithDelivery(target, sessionName, answers, displayed)
        : { ...await answerQuestions(sessionName, answers, requestId, displayed.interaction_id, target), feedback: fallbackNotice };
      if (!sameRequest()) return;
      if (!result.ok) { setRouteError(m.native_action_uncertain()); return; }
      if (result.feedback) chat.showDeliveryFeedback(result.feedback);
      if (result.fallback) navegando.current = true;
      chat.markAskDismissed();
      if (result.fallback) router.dismissTo(`/s/${serverId}/${sessionName}?askFallback=${Date.now()}` as never);
    } catch (e) {
      if (!sameRequest()) return;
      const msg = actionError(e);
      setRouteError(msg);
      const status = (e as { status?: number })?.status;
      if (displayed.provider !== 'codex' && displayed.interaction_id == null && status !== 409) {
        navegando.current = true;
        chat.markAskDismissed();
        router.replace(`/s/${serverId}/${sessionName}/terminal?aviso=${encodeURIComponent(msg)}` as never);
      }
    } finally {
      release();
    }
  };

  return (
    <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.root}>
      <View style={styles.inner} accessibilityViewIsModal onAccessibilityEscape={() => { if (router.canGoBack()) router.back(); }}>
        <Text ref={headingRef} accessible accessibilityRole="header" style={styles.heading}>
          {m.board_precisa_de_voce()}
        </Text>
        <AskStepper key={request.generation} payload={payload} onSubmit={handleSubmit} onClose={handleCancel} />
        {routeError ? (
          <Text style={styles.error} accessibilityRole="alert">
            {routeError}
          </Text>
        ) : null}
      </View>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: {
    flex: 1,
    backgroundColor: theme.tokens.bg.base,
  },
  inner: {
    flex: 1,
  },
  heading: {
    color: theme.tokens.text.primary,
    fontSize: theme.base.text.base,
    fontWeight: theme.base.weight.semibold,
    paddingHorizontal: theme.base.space[4],
    paddingTop: theme.base.space[2],
  },
  empty: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    padding: theme.base.space[4],
  },
  hint: {
    color: theme.tokens.text.muted,
    fontSize: theme.base.text.sm,
  },
  error: {
    color: theme.tokens.status.error,
    fontSize: theme.base.text.sm,
    textAlign: 'center',
    padding: theme.base.space[2],
  },
}));
