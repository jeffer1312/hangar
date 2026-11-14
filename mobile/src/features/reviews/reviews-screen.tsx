import { useCallback, useMemo, useRef, useState } from 'react';
import { FlatList, Platform, ScrollView, Text, TextInput, View } from 'react-native';
import { Stack, useIsFocused, useRouter } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import { useUnistyles } from 'react-native-unistyles';
import { mergeReviewLocations, readReviewSources, reviewIdentity, reviewNeedsAttention, type ReviewSource, type ReviewSummary } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { Screen } from '../../ui/Screen';
import { HeaderBack, useGlassHeader } from '../../ui/glassHeader';
import { Sheet } from '../../ui/Sheet';
import { ReviewButton, ReviewNotice, ReviewRow, ReviewSection, styles } from './review-ui';
import { useReviewResource } from './use-review-resource';
import * as m from '../../paraglide/messages';

type Row = { serverId: string; machine: string; review: ReviewSummary };
type Filter = 'all' | 'attention' | 'requested';

export function ReviewsScreen() {
  const router = useRouter();
  const { theme } = useUnistyles();
  const focused = useIsFocused();
  const header = useGlassHeader();
  const headerHeight = useHeaderHeight();
  const servers = useServers((state) => state.servers);
  const own = servers.filter((server) => !server.invite && !server.ephemeral && !server.removed && !server.disabled);
  const ownRef = useRef(own); ownRef.current = own;
  const key = JSON.stringify(own.map((server) => [server.id, server.baseUrl, server.token]));
  const cache = useRef<ReviewSource[]>([]);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const [access, setAccess] = useState(false);
  const resource = useReviewResource<ReviewSource[]>(key, (signal, publish) => readReviewSources(ownRef.current, { signal, previous: cache.current,
    onUpdate: (sources) => { if (!signal.aborted) { cache.current = sources; publish(sources); } },
  }), focused);
  const buckets = resource.data ?? [];
  const all = useMemo(() => mergeReviewLocations(buckets.flatMap((bucket): Row[] => bucket.listing?.reviews.map((review) => ({ serverId: bucket.server.id, machine: bucket.server.label, review })) ?? [])), [buckets]);
  const attention = all.filter((row) => reviewNeedsAttention(row.review)).length;
  const rows = useMemo(() => {
    const term = query.trim().toLocaleLowerCase();
    return all.filter(({ review }) => (filter === 'all' || filter === 'attention' && reviewNeedsAttention(review) || filter === 'requested' && review.review_requested)
      && `${review.title} ${review.repo} ${review.source} ${review.author}`.toLocaleLowerCase().includes(term))
      .sort((a, b) => Number(reviewNeedsAttention(b.review)) - Number(reviewNeedsAttention(a.review)) || a.review.repo.localeCompare(b.review.repo) || b.review.number - a.review.number);
  }, [all, query, filter]);
  const renderItem = useCallback(({ item }: { item: Row }) => <ReviewRow review={item.review} serverId={item.serverId} machine={item.machine} />, []);
  const headerOptions = { ...header, title: m.native_review_title(),
    headerLeft: router.canGoBack() ? undefined : () => <HeaderBack onPress={() => router.replace('/')} />,
    headerRight: () => <View style={styles.row}>
      <ReviewButton label={m.native_review_hosts_title()} icon="Settings2" iconOnly onPress={() => setAccess(true)} />
      <ReviewButton label={m.native_review_refresh()} icon="RefreshCw" iconOnly onPress={() => { void resource.refresh(); }} />
    </View>,
  };
  const errors = buckets.filter((bucket) => bucket.error);
  const unavailable = errors.length > 0 && all.length === 0 && buckets.every((bucket) => !!bucket.error || !bucket.listing);
  const inaccessible = buckets.length > 0 && buckets.every((bucket) => !bucket.listing || !bucket.listing.reviews.length && bucket.listing.hosts.length > 0 && bucket.listing.hosts.every((host) => host.state !== 'ok'));

  return <Screen edges={['bottom', 'left', 'right']}>
    <Stack.Screen options={headerOptions} />
    <FlatList data={rows} keyExtractor={(row) => JSON.stringify([row.serverId, reviewIdentity(row.review)])} renderItem={renderItem}
      style={styles.page} contentContainerStyle={[styles.content, Platform.OS !== 'ios' ? { paddingTop: headerHeight } : null]}
      contentInsetAdjustmentBehavior="automatic" keyboardShouldPersistTaps="handled" refreshing={resource.loading && all.length > 0}
      onRefresh={() => { void resource.refresh(); }}
      ListHeaderComponent={<View style={styles.section}>
        <TextInput value={query} onChangeText={setQuery} placeholder={m.native_review_search()} placeholderTextColor={theme.tokens.text.muted} accessibilityLabel={m.native_review_search()} style={styles.search} autoCapitalize="none" clearButtonMode="while-editing" />
        <ScrollView horizontal showsHorizontalScrollIndicator={false}>
          <View style={styles.row}>
            <ReviewButton label={m.native_review_filter_all()} onPress={() => setFilter('all')} primary={filter === 'all'} selected={filter === 'all'} />
            <ReviewButton label={`${m.native_review_filter_attention()}${attention ? ` (${attention})` : ''}`} onPress={() => setFilter('attention')} primary={filter === 'attention'} selected={filter === 'attention'} />
            <ReviewButton label={m.native_review_filter_requested()} onPress={() => setFilter('requested')} primary={filter === 'requested'} selected={filter === 'requested'} />
          </View>
        </ScrollView>
      </View>}
      ListEmptyComponent={resource.loading ? <ReviewNotice text={m.native_loading()} loading />
        : resource.error || unavailable ? <ReviewNotice text={resource.error || errors.map((bucket) => `${bucket.server.label}: ${bucket.error}`).join('\n')} retry={() => { void resource.refresh(); }} />
          : <ReviewNotice text={query || filter !== 'all' ? m.native_review_list_filtered() : inaccessible ? m.native_review_list_unavailable() : m.native_review_list_empty()} />} />
    {access ? <Sheet open onDismiss={() => setAccess(false)} sizes={['large']} scrollable>
      <ScrollView contentContainerStyle={styles.content}>
        <Text style={styles.title}>{m.native_review_hosts_title()}</Text>
        <Text style={styles.muted}>{m.native_review_hosts_hint()}</Text>
        {buckets.map((bucket) => <ReviewSection key={bucket.server.id} title={bucket.server.label}>
          {bucket.error ? <Text style={styles.error}>{bucket.error}</Text> : null}
          <View style={styles.group}>{bucket.listing?.hosts.map((host) => <View key={host.host} style={styles.hostRow}>
            <Text style={styles.heading}>{host.host}</Text>
            <Text style={styles.meta}>{[host.user, host.cli, m.native_review_host_repos({ n: String(host.repos) })].filter(Boolean).join(' · ')}</Text>
            {host.state !== 'ok' ? <Text style={styles.muted}>{host.error || (host.state === 'no_login' ? m.native_review_host_no_login({ comando: `${host.cli || 'gh'} auth login --hostname ${host.host}` })
              : host.state === 'cli_missing' ? m.native_review_host_cli_missing({ cli: host.cli || 'gh / glab' }) : m.native_review_host_unknown())}</Text> : null}
          </View>)}</View>
          {!bucket.listing?.hosts.length && !bucket.error ? <Text style={styles.muted}>{m.native_review_hosts_empty()}</Text> : null}
        </ReviewSection>)}
        {!buckets.length ? <Text style={styles.muted}>{m.native_review_hosts_empty()}</Text> : null}
        <ReviewButton label={m.native_review_refresh()} icon="RefreshCw" onPress={() => { void resource.refresh(); }} />
      </ScrollView>
    </Sheet> : null}
  </Screen>;
}
