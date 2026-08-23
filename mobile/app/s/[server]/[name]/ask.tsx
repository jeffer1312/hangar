import { useEffect, useRef, useState } from 'react';
import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { chatStore } from '../../../../src/stores/chat';
import { AskStepper } from '../../../../src/features/ask/AskStepper';
import { answerQuestions } from '@hangar/core';
import type { AnswerItem } from '@hangar/core';
import * as m from '../../../../src/paraglide/messages';

export default function AskSheet() {
  const router = useRouter();
  const { server, name } = useLocalSearchParams<{ server: string; name: string }>();
  const serverId = Array.isArray(server) ? server[0] : (server ?? '');
  const sessionName = Array.isArray(name) ? name[0] : (name ?? '');
  const chat = chatStore(serverId, sessionName);

  // payload copiado no mount — mesmo precedente do untrack em AskQuestionStepper.svelte:32-42
  const [payload] = useState(() => chat.use.getState().askPayload);
  const askOpen = chat.use((s) => s.askOpen);
  const [routeError, setRouteError] = useState('');
  const navegando = useRef(false);

  // desmontou por gesto/back = fechou; senão askOpen ficaria true e re-empurraria a rota
  useEffect(() => {
    return () => {
      chat.closeAsk();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // resposta dada pelo terminal, /clear, ou sucesso -> store fecha e folha sai da pilha
  useEffect(() => {
    if (!askOpen && !navegando.current && router.canGoBack()) {
      router.back();
    }
  }, [askOpen, router]);

  if (!payload) {
    return (
      <View style={styles.empty}>
        <Text style={styles.hint}>{m.askq_sua_resposta()}</Text>
      </View>
    );
  }

  const handleSubmit = async (answers: AnswerItem[]) => {
    setRouteError('');
    try {
      await answerQuestions(sessionName, answers);
      chat.markAskDismissed();
    } catch (e) {
      const msg = e instanceof Error ? e.message : m.askq_erro_envio();
      setRouteError(msg);
      const status = (e as { status?: number })?.status;
      if (status !== 409) {
        navegando.current = true;
        chat.markAskDismissed();
        router.replace(`/s/${serverId}/${sessionName}/terminal?aviso=${encodeURIComponent(msg)}` as never);
      }
      throw e;
    }
  };

  return (
    <KeyboardAvoidingView behavior="padding" style={styles.root}>
      <View style={styles.inner}>
        <AskStepper payload={payload} onSubmit={handleSubmit} onClose={() => chat.closeAsk()} />
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
