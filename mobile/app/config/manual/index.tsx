import { useMemo, useState } from 'react';
import { View } from 'react-native';
import { useRouter } from 'expo-router';
import { fold, type ManualArticle, type ManualCategoryId } from '@hangar/core';
import { Pagina } from '../../../src/features/config/Pagina';
import { PageHeader } from '../../../src/features/config/PageHeader';
import { SectionCard } from '../../../src/features/config/SectionCard';
import { SectionTabs } from '../../../src/features/config/SectionTabs';
import { SettingsMenuGroup } from '../../../src/features/config/SettingsMenu';
import { SettingsRow } from '../../../src/features/config/SettingsRow';
import { forPhone, shortDay, useManual, useNewsHistory } from '../../../src/features/updates/docs';
import { DocStatus, EmptyText, SearchField, ServerTag, articleIcon, categoryLabel } from '../../../src/features/updates/DocParts';
import { useServers } from '../../../src/stores/servers';
import { haptic } from '../../../src/ui/haptics';
import * as m from '../../../src/paraglide/messages';

const CATEGORIES: ManualCategoryId[] = ['comecar', 'sessoes', 'ferramentas', 'problemas'];
// A busca olha o texto inteiro do artigo, não só a lista.
const searchable = (a: ManualArticle) => fold([a.title, a.summary, a.intro, ...a.sections.flatMap((s) => [s.title, s.markdown])].join('\n'));

export default function ManualScreen() {
  const router = useRouter();
  const server = useServers((s) => s.active());
  const [state, retry] = useManual(server, true);
  const [news] = useNewsHistory(server, false);
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState<ManualCategoryId>('comecar');
  const articles = useMemo(() => (state.status === 'ok'
    ? state.data.categories.flatMap((c) => c.articles.filter((a) => forPhone(a.platforms)).map((a) => ({ category: c.id, article: a, text: searchable(a) })))
    : []), [state]);
  const q = fold(query.trim());
  const shown = q ? articles.filter((a) => a.text.includes(q)) : articles.filter((a) => a.category === category);
  // O histórico é extra: sem ele, o bloco some e o manual continua.
  const latest = news.status === 'ok' ? news.data.news.filter((n) => n.kind === 'new' && forPhone(n.platforms)).slice(0, 3) : [];

  return (
    <Pagina>
      <PageHeader title={m.manual_title()} subtitle={m.manual_lead()} />
      <ServerTag server={server} />
      <SearchField value={query} onChange={setQuery} placeholder={m.manual_search()} />
      {q ? null : (
        <SectionTabs<ManualCategoryId>
          current={category}
          onPick={(k) => { haptic.selection(); setCategory(k); }}
          tabs={CATEGORIES.map((key) => ({ key, label: categoryLabel(key) }))}
        />
      )}
      {state.status !== 'ok' ? <DocStatus state={state} onRetry={retry} />
        : !articles.length ? <EmptyText text={m.manual_empty()} />
        : !shown.length ? <EmptyText text={q ? m.manual_none_match() : m.manual_category_empty()} />
        : (
          <SectionCard>
            {shown.map(({ article: a }) => (
              <SettingsRow key={a.id} icon={articleIcon(a.icon)} title={a.title} description={a.summary || undefined}
                onPress={() => router.push({ pathname: '/config/manual/[id]', params: { id: a.id } } as never)} />
            ))}
          </SectionCard>
        )}
      {latest.length ? (
        <View>
          <SettingsMenuGroup title={m.manual_latest()} />
          <SectionCard>
            {latest.map((n) => (
              <SettingsRow key={n.id} title={n.title} description={shortDay(n.date)}
                onPress={() => router.push({ pathname: '/config/atualizacoes/[id]', params: { id: n.id } } as never)} />
            ))}
          </SectionCard>
        </View>
      ) : null}
    </Pagina>
  );
}
