import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Platform, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { Stack, useIsFocused, useLocalSearchParams, useRouter } from 'expo-router';
import { useHeaderHeight } from 'expo-router/react-navigation';
import type { Edge } from 'react-native-safe-area-context';
import { getSearchContextForServer, probeServerResponse, rememberEntry, type ArchiveEntry, type ChatEvent, type Server } from '@hangar/core';
import { useServers } from '../src/stores/servers';
import { Screen } from '../src/ui/Screen';
import { HeaderBack, useGlassHeader } from '../src/ui/glassHeader';
import { AssistantBubble } from '../src/chat/AssistantBubble';
import { UserBubble } from '../src/chat/UserBubble';
import * as m from '../src/paraglide/messages';

const EDGES: Edge[] = ['bottom', 'left', 'right'];
const IOS = Platform.OS === 'ios';

const one = (value: string | string[] | undefined) => (Array.isArray(value) ? value[0] : value) ?? '';
const message = (error: unknown) => error instanceof Error ? error.message : m.erro_desconhecido();

function Context({ server, project, sid, eventId, name, conversationTitle, provider, codexAccount }: { server: Server; project: string; sid: string; eventId: string; name: string; conversationTitle: string; provider: string; codexAccount: string }) {
  const router = useRouter();
  const [events, setEvents] = useState<ChatEvent[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState('');
  const generation = useRef(0);
  const openGeneration = useRef(0);
  const serverRef = useRef(server);
  serverRef.current = server;
  const scroll = useRef<ScrollView>(null);
  const [targetY, setTargetY] = useState<number | null>(null);
  const [contentHeight, setContentHeight] = useState(0);
  const followingTarget = useRef(true);
  const load = useCallback(() => {
    const id = ++generation.current;
    setLoading(true); setError(''); setEvents([]); setTargetY(null); setContentHeight(0); followingTarget.current = true;
    void getSearchContextForServer(serverRef.current, project, sid, eventId, provider, codexAccount)
      .then((context) => {
        if (id !== generation.current) return;
        if (!context.some((event) => event.id === eventId)) throw new Error(m.busca_contexto_indisponivel());
        setEvents(context);
      })
      .catch((cause: unknown) => { if (id === generation.current) setError(message(cause)); })
      .finally(() => { if (id === generation.current) setLoading(false); });
  }, [project, sid, eventId, provider, codexAccount]);
  useEffect(() => { load(); return () => { generation.current++; openGeneration.current++; }; }, [load]);
  useEffect(() => {
    if (targetY === null || !contentHeight || !followingTarget.current || !scroll.current) return;
    scroll.current.scrollTo({ y: targetY, animated: false });
  }, [targetY, contentHeight]);
  const open = async () => {
    if (opening) return;
    const server = serverRef.current;
    if (name) { router.push(`/s/${encodeURIComponent(server.id)}/${encodeURIComponent(name)}` as never); return; }
    const id = ++openGeneration.current;
    setOpening(true); setOpenError('');
    try {
      const response = await probeServerResponse(server, `/api/archive/${encodeURIComponent(project)}`);
      if (!response.ok) throw new Error(`${m.arquivo_conversa_erro()}: ${m.busca_motivo_http({ status: String(response.status) })}`);
      const entries: ArchiveEntry[] = await response.json();
      if (!Array.isArray(entries)) throw new Error(m.arquivo_conversa_erro());
      const matches = entries.filter((entry) => entry?.session_id === sid && entry.project === project);
      if (matches.length > 1) throw new Error(m.arquivo_conversa_conta_ambigua());
      if (matches.length !== 1) throw new Error(m.arquivo_conversa_erro());
      if (id !== openGeneration.current) return;
      rememberEntry(server.id, matches[0]);
      router.push(`/archive/${encodeURIComponent(server.id)}/${encodeURIComponent(project)}/${encodeURIComponent(sid)}` as never);
    } catch (cause) {
      if (id === openGeneration.current) setOpenError(message(cause));
    } finally {
      if (id === openGeneration.current) setOpening(false);
    }
  };
  return <View style={styles.body}>
    <View style={styles.actions}>
      <View style={styles.identity}>
        <Text style={styles.title}>{conversationTitle || name || sid.slice(0, 8)}</Text>
        <Text style={styles.meta}>{server.label}</Text>
      </View>
      <Pressable onPress={open} disabled={opening} style={styles.button} hitSlop={4} accessibilityRole="button">
        {opening ? <ActivityIndicator /> : null}
        <Text style={styles.action}>{name ? m.busca_abrir_sessao() : m.switcher_arquivo()}</Text>
      </Pressable>
    </View>
    {openError ? <Text accessibilityRole="alert" style={styles.error}>{openError}</Text> : null}
    {loading ? <View style={styles.status}><ActivityIndicator /><Text style={styles.meta}>{m.busca_carregando_contexto()}</Text></View> : null}
    {error ? <View style={styles.status}><Text accessibilityRole="alert" style={styles.error}>{error}</Text>
      <Pressable onPress={load} style={styles.button} hitSlop={4} accessibilityRole="button"><Text style={styles.action}>{m.busca_tentar_de_novo()}</Text></Pressable>
    </View> : null}
    {!loading && !error ? <ScrollView ref={scroll} style={styles.body} contentContainerStyle={styles.messages}
      contentInsetAdjustmentBehavior={IOS ? 'automatic' : undefined} onContentSizeChange={(_width, height) => setContentHeight(height)}
      onScrollBeginDrag={() => { followingTarget.current = false; }}>
      {events.map((event) => {
        const isTarget = event.id === eventId;
        return <View key={event.id} testID={isTarget ? 'search-active-event' : undefined}
          style={isTarget ? styles.active : undefined}
          onLayout={isTarget ? (e) => setTargetY(e.nativeEvent.layout.y) : undefined}>
          {isTarget ? <Text style={styles.marker}>{m.busca_trecho_aqui()}</Text> : null}
          {event.kind === 'user_msg' ? <UserBubble text={event.text ?? ''} serverId={server.id} ts={event.ts} eventId={event.id} />
            : <AssistantBubble text={event.text ?? ''} serverId={server.id} ts={event.ts} />}
        </View>;
      })}
    </ScrollView> : null}
  </View>;
}

export default function SearchPoint() {
  const router = useRouter();
  const focused = useIsFocused();
  const glassHeader = useGlassHeader();
  const headerHeight = useHeaderHeight();
  const canGoBack = router.canGoBack();
  const title = m.busca_trecho_aqui();
  const headerOptions = useMemo(() => ({ ...glassHeader, title,
    headerLeft: canGoBack ? undefined : () => <HeaderBack onPress={() => router.replace('/search' as never)} />,
  }), [glassHeader, title, canGoBack, router]);
  const params = useLocalSearchParams<{ server: string; project: string; sid: string; eventId: string; name?: string; conversationTitle?: string; provider?: string; codexAccount?: string }>();
  const serverId = one(params.server), project = one(params.project), sid = one(params.sid), eventId = one(params.eventId), name = one(params.name);
  const server = useServers((s) => s.servers.find((item) => item.id === serverId && !item.invite && !item.removed && !item.disabled));
  if (!focused) return null;
  return <Screen edges={EDGES}>
    <Stack.Screen options={headerOptions} />
    <View style={[styles.body, { paddingTop: headerHeight }]}>
      {server && project && sid && eventId ? <Context key={JSON.stringify([server.id, server.baseUrl, server.token, project, sid, eventId, name, one(params.provider), one(params.codexAccount)])}
        server={server} project={project} sid={sid} eventId={eventId} name={name} conversationTitle={one(params.conversationTitle)}
        provider={one(params.provider)} codexAccount={one(params.codexAccount)} />
        : <Text accessibilityRole="alert" style={styles.error}>{m.busca_contexto_indisponivel()}</Text>}
    </View>
  </Screen>;
}

const styles = StyleSheet.create((theme) => ({
  body: { flex: 1 },
  actions: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingHorizontal: theme.base.space[4] },
  identity: { flex: 1, gap: theme.base.space[1] },
  title: { fontSize: theme.base.text.base, color: theme.tokens.text.primary },
  button: { minHeight: 44, minWidth: 44, paddingHorizontal: theme.base.space[2], flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  action: { fontSize: theme.base.text.sm, color: theme.tokens.accent.base },
  meta: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted },
  status: { padding: theme.base.space[4], gap: theme.base.space[2] },
  error: { paddingHorizontal: theme.base.space[4], fontSize: theme.base.text.sm, color: theme.tokens.status.error },
  messages: { padding: theme.base.space[4], gap: theme.base.space[4] },
  active: { borderLeftWidth: 3, borderLeftColor: theme.tokens.accent.base, paddingLeft: theme.base.space[3] },
  marker: { fontSize: theme.base.text.xs, color: theme.tokens.accent.base, marginBottom: theme.base.space[2] },
}));
