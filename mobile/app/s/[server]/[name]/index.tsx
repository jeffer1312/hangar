import { useEffect, useRef, useState } from 'react';
import { Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { chatStore } from '../../../../src/stores/chat';
import { useServers } from '../../../../src/stores/servers';
import { useSessions } from '../../../../src/stores/sessions';
import { Screen } from '../../../../src/ui/Screen';
import { ChatHeader } from '../../../../src/chat/ChatHeader';
import { MessageList } from '../../../../src/chat/MessageList';
import { Composer } from '../../../../src/chat/Composer';
import { MoreSheet } from '../../../../src/chat/MoreSheet';
import { OptionButtons } from '../../../../src/chat/OptionButtons';
import { pendingAskFromEvents, askPayloadFromToolUse, getSessions, selectOption, interrupt } from '@hangar/core';
import type { Provider } from '@hangar/core';
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
  const askOpen = chat.use((s) => s.askOpen);
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

  // provider sem retain (regra do cabeçalho) — com fallback por request única pra Pi/Kimi
  const rowsProvider = useSessions((s) => s.rows.find((r) => r.serverId === serverId && r.name === name)?.provider ?? null) as Provider | null;
  const [fetchedProvider, setFetchedProvider] = useState<Provider | null>(null);
  useEffect(() => {
    if (rowsProvider) return;
    let alive = true;
    void getSessions()
      .then((all) => {
        if (!alive) return;
        const hit = all.find((s) => s.name === name);
        if (hit?.provider) setFetchedProvider(hit.provider as Provider);
      })
      .catch(() => {
        if (alive) console.warn('getSessions fallback falhou');
      });
    return () => {
      alive = false;
    };
  }, [rowsProvider, name]);
  const provider: Provider | null = rowsProvider ?? fetchedProvider;

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
              optionsSlot={optionsSlot}
              sessionName={name}
            />
          )}
        </View>
        {!servidorSumiu ? <Composer serverId={serverId} name={name} draft={draft} /> : null}
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
  aviso: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.status.error,
    textAlign: 'center',
    paddingVertical: theme.base.space[1],
  },
}));
