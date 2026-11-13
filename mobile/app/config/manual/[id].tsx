import { useMemo, useRef, useState } from 'react';
import { Platform, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { mkMarkdownStyle } from '../../../src/chat/AssistantBubble';
import { Pagina } from '../../../src/features/config/Pagina';
import { PageHeader } from '../../../src/features/config/PageHeader';
import { SectionCard } from '../../../src/features/config/SectionCard';
import { SettingsMenuGroup } from '../../../src/features/config/SettingsMenu';
import { SettingsRow } from '../../../src/features/config/SettingsRow';
import { useSettingsColors } from '../../../src/features/config/colors';
import { forPhone, shortDay, useManual } from '../../../src/features/updates/docs';
import { DocStatus, EmptyText, categoryLabel } from '../../../src/features/updates/DocParts';
import { Icon } from '../../../src/ui/Icon';
import { useServers } from '../../../src/stores/servers';
import * as m from '../../../src/paraglide/messages';

const IOS = Platform.OS === 'ios';

export default function ArticlePage() {
  const { id } = useLocalSearchParams<{ id: string }>();
  const router = useRouter();
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const header = useHeaderHeight();
  const server = useServers((s) => s.active());
  const [state, retry] = useManual(server, false);
  const md = useMemo(() => mkMarkdownStyle(theme), [theme]);
  const scrollRef = useRef<ScrollView>(null);
  const offsets = useRef(new Map<string, number>());
  // A seção escolhida no índice: a rolagem espera o índice fechar e as seções subirem.
  const pending = useRef<string | null>(null);
  const [tocOpen, setTocOpen] = useState(false);
  const found = state.status === 'ok'
    ? state.data.categories.flatMap((cat) => cat.articles.map((article) => ({ category: cat.id, article }))).find((a) => a.article.id === id)
    : undefined;
  const back = <Stack.Screen options={{ headerBackTitle: m.manual_title() }} />;

  if (state.status !== 'ok') return <Pagina>{back}<DocStatus state={state} onRetry={retry} /></Pagina>;
  if (!found) return <Pagina>{back}<EmptyText text={m.manual_not_found()} /></Pagina>;
  const { article, category } = found;
  const news = article.news.filter((n) => forPhone(n.platforms));

  const scrollTo = (sectionId: string) => {
    const y = offsets.current.get(sectionId);
    // No iOS o conteúdo começa sob o cabeçalho transparente; no Android a rolagem já começa abaixo dele.
    if (y !== undefined) scrollRef.current?.scrollTo({ y: Math.max(0, y - (IOS ? header : 0) - theme.base.space[2]) });
  };
  const pick = (sectionId: string) => {
    pending.current = sectionId;
    setTocOpen(false);
  };

  return (
    <Pagina scrollRef={scrollRef}>
      {back}
      <View style={styles.top}>
        <Text style={[styles.category, { color: c.muted }]}>{categoryLabel(category)}</Text>
        <PageHeader title={article.title} subtitle={article.summary || undefined} />
      </View>
      {article.sections.length ? (
        <SectionCard>
          <SettingsRow
            title={m.manual_on_this_page()}
            onPress={() => setTocOpen((v) => !v)}
            right={<Icon name={tocOpen ? 'ChevronUp' : 'ChevronDown'} size={16} color={c.muted} />}
          />
          {tocOpen ? article.sections.map((s) => <SettingsRow key={s.id} title={s.title} onPress={() => pick(s.id)} />) : null}
        </SectionCard>
      ) : null}
      {article.intro ? <View style={styles.block}><EnrichedMarkdownText markdown={article.intro} markdownStyle={md} flavor="github" /></View> : null}
      {article.sections.map((s) => (
        <View
          key={s.id}
          style={styles.block}
          onLayout={(e) => {
            offsets.current.set(s.id, e.nativeEvent.layout.y);
            if (pending.current === s.id) { pending.current = null; scrollTo(s.id); }
          }}
        >
          <Text accessibilityRole="header" style={[styles.sectionTitle, { color: c.text }]}>{s.title}</Text>
          <EnrichedMarkdownText markdown={s.markdown} markdownStyle={md} flavor="github" />
        </View>
      ))}
      {news.length ? (
        <View>
          <SettingsMenuGroup title={m.manual_changed()} />
          <SectionCard>
            {news.map((n) => (
              <SettingsRow key={n.id} title={n.title} description={shortDay(n.date)}
                onPress={() => router.push({ pathname: '/config/atualizacoes/[id]', params: { id: n.id } } as never)} />
            ))}
          </SectionCard>
        </View>
      ) : null}
    </Pagina>
  );
}

const styles = StyleSheet.create((theme) => ({
  top: { gap: theme.base.space[1] },
  category: { paddingHorizontal: theme.base.space[1], fontSize: theme.base.text.xs },
  block: { gap: theme.base.space[1.5], paddingHorizontal: theme.base.space[1] },
  sectionTitle: { ...theme.type.heading },
}));
