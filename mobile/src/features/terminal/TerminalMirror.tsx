import { useCallback, useRef, useState } from 'react';
import { Pressable, Text, TextInput, View, ActivityIndicator } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { LegendList, type LegendListRef } from '@legendapp/list/react-native';
import { getPane, sendKey, sendTermInput } from '@hangar/core';
import type { NavKey } from '@hangar/core';
import { usePanePoll } from './usePanePoll';
import * as m from '../../paraglide/messages';

interface Props {
  name: string;
  aviso?: string;
}

const BOTTOM_SLOP = 24;
const LINES_MAX = 1200;

export function TerminalMirror({ name, aviso }: Props) {
  const { theme } = useUnistyles();
  const [lines, setLines] = useState(200);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [localErr, setLocalErr] = useState(aviso ?? '');
  const [loadingMore, setLoadingMore] = useState(false);

  const { text, scrollback, err: pollErr, pending, refresh, setAtBottom } = usePanePoll(name, lines);
  const err = localErr || pollErr;

  const listRef = useRef<LegendListRef>(null);
  const [atBottomLocal, setAtBottomLocal] = useState(true);

  const handleScroll = useCallback(
    (e: { nativeEvent: { contentOffset: { y: number }; contentSize: { height: number }; layoutMeasurement: { height: number } } }) => {
      const { contentOffset, contentSize, layoutMeasurement } = e.nativeEvent;
      const dist = contentSize.height - contentOffset.y - layoutMeasurement.height;
      const now = dist <= BOTTOM_SLOP;
      setAtBottomLocal(now);
      setAtBottom(now);
    },
    [setAtBottom],
  );

  const toBottom = useCallback(() => {
    listRef.current?.scrollToEnd({ animated: true });
    setAtBottomLocal(true);
    setAtBottom(true);
  }, [setAtBottom]);

  const canLoadMore = scrollback > 0 && lines < LINES_MAX;

  const loadMore = useCallback(async () => {
    if (loadingMore || !canLoadMore) return;
    setLoadingMore(true);
    const next = lines === 200 ? 600 : 1200;
    try {
      await getPane(name, next);
      setLines(next);
      await refresh();
      setLocalErr('');
    } catch (e) {
      setLocalErr(e instanceof Error ? e.message : m.term_erro());
    } finally {
      setLoadingMore(false);
    }
  }, [loadingMore, canLoadMore, lines, name, refresh]);

  const press = useCallback(
    async (key: NavKey | 'C-c') => {
      if (busy) return;
      setBusy(true);
      setLocalErr('');
      try {
        if (key === 'C-c') {
          await sendTermInput(name, { key: 'C-c' });
        } else {
          await sendKey(name, key as NavKey);
        }
        await refresh();
        toBottom();
      } catch (e) {
        setLocalErr(e instanceof Error ? e.message : m.term_erro());
      } finally {
        setBusy(false);
      }
    },
    [busy, name, refresh, toBottom],
  );

  const submitDraft = useCallback(
    async (withEnter: boolean) => {
      const value = draft;
      if (!value || busy) return;
      setBusy(true);
      setLocalErr('');
      try {
        if (withEnter) {
          await sendTermInput(name, { text: value, key: 'Enter' });
        } else {
          await sendTermInput(name, { text: value });
        }
        setDraft('');
        await refresh();
        toBottom();
      } catch (e) {
        setLocalErr(e instanceof Error ? e.message : m.term_erro_enviar());
      } finally {
        setBusy(false);
      }
    },
    [draft, busy, name, refresh, toBottom],
  );

  // linhas para render — trim só fim, como Svelte
  const displayLines = text.replace(/\s+$/, '').split('\n');

  return (
    <View style={styles.root}>
      {canLoadMore ? (
        <Pressable
          onPress={loadMore}
          disabled={loadingMore}
          style={[styles.loadMore, { borderColor: theme.tokens.border.subtle, backgroundColor: theme.tokens.bg.elevated }]}
          accessibilityRole="button"
          accessibilityLabel={m.term_carregar_mais({ n: lines })}
        >
          {loadingMore ? (
            <ActivityIndicator size="small" color={theme.tokens.text.muted} />
          ) : (
            <Text style={[styles.loadMoreText, { color: theme.tokens.text.muted }]}>{m.term_carregar_mais({ n: lines })}</Text>
          )}
        </Pressable>
      ) : null}

      <LegendList
        ref={listRef}
        data={displayLines}
        keyExtractor={(_, i) => String(i)}
        renderItem={({ item }) => (
          <Text
            style={[
              styles.mono,
              {
                color: theme.tokens.text.primary,
                fontFamily: theme.base.fontMono,
              },
            ]}
            selectable
          >
            {item || ' '}
          </Text>
        )}
        estimatedItemSize={14}
        alignItemsAtEnd
        initialScrollAtEnd
        maintainScrollAtEnd
        onScroll={handleScroll}
        scrollEventThrottle={16}
        keyboardShouldPersistTaps="handled"
        style={styles.scroll}
        contentContainerStyle={styles.scrollContent}
      />

      {err ? (
        <Text style={[styles.err, { color: theme.tokens.status.error }]} accessibilityRole="alert">
          {err}
        </Text>
      ) : null}

      {!atBottomLocal ? (
        <Pressable
          onPress={toBottom}
          style={[styles.pending, { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle }]}
          accessibilityRole="button"
        >
          <Text style={[styles.pendingText, { color: theme.tokens.text.secondary }]}>
            {pending ? m.term_pausado_saida() : m.term_pausado()} {m.term_voltar_fim()}
          </Text>
        </Pressable>
      ) : null}

      {/* campo de texto + enviar */}
      <View style={[styles.compose, { borderTopColor: theme.tokens.border.subtle, backgroundColor: theme.tokens.bg.base }]}>
        <TextInput
          value={draft}
          onChangeText={setDraft}
          placeholder={m.term_digitar()}
          placeholderTextColor={theme.tokens.text.muted}
          style={[
            styles.input,
            {
              color: theme.tokens.text.primary,
              borderColor: theme.tokens.border.default,
              backgroundColor: theme.tokens.bg.surface,
              fontFamily: theme.base.fontMono,
            },
          ]}
          autoCapitalize="none"
          autoCorrect={false}
          spellCheck={false}
          enterKeyHint="send"
          onSubmitEditing={() => submitDraft(true)}
          accessibilityLabel={m.term_texto_para()}
        />
        <Pressable
          onPress={() => submitDraft(false)}
          disabled={!draft || busy}
          style={[styles.sendBtn, { backgroundColor: theme.tokens.bg.elevated, opacity: !draft || busy ? 0.5 : 1 }]}
          accessibilityLabel={m.term_enviar_sem_enter()}
          accessibilityRole="button"
        >
          <Text style={[styles.sendText, { color: theme.tokens.text.primary }]}>↵̸</Text>
        </Pressable>
        <Pressable
          onPress={() => submitDraft(true)}
          disabled={!draft || busy}
          style={[styles.sendBtn, styles.sendPrimary, { backgroundColor: theme.tokens.accent.base, opacity: !draft || busy ? 0.5 : 1 }]}
          accessibilityLabel={m.term_enviar_com_enter()}
          accessibilityRole="button"
        >
          <Text style={[styles.sendText, { color: theme.tokens.text.inverse }]}>{m.term_envia()}</Text>
        </Pressable>
      </View>

      {/* barra de teclas de resgate */}
      <View
        style={[styles.keysBar, { borderTopColor: theme.tokens.border.subtle, backgroundColor: theme.tokens.bg.surface }]}
        accessibilityLabel={m.term_teclas_resgate()}
      >
        <Text style={[styles.keysHint, { color: theme.tokens.text.muted }]}>{m.term_resgate()}</Text>
        <View style={styles.keysRow}>
          <Pressable
            onPress={() => press('Escape')}
            disabled={busy}
            style={[styles.key, { borderColor: theme.tokens.border.default, opacity: busy ? 0.5 : 1 }]}
            accessibilityRole="button"
            accessibilityLabel="Escape"
          >
            <Text style={[styles.keyText, { color: theme.tokens.text.primary }]}>Esc</Text>
          </Pressable>
          <Pressable
            onPress={() => press('Up')}
            disabled={busy}
            style={[styles.key, { borderColor: theme.tokens.border.default, opacity: busy ? 0.5 : 1 }]}
            accessibilityRole="button"
            accessibilityLabel={m.term_seta_cima()}
          >
            <Text style={[styles.keyText, { color: theme.tokens.text.primary }]}>↑</Text>
          </Pressable>
          <Pressable
            onPress={() => press('Down')}
            disabled={busy}
            style={[styles.key, { borderColor: theme.tokens.border.default, opacity: busy ? 0.5 : 1 }]}
            accessibilityRole="button"
            accessibilityLabel={m.term_seta_baixo()}
          >
            <Text style={[styles.keyText, { color: theme.tokens.text.primary }]}>↓</Text>
          </Pressable>
          <Pressable
            onPress={() => press('Enter')}
            disabled={busy}
            style={[styles.key, styles.keyEnter, { borderColor: theme.tokens.border.default, opacity: busy ? 0.5 : 1 }]}
            accessibilityRole="button"
            accessibilityLabel="Enter"
          >
            <Text style={[styles.keyText, { color: theme.tokens.text.primary }]}>⏎</Text>
          </Pressable>
          <Pressable
            onPress={() => press('Tab')}
            disabled={busy}
            style={[styles.key, { borderColor: theme.tokens.border.default, opacity: busy ? 0.5 : 1 }]}
            accessibilityRole="button"
            accessibilityLabel="Tab"
          >
            <Text style={[styles.keyText, { color: theme.tokens.text.primary }]}>⇥</Text>
          </Pressable>
          <Pressable
            onPress={() => press('C-c')}
            disabled={busy}
            style={[styles.key, { borderColor: theme.tokens.border.default, opacity: busy ? 0.5 : 1 }]}
            accessibilityRole="button"
            accessibilityLabel="Ctrl-C"
          >
            <Text style={[styles.keyText, { color: theme.tokens.text.primary }]}>Ctrl-C</Text>
          </Pressable>
        </View>
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: {
    flex: 1,
    backgroundColor: theme.tokens.bg.base,
  },
  loadMore: {
    marginHorizontal: theme.base.space[2],
    marginTop: theme.base.space[2],
    paddingVertical: theme.base.space[2],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
    alignItems: 'center',
    justifyContent: 'center',
    minHeight: 44,
  },
  loadMoreText: {
    fontSize: theme.base.text.xs,
  },
  scroll: {
    flex: 1,
  },
  scrollContent: {
    padding: theme.base.space[3],
    paddingBottom: theme.base.space[2],
  },
  mono: {
    fontSize: 11,
    lineHeight: 14,
  },
  err: {
    fontSize: theme.base.text.xs,
    textAlign: 'center',
    paddingVertical: theme.base.space[1],
    paddingHorizontal: theme.base.space[2],
  },
  pending: {
    marginHorizontal: theme.base.space[2],
    marginVertical: theme.base.space[1],
    paddingVertical: theme.base.space[2],
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
    alignItems: 'center',
  },
  pendingText: {
    fontSize: theme.base.text.xs,
    textAlign: 'center',
  },
  compose: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[2],
    borderTopWidth: 1,
  },
  input: {
    flex: 1,
    minHeight: 44,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
    fontSize: 13,
  },
  sendBtn: {
    minHeight: 44,
    minWidth: 44,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.sm,
    alignItems: 'center',
    justifyContent: 'center',
  },
  sendPrimary: {},
  sendText: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
  keysBar: {
    flexDirection: 'column',
    gap: theme.base.space[1],
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[2],
    borderTopWidth: 1,
  },
  keysHint: {
    fontSize: theme.base.text.xxs,
    fontWeight: '600',
    textTransform: 'uppercase',
  },
  keysRow: {
    flexDirection: 'row',
    gap: theme.base.space[2],
    flexWrap: 'wrap',
  },
  key: {
    minHeight: 44,
    minWidth: 44,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: theme.tokens.bg.elevated,
  },
  keyEnter: {
    backgroundColor: theme.tokens.accent.dim,
  },
  keyText: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
}));
