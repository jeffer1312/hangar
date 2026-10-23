import { useCallback, useEffect, useMemo, useState } from 'react';
import { ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { groupGlobal, type AggSession } from '@hangar/core';
import { Sheet } from '../ui/Sheet';
import { superficie } from '../theme/superficie';
import { useSessions } from '../stores/sessions';
import { useIdentities } from '../features/pair/identities';
import { SessionRow } from '../features/sessions/SessionRow';
import { GroupBlockHeader, SessionSectionCard, SessionSectionHeader } from '../features/sessions/SessionSection';
import { buildSessionSections, sessionGroupKey } from '../features/sessions/sessionSections';
import * as m from '../paraglide/messages';

// Trocar de sessão sem voltar pra lista. `replace` e não `push`: empilhar chats faria o voltar
// percorrer a fila de sessões visitadas em vez de sair do chat.
export function SessionPickerSheet({
  open,
  onClose,
  atual,
  serverId,
}: {
  open: boolean;
  onClose: () => void;
  atual: string;
  serverId: string;
}) {
  // O TrueSheet desenha os filhos mesmo fechado: a lista só existe da abertura até a folha sumir.
  const [shown, setShown] = useState(open);
  if (open && !shown) setShown(true);
  useEffect(() => {
    if (!shown) return;
    return useSessions.getState().retain();
  }, [shown]);

  return (
    <Sheet open={open} sizes={['medium', 'large']} onDismiss={() => { setShown(false); onClose(); }} scrollable>
      {shown ? <PickerList onClose={onClose} atual={atual} serverId={serverId} /> : null}
    </Sheet>
  );
}

function PickerList({ onClose, atual, serverId }: { onClose: () => void; atual: string; serverId: string }) {
  const { theme } = useUnistyles();
  const insets = useSafeAreaInsets();
  const router = useRouter();
  const rows = useSessions((s) => s.rows);
  const byServer = useSessions((s) => s.byServer);
  const loading = useSessions((s) => s.loading);
  const machineStatus = byServer.filter((bucket) => bucket.server.inviteEnded || bucket.error || !bucket.loaded);
  const effectiveLoading = byServer.length
    ? byServer.some((bucket) => !bucket.server.inviteEnded && !bucket.loaded && !bucket.error)
    : loading;
  const [query, setQuery] = useState('');
  const [collapsedGroups, setCollapsedGroups] = useState<ReadonlySet<string>>(() => new Set());
  const search = query.trim().toLowerCase();
  const liveRows = useMemo(() => rows.filter((s) => s.state !== 'dead'), [rows]);
  // Como na lista: o identificador de cada máquina só é lido quando há grupo entre máquinas.
  const crossMachine = useMemo(() => liveRows.some((r) => r.pair_owner || r.pair_peers?.some((p) => p.includes('::'))), [liveRows]);
  const servers = useMemo(() => crossMachine ? byServer.map((b) => b.server) : [], [crossMachine, byServer]);
  const { identities } = useIdentities(servers);
  const pairGroups = useMemo(() => groupGlobal(liveRows, identities), [liveRows, identities]);
  const groups = useMemo(() => {
    const current = liveRows.find((s) => s.name === atual && s.serverId === serverId);
    const currentServer = current?.outside?.localServerId ?? serverId;
    return buildSessionSections(liveRows, 'server', search, collapsedGroups, currentServer, pairGroups);
  }, [liveRows, atual, serverId, search, collapsedGroups, pairGroups]);
  const multipleServers = useMemo(() => new Set(liveRows.map((s) => s.serverId)).size > 1, [liveRows]);
  const toggleGroup = useCallback((id: string) => setCollapsedGroups((current) => {
    const next = new Set(current);
    const key = sessionGroupKey('server', id);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  }), []);
  const selectSession = useCallback((session: AggSession) => {
    if (session.name === atual && session.serverId === serverId) return;
    onClose();
    router.replace(`/s/${session.serverId}/${session.name}` as never);
  }, [atual, serverId, onClose, router]);

  return (
    <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.container}>
    <ScrollView style={styles.container}
      contentContainerStyle={[styles.inner, { paddingBottom: insets.bottom + theme.base.space[3] }]}
      keyboardShouldPersistTaps="handled" keyboardDismissMode="on-drag">
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.sessao_trocar_de()}</Text>
      {liveRows.length > 8 || query ? <TextInput
        value={query}
        onChangeText={setQuery}
        placeholder={m.switcher_buscar_sessao()}
        accessibilityLabel={m.switcher_buscar_sessao()}
        placeholderTextColor={theme.tokens.text.muted}
        autoCapitalize="none"
        autoCorrect={false}
        returnKeyType="search"
        style={[styles.search, { color: theme.tokens.text.primary, backgroundColor: superficie(theme, 0.6) }]}
      /> : null}
      {machineStatus.map((bucket) => <Text key={bucket.server.id}
        accessibilityRole={bucket.error ? 'alert' : undefined}
        style={[styles.meta, { color: bucket.error ? theme.tokens.status.error : theme.tokens.text.muted }]}>
        {bucket.server.inviteEnded ? `${bucket.server.label}: ${m.convite_encerrado()}`
          : bucket.error === 'offline' ? m.lista_servidor_offline({ label: bucket.server.label })
          : `${bucket.server.label}: ${bucket.error || m.lista_carregando()}`}
      </Text>)}
      {effectiveLoading && !machineStatus.length ? <Text style={[styles.meta, { color: theme.tokens.text.muted }]}>{m.lista_carregando()}</Text> : null}
      {!groups.length && !effectiveLoading && machineStatus.every((bucket) => bucket.server.inviteEnded) ? <Text style={[styles.meta, { color: theme.tokens.text.muted }]}>
        {search ? m.lista_vazia_filtro() : m.lista_nenhuma_ativa()}
      </Text> : null}
      {groups.map((group) => <View key={group.id}>
        <SessionSectionHeader section={group} onToggle={toggleGroup} />
        {group.data.map((session, index) => {
          const head = group.heads.get(`${session.serverId}::${session.name}`);
          return <SessionSectionCard key={`${session.serverId}::${session.name}`} index={index} total={group.data.length}>
            {head ? <GroupBlockHeader head={head} onToggle={toggleGroup} /> : null}
            {head?.collapsed ? null : <SessionRow session={session} compact
              selected={session.name === atual && session.serverId === serverId}
              mostrarServidor={!!session.outside || (multipleServers && (group.id.startsWith('state:') || session.serverId !== group.id))}
              onPress={selectSession} />}
          </SessionSectionCard>;
        })}
      </View>)}
    </ScrollView>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create((theme) => ({
  container: { flex: 1 },
  inner: { padding: theme.base.space[3] },
  title: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold, marginBottom: theme.base.space[2] },
  search: { fontSize: theme.base.text.sm, minHeight: 44, paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md, marginBottom: theme.base.space[2] },
  meta: { fontSize: theme.base.text.xs, flexShrink: 1 },
}));
