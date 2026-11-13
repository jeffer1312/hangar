import { memo, useCallback, useMemo, useState } from 'react';
import { View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { fold, type NewsHistoryItem, type UpdateNewsKind } from '@hangar/core';
import { Pagina } from '../../../src/features/config/Pagina';
import { PageHeader } from '../../../src/features/config/PageHeader';
import { SectionCard } from '../../../src/features/config/SectionCard';
import { SectionTabs } from '../../../src/features/config/SectionTabs';
import { SettingsMenuGroup } from '../../../src/features/config/SettingsMenu';
import { SettingsRow } from '../../../src/features/config/SettingsRow';
import { useSettingsColors } from '../../../src/features/config/colors';
import { useSmallMarkdown } from '../../../src/features/updates/WhatsNewSheet';
import { animationFor } from '../../../src/features/updates/animations';
import { dayLabel, forPhone, useNewsHistory, useUnseenNews } from '../../../src/features/updates/docs';
import { DocStatus, EmptyText, KindPill, SearchField, ServerTag, UnseenDot, kindLabel } from '../../../src/features/updates/DocParts';
import { useServers } from '../../../src/stores/servers';
import { haptic } from '../../../src/ui/haptics';
import * as m from '../../../src/paraglide/messages';

type Filter = 'all' | UpdateNewsKind;
type Day = { date: string; items: NewsHistoryItem[] };

/** A linha abre a página quando há o que mostrar além do resumo: mais texto ou a animação que o app tem. */
const newsOpens = (item: NewsHistoryItem) => item.more || !!animationFor(item.animation);

export default function Updates() {
  const router = useRouter();
  const server = useServers((s) => s.active());
  const [state, retry] = useNewsHistory(server, true);
  const unseen = useUnseenNews(server);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const mine = useMemo(() => (state.status === 'ok' ? state.data.news.filter((n) => forPhone(n.platforms)) : []), [state]);
  // Já vem do mais novo para o mais antigo: agrupar é juntar os vizinhos do mesmo dia.
  const days = useMemo(() => {
    const q = fold(query.trim());
    const out: Day[] = [];
    for (const n of mine) {
      if (filter !== 'all' && n.kind !== filter) continue;
      if (q && !fold(`${n.title}\n${n.text}`).includes(q)) continue;
      const last = out[out.length - 1];
      if (last?.date === n.date) last.items.push(n);
      else out.push({ date: n.date, items: [n] });
    }
    return out;
  }, [mine, filter, query]);
  const open = useCallback((id: string) => router.push({ pathname: '/config/atualizacoes/[id]', params: { id } } as never), [router]);
  const renderItem = useCallback(({ item }: { item: Day }) => <DayCard day={item} unseen={unseen} onOpen={open} />, [unseen, open]);

  return (
    <Pagina list={{ data: days, renderItem, keyExtractor: (d) => d.date }}>
      <PageHeader title={m.updates_history_title()} subtitle={m.updates_history_lead()} />
      <ServerTag server={server} />
      <SearchField value={query} onChange={setQuery} placeholder={m.updates_history_search()} />
      <SectionTabs<Filter>
        current={filter}
        onPick={(k) => { haptic.selection(); setFilter(k); }}
        tabs={[
          { key: 'all', label: m.updates_history_filter_all() },
          { key: 'new', label: m.updates_history_filter_new() },
          { key: 'improvement', label: m.updates_history_filter_improvements() },
          { key: 'fix', label: m.updates_history_filter_fixes() },
        ]}
      />
      {state.status !== 'ok' ? <DocStatus state={state} onRetry={retry} />
        : days.length === 0 ? <EmptyText text={mine.length ? m.updates_history_none_match() : m.updates_history_empty()} />
        : null}
    </Pagina>
  );
}

const DayCard = memo(function DayCard({ day, unseen, onOpen }: { day: Day; unseen: ReadonlySet<string>; onOpen: (id: string) => void }) {
  return (
    <View style={styles.day}>
      <SettingsMenuGroup title={dayLabel(day.date)} />
      <SectionCard>
        {day.items.map((n) => <NewsRow key={n.id} item={n} unseen={unseen.has(n.id)} onOpen={onOpen} />)}
      </SectionCard>
    </View>
  );
});

const NewsRow = memo(function NewsRow({ item, unseen, onOpen }: { item: NewsHistoryItem; unseen: boolean; onOpen: (id: string) => void }) {
  const c = useSettingsColors();
  const md = useSmallMarkdown(c.muted);
  const opens = newsOpens(item);
  const label = [item.title, kindLabel(item.kind), unseen ? m.updates_history_unseen() : ''].filter(Boolean).join(', ');
  return (
    <SettingsRow
      title={item.title}
      titleBefore={unseen ? <UnseenDot /> : null}
      titleAfter={<KindPill kind={item.kind} />}
      accessibilityLabel={label}
      onPress={opens ? () => onOpen(item.id) : undefined}
    >
      {/* Sem página própria, o resumo inteiro fica na linha. */}
      {!opens && item.summary ? <EnrichedMarkdownText markdown={item.summary} markdownStyle={md} flavor="github" /> : null}
    </SettingsRow>
  );
});

const styles = StyleSheet.create((theme) => ({
  day: { paddingBottom: theme.base.space[2] },
}));
