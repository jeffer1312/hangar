import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, FlatList, Keyboard, Platform, Pressable, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Stack, useIsFocused, useLocalSearchParams, useRouter } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import type { Edge } from 'react-native-safe-area-context';
import { askHistoryForServer, getSearchContextForServer, searchTranscriptsForServer, type ChatEvent, type SearchHit, type Server } from '@hangar/core';
import { useServers } from '../src/stores/servers';
import { Screen } from '../src/ui/Screen';
import { HeaderBack, useGlassHeader } from '../src/ui/glassHeader';
import { Icon } from '../src/ui/Icon';
import { AssistantBubble } from '../src/chat/AssistantBubble';
import { superficie } from '../src/theme/superficie';
import * as m from '../src/paraglide/messages';

const EDGES: Edge[] = ['bottom', 'left', 'right'];
const IOS = Platform.OS === 'ios';

type Hit = { server: Server; hit: SearchHit };
type Bucket = { server: Server; hits: SearchHit[]; loading: boolean; error: string };
const message = (e: unknown) => e instanceof Error ? e.message : m.erro_desconhecido();
const searchError = (cause: unknown) => {
  if (cause instanceof Error) {
    if (cause.name === 'TimeoutError') return m.busca_motivo_timeout();
    if (/^\d{3}$/.test(cause.message)) return m.busca_motivo_http({ status: cause.message });
    if (cause.name === 'AbortError' || /\b(?:cancell?ed|aborted)\b/i.test(cause.message)) return m.busca_motivo_cancelamento();
  }
  return m.falha_conexao();
};
const hitKey = ({ server, hit }: Hit) => JSON.stringify([server.id, hit.project, hit.session_id, hit.event_id || hit.line]);

function Highlight({ text, query }: { text: string; query: string }) {
  const words = [...new Set(query.trim().split(/\s+/).filter(Boolean))].sort((a, b) => b.length - a.length);
  const pattern = words.length ? new RegExp(`(${words.map((word) => word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})`, 'gi') : null;
  return <Text style={styles.snippet}>{pattern ? text.split(pattern).map((part, i) =>
    <Text key={i} style={i % 2 ? styles.highlight : undefined}>{part}</Text>) : text}</Text>;
}

function Result({ item, query }: { item: Hit; query: string }) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const { server, hit } = item;
  const [expanded, setExpanded] = useState(false);
  const [context, setContext] = useState<ChatEvent[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const generation = useRef(0);
  useEffect(() => () => { generation.current++; }, []);
  const loadContext = () => {
    if (!hit.event_id) { setError(m.busca_contexto_indisponivel()); return; }
    const g = ++generation.current;
    setLoading(true); setError('');
    void getSearchContextForServer(server, hit.project, hit.session_id, hit.event_id, hit.provider, hit.codex_account)
      .then((events) => { if (g === generation.current) setContext(events); })
      .catch((e: unknown) => { if (g === generation.current) setError(`${m.busca_contexto_indisponivel()}: ${message(e)}`); })
      .finally(() => { if (g === generation.current) setLoading(false); });
  };
  const toggle = () => {
    if (expanded) { setExpanded(false); return; }
    setExpanded(true);
    if (!context && !loading) loadContext();
  };
  return <View style={styles.result}>
    <Text style={styles.meta}>{[server.label, hit.session_name || hit.session_id.slice(0, 8),
      hit.live ? m.switcher_ativa() : m.switcher_arquivo(), hit.role === 'user' ? m.busca_voce() : hit.role === 'assistant' ? m.busca_assistente() : null].filter(Boolean).join(' · ')}</Text>
    <Pressable onPress={toggle} style={styles.snippetButton} accessibilityRole="button" accessibilityState={{ expanded }}>
      <Highlight text={hit.line} query={query} />
    </Pressable>
    {expanded ? <View style={styles.context}>
      {loading ? <Text style={styles.meta}>{m.busca_carregando_contexto()}</Text> : null}
      {error ? <View><Text accessibilityRole="alert" style={styles.error}>{error}</Text>
        <Pressable onPress={loadContext} accessibilityRole="button" style={styles.retry} hitSlop={4}>
          <Text style={styles.action}>{m.busca_tentar_de_novo()}</Text></Pressable></View> : null}
      {context?.map((event) => <View key={event.id} style={event.id === hit.event_id ? styles.active : undefined}>
        <Text style={styles.meta}>{event.kind === 'user_msg' ? m.busca_voce() : m.busca_assistente()}</Text>
        {event.kind === 'assistant_msg' ? <AssistantBubble text={event.text ?? ''} serverId={server.id} ts={event.ts} />
          : <Highlight text={event.text ?? ''} query={query} />}
      </View>)}
      {context?.length === 0 ? <Text style={styles.meta}>{m.busca_contexto_indisponivel()}</Text> : null}
    </View> : null}
    {hit.event_id ? <Pressable style={styles.open} hitSlop={4} accessibilityRole="button" accessibilityLabel={m.busca_abrir_ponto()}
      onPress={() => router.push({ pathname: '/search-point', params: { server: server.id, project: hit.project, sid: hit.session_id,
        eventId: hit.event_id!, name: hit.live ? hit.session_name ?? '' : '', conversationTitle: hit.session_name || hit.session_id.slice(0, 8),
        ...(hit.provider ? { provider: hit.provider } : {}), ...(hit.codex_account ? { codexAccount: hit.codex_account } : {}) } } as never)}>
      <Icon name="ArrowUpRight" size={16} color={theme.tokens.accent.base} />
      <Text style={styles.action}>{m.busca_abrir_ponto()}</Text>
    </Pressable> : <Text style={styles.meta}>{m.busca_contexto_indisponivel()}</Text>}
  </View>;
}

export default function SearchScreen() {
  const { theme } = useUnistyles();
  const router = useRouter();
  const focused = useIsFocused();
  const glassHeader = useGlassHeader();
  const headerHeight = useHeaderHeight();
  const canGoBack = router.canGoBack();
  const title = m.switcher_buscar_conversas();
  const headerOptions = useMemo(() => ({ ...glassHeader, title,
    headerLeft: canGoBack ? undefined : () => <HeaderBack onPress={() => router.replace('/')} />,
  }), [glassHeader, title, canGoBack, router]);
  const servers = useServers((s) => s.servers);
  const activeId = useServers((s) => s.activeId);
  const params = useLocalSearchParams<{ q?: string }>();
  const [query, setQuery] = useState(() => typeof params.q === 'string' ? params.q : '');
  const searchTerm = query.trim();
  const own = servers.filter((server) => !server.invite && !server.removed && !server.disabled);
  const serverKey = JSON.stringify(own.map((server) => [server.id, server.baseUrl, server.token]));
  const key = JSON.stringify([searchTerm, serverKey]);
  const scope = useRef({ key, focused });
  if (scope.current.key !== key || scope.current.focused !== focused) scope.current = { key, focused };
  const token = scope.current;
  const attemptRef = useRef<{ scope: typeof token; alive: boolean } | null>(null);
  const current = (attempt: NonNullable<typeof attemptRef.current>) => scope.current === attempt.scope && attempt.alive && attempt.scope.focused;
  const input = useRef<TextInput>(null);
  const ownRef = useRef(own); ownRef.current = own;
  const [state, setState] = useState<{ key: string; buckets: Bucket[] }>({ key: '', buckets: [] });
  const buckets = state.key === key ? state.buckets : [];
  const askKey = JSON.stringify([key, activeId]);
  const askScope = useRef({ key: askKey });
  if (askScope.current.key !== askKey) askScope.current = { key: askKey };
  const askToken = askScope.current;
  const [asked, setAsked] = useState<{ scope: typeof askToken; serverId: string; loading: boolean; answer: string; hits: SearchHit[]; error: string } | null>(null);
  const askGeneration = useRef(0);
  const ask = asked?.scope === askToken ? asked : null;
  const active = own.find((server) => server.id === activeId);
  const searchMachine = (server: Server, attempt = attemptRef.current) => {
    if (!attempt || !current(attempt)) return;
    setState((before) => ({ key, buckets: before.key !== key ? [] : before.buckets.map((b) => b.server.id === server.id ? { ...b, loading: true, error: '' } : b) }));
    void searchTranscriptsForServer(server, searchTerm)
      .then((hits) => {
        if (!current(attempt)) return;
        setState((before) => ({ key, buckets: before.buckets.map((b) => b.server.id === server.id ? { server, hits, loading: false, error: '' } : b) }));
      })
      .catch((e: unknown) => {
        if (!current(attempt)) return;
        setState((before) => ({ key, buckets: before.buckets.map((b) => b.server.id === server.id ? { ...b, loading: false, error: searchError(e) } : b) }));
      });
  };
  useEffect(() => {
    const attempt = { scope: token, alive: true };
    attemptRef.current = attempt;
    if (!focused) {
      input.current?.blur(); Keyboard.dismiss();
      setAsked((before) => before?.loading ? null : before);
    }
    if (!focused || !searchTerm) return () => { attempt.alive = false; };
    const timer = setTimeout(() => {
      if (!current(attempt)) return;
      const targets = state.key === key
        ? ownRef.current.filter((server) => state.buckets.some((bucket) => bucket.server.id === server.id && bucket.loading))
        : ownRef.current;
      setState((before) => before.key === key ? before : { key, buckets: targets.map((server) => ({ server, hits: [], loading: true, error: '' })) });
      for (const server of targets) searchMachine(server, attempt);
    }, 250);
    return () => { attempt.alive = false; clearTimeout(timer); };
    // A identidade da máquina importa; a publicação SSE da lista não dispara busca.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token]);
  const askHistory = () => {
    const attempt = attemptRef.current;
    if (!active || !searchTerm || ask?.loading || !attempt || !current(attempt)) return;
    const id = ++askGeneration.current;
    const server = active;
    const valid = () => current(attempt) && id === askGeneration.current && askScope.current === askToken;
    setAsked({ scope: askToken, serverId: server.id, loading: true, answer: '', hits: [], error: '' });
    void askHistoryForServer(server, searchTerm)
      .then((result) => { if (valid()) setAsked({ scope: askToken, serverId: server.id, loading: false, ...result, error: '' }); })
      .catch((e: unknown) => { if (valid()) setAsked({ scope: askToken, serverId: server.id, loading: false, answer: '', hits: [], error: message(e) }); });
  };
  const hits = useMemo(() => {
    const all = buckets.flatMap((bucket) => bucket.hits.map((hit) => ({ server: bucket.server, hit })));
    const source = own.find((server) => server.id === ask?.serverId);
    if (source) all.push(...(ask?.hits ?? []).map((hit) => ({ server: source, hit })));
    return [...new Map(all.map((hit) => [hitKey(hit), hit])).values()]
      .sort((a, b) => (b.hit.ts ?? b.hit.mtime) - (a.hit.ts ?? a.hit.mtime) || hitKey(a).localeCompare(hitKey(b)));
  }, [buckets, ask, serverKey]);
  const renderItem = useCallback(({ item }: { item: Hit }) => <Result item={item} query={searchTerm} />, [searchTerm]);
  const searching = buckets.some((bucket) => bucket.loading) || !!searchTerm && state.key !== key;
  const failedBuckets = buckets.filter((bucket) => bucket.error);
  const header = <View style={styles.status}>
    {searchTerm && active ? <Pressable onPress={askHistory} disabled={ask?.loading} hitSlop={4} accessibilityRole="button"
      accessibilityLabel={ask?.loading ? m.switcher_perguntando() : m.mobile_busca_perguntar()} style={styles.ask}>
      <Icon name="Sparkles" size={18} color={theme.tokens.accent.base} />
      <Text style={styles.action}>{ask?.loading ? m.switcher_perguntando() : m.mobile_busca_perguntar()}</Text>
    </Pressable> : null}
    {ask?.error ? <Text accessibilityRole="alert" style={styles.error}>{ask.error}</Text> : null}
    {ask?.answer ? <AssistantBubble text={ask.answer} serverId={ask.serverId} /> : null}
    {searching ? <View style={styles.loading}><ActivityIndicator /><Text style={styles.meta}>{m.switcher_buscando()}</Text></View> : null}
    {failedBuckets.map((bucket) => <View key={bucket.server.id}>
      <Text accessibilityRole="alert" style={styles.error}>{m.busca_servidor_falhou_motivo({ servidor: bucket.server.label, motivo: bucket.error })}</Text>
      <Pressable onPress={() => searchMachine(bucket.server)} accessibilityRole="button" style={styles.retry} hitSlop={4}>
        <Text style={styles.action}>{m.busca_tentar_de_novo()}</Text>
      </Pressable>
    </View>)}
    {!own.length ? <Text style={styles.meta}>{m.busca_sem_servidores()}</Text> : null}
    {!searchTerm ? <Text style={styles.meta}>{m.busca_digite_todas()}</Text> : null}
    {searchTerm && !searching && !hits.length && failedBuckets.length < buckets.length
      ? <Text style={styles.meta}>{failedBuckets.length ? m.mobile_busca_vazio_parcial() : m.busca_nenhum_resultado()}</Text> : null}
  </View>;
  return <Screen edges={EDGES}>
    <Stack.Screen options={headerOptions} />
    <View style={[styles.field, { backgroundColor: superficie(theme, 0.6), marginTop: headerHeight }]}>
      <Icon name="Search" size={18} color={theme.tokens.text.muted} />
      <TextInput ref={input} value={query} onChangeText={setQuery} placeholder={m.switcher_buscar_conversas()}
        accessibilityLabel={m.switcher_buscar_conversas()} placeholderTextColor={theme.tokens.text.muted}
        autoCapitalize="none" autoCorrect={false} returnKeyType="search" autoFocus style={styles.input} />
      {query ? <Pressable onPress={() => setQuery('')} style={styles.icon} accessibilityRole="button" accessibilityLabel={m.lista_filtro_limpar()}>
        <Icon name="X" size={18} color={theme.tokens.text.muted} />
      </Pressable> : null}
    </View>
    <FlatList data={hits} keyExtractor={(item) => `${key}:${hitKey(item)}`} renderItem={renderItem}
      ListHeaderComponent={header} contentContainerStyle={styles.list} contentInsetAdjustmentBehavior={IOS ? 'automatic' : undefined}
      keyboardShouldPersistTaps="handled" keyboardDismissMode="on-drag" />
  </Screen>;
}

const styles = StyleSheet.create((theme) => ({
  icon: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  field: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingLeft: theme.base.space[3], marginHorizontal: theme.base.space[3], borderRadius: theme.base.radius.lg },
  input: { flex: 1, minHeight: 44, paddingVertical: theme.base.space[2], fontSize: theme.base.text.base, color: theme.tokens.text.primary },
  list: { padding: theme.base.space[4], gap: theme.base.space[4] },
  status: { gap: theme.base.space[3] },
  loading: { flexDirection: 'row', gap: theme.base.space[2], alignItems: 'center' },
  meta: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  snippet: { fontSize: theme.base.text.base, color: theme.tokens.text.primary },
  snippetButton: { minHeight: 44, minWidth: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[2] },
  highlight: { backgroundColor: theme.tokens.status.warning, color: theme.tokens.bg.base },
  result: { gap: theme.base.space[2], paddingVertical: theme.base.space[3], borderBottomWidth: StyleSheet.hairlineWidth, borderBottomColor: theme.tokens.border.subtle },
  context: { gap: theme.base.space[3], paddingLeft: theme.base.space[3], borderLeftWidth: 1, borderLeftColor: theme.tokens.border.default },
  active: { borderLeftWidth: 2, borderLeftColor: theme.tokens.accent.base, paddingLeft: theme.base.space[2] },
  error: { fontSize: theme.base.text.sm, color: theme.tokens.status.error },
  open: { alignSelf: 'flex-start', flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1], minHeight: 44, minWidth: 44, paddingHorizontal: theme.base.space[2] },
  action: { fontSize: theme.base.text.sm, color: theme.tokens.accent.base },
  ask: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 44, minWidth: 44, paddingHorizontal: theme.base.space[2] },
  retry: { minHeight: 44, minWidth: 44, paddingHorizontal: theme.base.space[2], justifyContent: 'center', alignSelf: 'flex-start' },
}));
