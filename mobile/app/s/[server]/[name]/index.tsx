import { useEffect, useRef, useState } from 'react';
import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { chatStore } from '../../../../src/stores/chat';
import { useServers } from '../../../../src/stores/servers';
import { Screen } from '../../../../src/ui/Screen';
import { ChatHeader } from '../../../../src/chat/ChatHeader';
import { MessageList } from '../../../../src/chat/MessageList';
import { Composer } from '../../../../src/chat/Composer';
import { MoreSheet } from '../../../../src/chat/MoreSheet';
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
        onMore={() => setMoreOpen(true)}
      />
      <MoreSheet open={moreOpen} onClose={() => setMoreOpen(false)} serverId={serverId} name={name} />
      {/* Lista e Composer dentro do mesmo KAV: ambos sobem com o teclado e a lista termina acima do composer */}
      <KeyboardAvoidingView behavior="padding" style={styles.body}>
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
              olderFailed={olderFailed}
              onLoadOlder={chat.loadOlder}
              pending={pending}
            />
          )}
        </View>
        {!servidorSumiu ? <Composer serverId={serverId} name={name} /> : null}
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
  retry: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.accent.base,
    textAlign: 'center',
    minHeight: 44,
    lineHeight: 44,
  },
}));
