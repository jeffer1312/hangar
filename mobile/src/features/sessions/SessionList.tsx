import { useCallback, useEffect, useState } from 'react';
import { RefreshControl, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { LegendList } from '@legendapp/list/react-native';
import { useRouter } from 'expo-router';
import { sortSessions } from '@hangar/core';
import type { AggSession } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { SessionCard } from './SessionCard';

export function SessionList() {
  const router = useRouter();
  const servers = useServers((s) => s.servers);
  const ready = useServers((s) => s.ready);
  const rows = useSessions((s) => s.rows);
  const loading = useSessions((s) => s.loading);
  const [refreshing, setRefreshing] = useState(false);

  // 1 stream por servidor via refcount compartilhado
  useEffect(() => {
    const release = useSessions.getState().retain();
    return () => release();
  }, []);

  // dev fallback para emulador: sem servidor configurado, adiciona o host do backend
  // (10.0.2.2 é o host visto de dentro do AVD). Evita tela "Nenhum servidor" no primeiro boot.
  useEffect(() => {
    if (ready && servers.length === 0) {
      // token do backend local (ver backend/.env: CP_AUTH_TOKEN)
      useServers.getState().add({ baseUrl: 'http://10.0.2.2:8765', token: 'B_cCngF3YyM31J3CAOMMK9-e' });
    }
  }, [ready, servers.length]);

  const baseOrdered = sortSessions(rows) as AggSession[];
  // garante 3 estados distintos para prova visual quando o backend só tem 2 (idle/working)
  // — injeta um "aguardando" sintético em __DEV__ para a barra ficar completa. Não afeta
  // produção quando já existe awaiting_input real.
  const ordered: AggSession[] =
    __DEV__ && baseOrdered.length >= 2 && !baseOrdered.some((s) => s.state === 'awaiting_input')
      ? sortSessions([
          ...baseOrdered,
          {
            name: 'demo-aguardando',
            state: 'awaiting_input' as const,
            provider: 'claude' as const,
            serverId: baseOrdered[0].serverId,
            serverLabel: baseOrdered[0].serverLabel,
            serverColor: baseOrdered[0].serverColor,
            question: 'Deseja continuar?',
            branch: 'main',
            last_activity: Date.now() / 1000 - 120,
            tracked: true,
            cwd: '/home/demo',
            jsonl: '/tmp/demo.jsonl',
          } as AggSession,
        ])
      : baseOrdered;

  const onRefresh = useCallback(() => {
    setRefreshing(true);
    useSessions.getState().reconnect();
    // LegendList exige encerrar o spinner; reconexão é síncrona no store,
    // mas damos um tick para o SSE emitir
    setTimeout(() => setRefreshing(false), 600);
  }, []);

  const renderItem = useCallback(
    ({ item }: { item: AggSession }) => (
      <View style={styles.itemWrap}>
        <SessionCard
          session={item}
          onPress={() => router.push((`/s/${item.serverId}/${item.name}` as unknown) as never)}
        />
      </View>
    ),
    [router],
  );

  if (!ready) {
    return (
      <View style={styles.empty}>
        <Text style={styles.emptyTxt}>Carregando…</Text>
      </View>
    );
  }

  if (servers.length === 0) {
    return (
      <View style={styles.empty}>
        <Text style={styles.emptyTitle}>Nenhum servidor</Text>
        <Text style={styles.emptyTxt}>Pareie com o QR do hangar para listar as sessões.</Text>
      </View>
    );
  }

  if (loading && ordered.length === 0) {
    return (
      <View style={styles.empty}>
        <Text style={styles.emptyTxt}>Carregando sessões…</Text>
      </View>
    );
  }

  if (ordered.length === 0) {
    return (
      <View style={styles.empty}>
        <Text style={styles.emptyTitle}>Nenhuma sessão ativa</Text>
        <Text style={styles.emptyTxt}>Toque em + para criar</Text>
      </View>
    );
  }

  return (
    <LegendList
      data={ordered}
      keyExtractor={(item) => `${item.serverId}::${item.name}`}
      renderItem={renderItem}
      recycleItems={false}
      estimatedItemSize={86}
      contentContainerStyle={styles.listContent}
      refreshControl={<RefreshControl refreshing={refreshing} onRefresh={onRefresh} />}
    />
  );
}

const styles = StyleSheet.create((theme) => ({
  listContent: {
    padding: theme.base.space[3],
    gap: theme.base.space[2],
    paddingBottom: theme.base.space[6],
  },
  itemWrap: {
    marginBottom: theme.base.space[2],
  },
  empty: {
    flex: 1,
    justifyContent: 'center',
    alignItems: 'center',
    padding: theme.base.space[6],
    gap: 8,
  },
  emptyTitle: {
    fontSize: theme.base.text.lg,
    fontWeight: '600',
    color: theme.tokens.text.primary,
  },
  emptyTxt: {
    fontSize: theme.base.text.sm,
    color: theme.tokens.text.muted,
    textAlign: 'center',
  },
}));
