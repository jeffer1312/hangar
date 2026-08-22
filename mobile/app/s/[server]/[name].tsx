import { useEffect } from 'react';
import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { chatStore } from '../../../src/stores/chat';
import { Screen } from '../../../src/ui/Screen';
import { ChatHeader } from '../../../src/chat/ChatHeader';
import { MessageList } from '../../../src/chat/MessageList';
import { Composer } from '../../../src/chat/Composer';
import * as m from '../../../src/paraglide/messages';

// Tela de chat de uma sessão: histórico janelado + SSE ao vivo (store chat.ts).
// O composer real entra na Task 9; aqui só o placeholder sticky de 56px.
export default function ChatScreen() {
  const router = useRouter();
  const params = useLocalSearchParams<{ server: string; name: string }>();
  const serverId = Array.isArray(params.server) ? params.server[0] : (params.server ?? '');
  const name = Array.isArray(params.name) ? params.name[0] : (params.name ?? '');

  const chat = chatStore(serverId, name);
  useEffect(() => {
    chat.retain();
    return () => chat.release();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverId, name]);

  const events = chat.use((s) => s.events);
  const stateEvent = chat.use((s) => s.stateEvent);
  const preview = chat.use((s) => s.preview);
  const statusLine = chat.use((s) => s.statusLine);
  const loading = chat.use((s) => s.loading);
  const error = chat.use((s) => s.error);
  const olderFailed = chat.use((s) => s.olderFailed);
  const pending = chat.use((s) => s.pending);

  return (
    <Screen>
      <ChatHeader
        name={name}
        state={stateEvent?.state ?? null}
        onBack={() => {
          if (router.canGoBack()) router.back();
          else router.replace('/');
        }}
      />
      {/* KeyboardAvoidingView dá inset à lista pra não ficar sob o Composer quando o teclado sobe; Composer já sobe via KeyboardStickyView */}
      <KeyboardAvoidingView behavior="padding" style={styles.body}>
        <View style={styles.inner}>
          {loading && !error ? (
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
              statusLine={statusLine}
              olderFailed={olderFailed}
              onLoadOlder={chat.loadOlder}
              pending={pending}
            />
          )}
        </View>
      </KeyboardAvoidingView>
      <Composer serverId={serverId} name={name} />
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
  retry: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.accent.base,
    textAlign: 'center',
    minHeight: 44,
    lineHeight: 44,
  },
}));
