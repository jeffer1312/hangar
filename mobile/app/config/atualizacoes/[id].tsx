import { useMemo } from 'react';
import { Text, View, useWindowDimensions } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { mkMarkdownStyle } from '../../../src/chat/AssistantBubble';
import { Pagina } from '../../../src/features/config/Pagina';
import { Pill } from '../../../src/features/config/PageHeader';
import { useSettingsColors } from '../../../src/features/config/colors';
import { animationFor } from '../../../src/features/updates/animations';
import { dayLabel, useNewsHistory } from '../../../src/features/updates/docs';
import { DocStatus, EmptyText, KindPill } from '../../../src/features/updates/DocParts';
import { useServers } from '../../../src/stores/servers';
import * as m from '../../../src/paraglide/messages';

export default function NewsPage() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const router = useRouter();
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const { width } = useWindowDimensions();
  const server = useServers((s) => s.active());
  const [state, retry] = useNewsHistory(server, false);
  const md = useMemo(() => mkMarkdownStyle(theme), [theme]);
  const item = state.status === 'ok' ? state.data.news.find((n) => n.id === id) : undefined;
  const Animation = animationFor(item?.animation ?? null);
  const back = <Stack.Screen options={{ headerBackTitle: m.updates_history_title() }} />;

  if (state.status !== 'ok') return <Pagina>{back}<DocStatus state={state} onRetry={retry} /></Pagina>;
  if (!item) return <Pagina>{back}<EmptyText text={m.updates_history_not_found()} /></Pagina>;
  return (
    <Pagina>
      {back}
      <View style={styles.top}>
        <Text selectable style={[styles.date, { color: c.muted }]}>{dayLabel(item.date, true)}</Text>
        <View style={styles.titleLine}>
          <Text selectable accessibilityRole="header" style={[styles.title, { color: c.text }]}>{item.title}</Text>
          <KindPill kind={item.kind} />
        </View>
      </View>
      {/* Largura da página menos o recuo dela dos dois lados. */}
      {Animation ? <Animation width={width - theme.base.space[3] * 2} /> : null}
      {item.text ? <View style={styles.body}><EnrichedMarkdownText markdown={item.text} markdownStyle={md} flavor="github" /></View> : null}
      {item.try_it ? <Text selectable style={[styles.tryIt, { color: c.muted }]}>{m.update_notes_try_it({ text: item.try_it })}</Text> : null}
      {item.manual ? (
        <View style={styles.actions}>
          <Pill label={m.updates_history_read_manual()} icon="BookOpen"
            onPress={() => router.push({ pathname: '/config/manual/[id]', params: { id: item.manual } } as never)} />
        </View>
      ) : null}
    </Pagina>
  );
}

const styles = StyleSheet.create((theme) => ({
  top: { gap: theme.base.space[1], paddingHorizontal: theme.base.space[1] },
  date: { fontSize: theme.base.text.xs },
  titleLine: { flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: theme.base.space[2] },
  title: { flexShrink: 1, fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold },
  body: { paddingHorizontal: theme.base.space[1] },
  tryIt: { paddingHorizontal: theme.base.space[1], fontSize: theme.base.text.sm },
  actions: { flexDirection: 'row' },
}));
