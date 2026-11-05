import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
import { ActivityIndicator, AppState, FlatList, Pressable, RefreshControl, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import {
  getArchiveRecentForServer,
  mergeConversations,
  rememberEntry,
  relativeTime,
  rotuloEstado,
  type ArchiveEntry,
  type ConversationRow,
  type State,
} from '@hangar/core';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { Icon } from '../../ui/Icon';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { superficie } from '../../theme/superficie';
import { HeadlessSignal } from './HeadlessSignal';
import * as m from '../../paraglide/messages';

const PILL_DO_ESTADO: Record<State, 'working' | 'idle' | 'input' | 'dead'> = {
  working: 'working',
  idle: 'idle',
  awaiting_input: 'input',
  dead: 'dead',
};

const chave = (r: ConversationRow) =>
  r.kind === 'live' ? `l:${r.serverId}:${r.name}` : `c:${r.serverId}:${r.entry.project}:${r.entry.session_id}`;

// Modo "Conversas" da gaveta: vivas e fechadas juntas, por recência. As vivas vêm do store da lista
// (um stream por servidor, que o SessionList já retém); as fechadas, dos recentes de cada servidor.
export function ConversationList({ header, onClose }: { header: ReactElement; onClose: () => void }) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const servers = useServers((s) => s.servers);
  const rows = useSessions((s) => s.rows);
  const byServer = useSessions((s) => s.byServer);
  const [closedByServer, setClosedByServer] = useState(() => new Map<string, ArchiveEntry[]>());
  // Por id: dois servidores podem ter o mesmo rótulo (o rótulo sai do primeiro número do IP).
  const [failed, setFailed] = useState<{ id: string; label: string }[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const closedRef = useRef(closedByServer);
  const seq = useRef(0);

  const live = useMemo(() => {
    const own = new Set(servers.filter((s) => !s.invite).map((s) => s.id));
    return rows.filter((r) => own.has(r.serverId));
  }, [rows, servers]);
  const list = useMemo(() => mergeConversations(live, closedByServer), [live, closedByServer]);

  const load = useCallback(async () => {
    const mine = ++seq.current;
    // Servidor fora do ar não é consultado: cada tentativa custaria o teto de 8 s da lista inteira.
    const down = new Set(useSessions.getState().byServer.filter((b) => b.error).map((b) => b.server.id));
    const own = useServers.getState().servers.filter((s) => !s.invite);
    const tried = own.filter((s) => !down.has(s.id));
    const results = await Promise.allSettled(tried.map((s) => getArchiveRecentForServer(s)));
    if (mine !== seq.current) return;
    const prev = closedRef.current;
    const next = new Map<string, ArchiveEntry[]>();
    const bad: { id: string; label: string }[] = [];
    // Servidor fora do ar ou que falhou agora entra na linha de falha e mantém a última lista boa.
    for (const s of own) {
      if (!down.has(s.id)) continue;
      bad.push({ id: s.id, label: s.label });
      const old = prev.get(s.id);
      if (old) next.set(s.id, old);
    }
    results.forEach((r, i) => {
      if (r.status === 'fulfilled') next.set(tried[i].id, r.value);
      // Convidado não tem Arquivo: a máquina respondeu, só não há o que listar.
      else if ((r.reason as { code?: string } | undefined)?.code === 'erro_fora_do_convidado') next.set(tried[i].id, []);
      else {
        bad.push({ id: tried[i].id, label: tried[i].label });
        const old = prev.get(tried[i].id);
        if (old) next.set(tried[i].id, old);
      }
    });
    closedRef.current = next;
    setClosedByServer(next);
    setFailed(bad);
    setLoaded(true);
  }, []);

  useEffect(() => {
    void load();
    const sub = AppState.addEventListener('change', (s) => { if (s === 'active') void load(); });
    return () => {
      seq.current++;
      sub.remove();
    };
  }, [load]);

  // Sessão que fechou passa a existir só no arquivo: relê pra ela aparecer como fechada.
  const liveKeys = live.map((s) => `${s.serverId}::${s.name}`).join('\n');
  const prevLive = useRef(new Set<string>());
  useEffect(() => {
    const now = new Set(liveKeys ? liveKeys.split('\n') : []);
    const shrank = [...prevLive.current].some((k) => !now.has(k));
    prevLive.current = now;
    if (shrank) void load();
  }, [liveKeys, load]);

  // Servidor que voltou ao ar: relê os recentes dele.
  const downKeys = byServer.filter((b) => b.error).map((b) => b.server.id).join('\n');
  const prevDown = useRef(new Set<string>());
  useEffect(() => {
    const now = new Set(downKeys ? downKeys.split('\n') : []);
    const recovered = [...prevDown.current].some((id) => !now.has(id));
    prevDown.current = now;
    if (recovered) void load();
  }, [downKeys, load]);

  const onRefresh = useCallback(() => {
    setRefreshing(true);
    void load().finally(() => setRefreshing(false));
  }, [load]);

  const open = useCallback((row: ConversationRow) => {
    onClose();
    if (row.kind === 'live') {
      router.push(`/s/${row.serverId}/${row.name}` as never);
      return;
    }
    rememberEntry(row.serverId, row.entry);
    router.push(`/archive/${row.serverId}/${encodeURIComponent(row.entry.project)}/${encodeURIComponent(row.entry.session_id)}` as never);
  }, [onClose, router]);

  const muted = theme.tokens.text.muted;
  const renderItem = useCallback(({ item }: { item: ConversationRow }) => {
    const session = item.kind === 'live' ? live.find((s) => s.serverId === item.serverId && s.name === item.name) : undefined;
    const provider = session?.provider;
    const estado = item.kind === 'live' ? (item.state as State) : null;
    const when = relativeTime(item.at);
    return (
      <Pressable
        onPress={() => open(item)}
        style={({ pressed }) => [styles.row, pressed && { backgroundColor: superficie(theme, 0.6) }]}
        accessibilityRole="button"
        accessibilityLabel={[item.title, session?.headless === true ? m.criar_modo_exec_headless() : '', estado && PILL_DO_ESTADO[estado] ? rotuloEstado(estado) : '', when].filter(Boolean).join(', ')}
      >
        <View style={styles.lead}>
          {item.kind === 'live' ? (
            <>
              {provider ? <ProviderGlyph provider={provider} size={18} /> : <Icon name="MessageCircle" size={18} color={muted} />}
              {estado && PILL_DO_ESTADO[estado] ? (
                <View style={[styles.dot, { backgroundColor: theme.tokens.pill[PILL_DO_ESTADO[estado]].fg, borderColor: theme.tokens.bg.base }]} />
              ) : null}
            </>
          ) : (
            <Icon name="MessageCircle" size={18} color={muted} />
          )}
        </View>
        <View style={styles.titleRow}>
          <Text style={[styles.title, { color: theme.tokens.text.primary }]} numberOfLines={1}>{item.title}</Text>
          {session?.headless === true ? <HeadlessSignal /> : null}
        </View>
        <Text style={[styles.when, { color: muted }]} numberOfLines={1}>{when}</Text>
      </Pressable>
    );
  }, [live, open, theme, muted]);

  return (
    <FlatList
      data={list}
      keyExtractor={chave}
      renderItem={renderItem}
      ListHeaderComponent={
        <>
          {header}
          <Text style={[styles.section, { color: muted }]} accessibilityRole="header">{m.conversas_recentes()}</Text>
        </>
      }
      ListEmptyComponent={
        !loaded ? (
          <View style={styles.empty}>
            <ActivityIndicator />
            <Text style={[styles.emptyTxt, { color: muted }]}>{m.comum_carregando()}</Text>
          </View>
        ) : failed.length === 0 ? (
          <Text style={[styles.emptyTxt, styles.emptyPad, { color: muted }]}>{m.conversas_vazio()}</Text>
        ) : null
      }
      ListFooterComponent={
        failed.length ? (
          <View>
            {failed.map((s) => (
              <Text key={s.id} style={[styles.failed, { color: theme.tokens.status.warning }]} accessibilityRole="alert">
                {m.conversas_servidor_falhou({ servidor: s.label })}
              </Text>
            ))}
          </View>
        ) : null
      }
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
      refreshControl={<RefreshControl refreshing={refreshing} onRefresh={onRefresh} />}
    />
  );
}

const styles = StyleSheet.create((theme) => ({
  content: { flexGrow: 1, paddingBottom: theme.base.space[2] },
  section: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, paddingTop: theme.base.space[3], paddingBottom: theme.base.space[1], paddingHorizontal: theme.base.space[3] },
  row: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], minHeight: 48, paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.lg },
  lead: { width: 22, alignItems: 'center' },
  dot: { position: 'absolute', right: -3, bottom: -3, width: 8, height: 8, borderRadius: theme.base.radius.full, borderWidth: 1 },
  titleRow: { flex: 1, minWidth: 0, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  title: { flexShrink: 1, minWidth: 0, fontSize: theme.base.text.base },
  when: { flexShrink: 0, fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'] },
  empty: { alignItems: 'center', gap: theme.base.space[2], paddingTop: theme.base.space[6] },
  emptyTxt: { fontSize: theme.base.text.sm, textAlign: 'center' },
  emptyPad: { padding: theme.base.space[6] },
  failed: { fontSize: theme.base.text.sm, paddingVertical: theme.base.space[2], paddingHorizontal: theme.base.space[3] },
}));
