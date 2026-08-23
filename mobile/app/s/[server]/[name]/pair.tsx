import { useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Alert, Pressable, ScrollView, Text, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { Stack, useLocalSearchParams, useRouter } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { formataErro, getHistory, getPairContract, getSessions, pairSession, unpairSession } from '@hangar/core';
import type { ChatEvent } from '@hangar/core';
import { useServers } from '../../../../src/stores/servers';
import { useSessions } from '../../../../src/stores/sessions';
import { mkMarkdownStyle } from '../../../../src/chat/AssistantBubble';
import { PairMembers } from '../../../../src/features/pair/PairMembers';
import { PairPicker } from '../../../../src/features/pair/PairPicker';
import { PairFeed } from '../../../../src/features/pair/PairFeed';
import { montarFeed } from '../../../../src/features/pair/pairFeed';
import * as m from '../../../../src/paraglide/messages';

export default function PairSheet() {
  const { theme } = useUnistyles();
  const router = useRouter();
  const params = useLocalSearchParams<{ server: string; name: string }>();
  const serverId = Array.isArray(params.server) ? params.server[0] : (params.server ?? '');
  const name = Array.isArray(params.name) ? params.name[0] : (params.name ?? '');
  const ready = useServers((state) => state.ready);
  const routeServer = useServers((state) => state.servers.find((server) => server.id === serverId) ?? null);
  const current = useSessions((state) => state.rows.find((row) => row.serverId === serverId && row.name === name) ?? null);
  const peers = current?.pair_peers ?? [];
  const peersKey = peers.join('\u0000');

  const [sessions, setSessions] = useState<Awaited<ReturnType<typeof getSessions>>>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState('');
  const [routeMissing, setRouteMissing] = useState(false);
  const [picked, setPicked] = useState<string[]>([]);
  const [task, setTask] = useState('');
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState('');
  const [feed, setFeed] = useState<ReturnType<typeof montarFeed>['feed']>([]);
  const [feedFailed, setFeedFailed] = useState<string[]>([]);
  const [feedLoading, setFeedLoading] = useState(false);
  const [contract, setContract] = useState<{ path: string; content: string } | null>(null);
  const [contractError, setContractError] = useState('');
  const epoch = useRef(0);
  const mdStyle = useMemo(() => mkMarkdownStyle(theme), [theme]);

  useEffect(() => {
    setActionError('');
  }, [serverId, name]);

  useEffect(() => {
    if (!ready) return;
    const currentEpoch = ++epoch.current;
    setLoading(true);
    setLoadError('');
    setPicked([]);
    setTask('');
    setAdding(false);
    setFeed([]);
    setFeedFailed([]);
    setFeedLoading(false);
    setContract(null);
    setContractError('');

    if (!useServers.getState().ensureActive(serverId)) {
      setRouteMissing(true);
      setLoading(false);
      return;
    }
    setRouteMissing(false);

    const members = [name, ...peers];
    void (async () => {
      try {
        const all = await getSessions();
        if (currentEpoch !== epoch.current) return;
        setSessions(all.filter((session) => session.name !== name && session.state !== 'dead'));
        setLoadError('');
      } catch {
        if (currentEpoch === epoch.current) setLoadError(m.forward_nao_listou());
      }

      if (!peers.length) {
        if (currentEpoch === epoch.current) {
          setFeedLoading(false);
          setLoading(false);
        }
        return;
      }

      setFeedLoading(true);
      const results = await Promise.all(
        members.map((member) =>
          getHistory(member)
            .then((history) => ({ ok: true, h: history }))
            .catch(() => ({ ok: false, h: [] as ChatEvent[] })),
        ),
      );
      if (currentEpoch !== epoch.current) return;
      const built = montarFeed(members, results);
      setFeed(built.feed);
      setFeedFailed(built.failed);
      setFeedLoading(false);

      try {
        const shared = await getPairContract(name);
        if (currentEpoch === epoch.current) setContract({ path: shared.path, content: shared.content });
      } catch {
        if (currentEpoch === epoch.current) setContractError(m.arquivo_carregar_erro());
      } finally {
        if (currentEpoch === epoch.current) setLoading(false);
      }
    })();

    return () => {
      if (epoch.current === currentEpoch) epoch.current += 1;
    };
    // peersKey é a chave primitiva: o store cria um array novo a cada poll.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, serverId, name, peersKey]);

  const candidates = sessions.filter((session) => !peers.includes(session.name));

  function togglePick(peer: string) {
    setPicked((currentPicked) => currentPicked.includes(peer) ? currentPicked.filter((item) => item !== peer) : [...currentPicked, peer]);
  }

  async function doPair() {
    if (!picked.length || busy) return;
    const selected = picked;
    setBusy(true);
    setActionError('');
    try {
      const result = await pairSession(name, selected, task.trim());
      if (result.warning) {
        setActionError(formataErro(result.warning) ?? String(result.warning));
      } else {
        router.back();
      }
    } catch {
      setActionError(m.par_falhou_pareamento({ nomes: selected.join(', ') }));
    } finally {
      setBusy(false);
    }
  }

  async function doLeave() {
    if (busy) return;
    setBusy(true);
    setActionError('');
    try {
      const result = await unpairSession(name);
      if (result.warning) {
        setActionError(formataErro(result.warning) ?? String(result.warning));
      } else {
        router.back();
      }
    } catch {
      setActionError(m.par_falhou_saida());
    } finally {
      setBusy(false);
    }
  }

  function confirmLeave() {
    Alert.alert(
      m.comandos_confirmar({ n: m.par_sair_grupo() }),
      undefined,
      [
        { text: m.comum_cancelar(), style: 'cancel' },
        { text: m.comum_confirmar(), style: 'destructive', onPress: () => void doLeave() },
      ],
    );
  }

  function openPeer(peer: string) {
    router.push(`/s/${serverId}/${encodeURIComponent(peer)}` as never);
  }

  if (!ready) {
    return (
      <>
        <Stack.Screen options={sheetOptions} />
        <View style={[styles.center, { backgroundColor: theme.tokens.bg.base }]}>
          <ActivityIndicator color={theme.tokens.text.muted} />
          <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
        </View>
      </>
    );
  }

  if (routeMissing || !routeServer) {
    return (
      <>
        <Stack.Screen options={sheetOptions} />
        <View style={[styles.center, { backgroundColor: theme.tokens.bg.base }]}>
          <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert">{m.compare_servidor_nao_encontrado()}</Text>
          <Pressable onPress={() => router.back()} accessibilityRole="button">
            <Text style={[styles.back, { color: theme.tokens.accent.base }]}>{m.comum_voltar()}</Text>
          </Pressable>
        </View>
      </>
    );
  }

  return (
    <>
      <Stack.Screen options={sheetOptions} />
      <KeyboardAvoidingView behavior="padding" style={[styles.root, { backgroundColor: theme.tokens.bg.base }]}>
        <ScrollView
          contentInsetAdjustmentBehavior="automatic"
          keyboardShouldPersistTaps="handled"
          showsVerticalScrollIndicator={false}
          contentContainerStyle={styles.scroll}
        >
          {actionError ? <Text style={[styles.actionError, { color: theme.tokens.status.error }]} accessibilityRole="alert" selectable>{actionError}</Text> : null}
          {peers.length && loadError ? <Text style={[styles.actionError, { color: theme.tokens.status.error }]} accessibilityRole="alert" selectable>{loadError}</Text> : null}
          {peers.length ? (
            <>
              <PairMembers
                peers={peers}
                sessions={sessions}
                candidates={candidates}
                picked={picked}
                adding={adding}
                busy={busy}
                onOpenPeer={openPeer}
                onToggleAdding={() => { setAdding(true); setPicked([]); }}
                onToggle={togglePick}
                onAdd={() => void doPair()}
                onLeave={confirmLeave}
              />

              {contract?.content ? (
                <View style={[styles.contract, { borderTopColor: theme.tokens.border.subtle }]}>
                  <Text style={[styles.sectionTitle, { color: theme.tokens.text.secondary }]}>{m.par_contrato_titulo()}</Text>
                  <View style={[styles.contractBody, { backgroundColor: theme.tokens.bg.surface, borderColor: theme.tokens.border.subtle }]}>
                    <EnrichedMarkdownText markdown={contract.content} markdownStyle={mdStyle} flavor="github" />
                  </View>
                  <Text style={[styles.path, { color: theme.tokens.text.muted }]} numberOfLines={1} selectable>{contract.path}</Text>
                </View>
              ) : contractError ? (
                <Text style={[styles.contractError, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{contractError}</Text>
              ) : null}

              <PairFeed sessionName={name} feed={feed} failed={feedFailed} loading={feedLoading} />
            </>
          ) : (
            <PairPicker
              sessions={candidates}
              picked={picked}
              task={task}
              busy={busy}
              error={actionError || loadError}
              loading={loading}
              onToggle={togglePick}
              onTaskChange={setTask}
              onPair={() => void doPair()}
            />
          )}
        </ScrollView>
      </KeyboardAvoidingView>
    </>
  );
}

const sheetOptions = {
  headerShown: false,
  headerTransparent: true,
  sheetAllowedDetents: [0.92],
  sheetGrabberVisible: true,
  contentStyle: { backgroundColor: 'transparent' },
};

const styles = StyleSheet.create((theme) => ({
  root: { flex: 1 },
  scroll: { padding: theme.base.space[4], gap: theme.base.space[4], paddingBottom: theme.base.space[6] },
  center: { flex: 1, alignItems: 'center', justifyContent: 'center', gap: theme.base.space[2], padding: theme.base.space[4] },
  muted: { fontSize: theme.base.text.sm, textAlign: 'center' },
  error: { fontSize: theme.base.text.sm, textAlign: 'center' },
  back: { minHeight: 44, lineHeight: 44, fontSize: theme.base.text.sm },
  actionError: { fontSize: theme.base.text.sm, lineHeight: 20 },
  contract: { gap: theme.base.space[2], borderTopWidth: 1, paddingTop: theme.base.space[3] },
  sectionTitle: { fontSize: theme.base.text.sm, fontWeight: '700' },
  contractBody: { borderWidth: 1, borderRadius: theme.base.radius.md, padding: theme.base.space[3] },
  path: { fontSize: theme.base.text.xs },
  contractError: { fontSize: theme.base.text.xs },
}));
