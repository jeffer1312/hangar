import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Alert, Pressable, RefreshControl, SectionList, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { MenuView } from '@react-native-menu/menu';
import type { SwipeableMethods } from 'react-native-gesture-handler/ReanimatedSwipeable';
import { useRouter } from 'expo-router';
import {
  agruparSessoes,
  deleteSession,
  effectiveGroupBy,
  formataErro,
  renameSession,
  resumeSession,
  type AggSession,
  type GroupBy,
} from '@hangar/core';
import { useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { useAparencia, ehGroupBy } from '../../stores/aparencia';
import { Glass } from '../../ui/Glass';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { SessionRow } from './SessionRow';
import { AttentionStrip } from './AttentionStrip';
import { RenameSheet } from './RenameSheet';
import * as m from '../../paraglide/messages';
import { superficie } from '../../theme/superficie';

const ROTULO_AGRUPAR: Record<GroupBy, () => string> = {
  none: m.lista_agrupar_nenhum,
  server: m.lista_agrupar_servidor,
  project: m.lista_agrupar_projeto,
};

// Erro LANÇADO pelo apiFetch — já vem com mensagem pronta. O `formataErro` é pro envelope do
// corpo da resposta (o `warning` do excluir), e aplicado a um Error devolve o código cru.
const mensagemDe = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export function SessionList() {
  const { theme } = useUnistyles();
  const router = useRouter();
  const servers = useServers((s) => s.servers);
  const ready = useServers((s) => s.ready);
  const rows = useSessions((s) => s.rows);
  const loading = useSessions((s) => s.loading);
  const agrupar = useAparencia((s) => s.agrupar);
  const [filtro, setFiltro] = useState('');
  const [refreshing, setRefreshing] = useState(false);
  // guarda a sessão inteira, não o nome: dois servidores podem ter sessões de mesmo nome
  const [renomeando, setRenomeando] = useState<AggSession | null>(null);
  // uma linha aberta por vez: a anterior fecha quando outra abre, e ao rolar
  const aberta = useRef<SwipeableMethods | null>(null);
  const trocarAberta = useCallback((nova: SwipeableMethods | null) => {
    if (aberta.current && aberta.current !== nova) aberta.current.close();
    aberta.current = nova;
  }, []);

  // 1 stream por servidor via refcount compartilhado
  useEffect(() => {
    const release = useSessions.getState().retain();
    return () => release();
  }, []);

  // A API fala com o servidor ATIVO — uma ação numa linha de outro servidor iria pro lugar errado.
  const comServidor = useCallback((s: AggSession, fn: () => void) => {
    if (!useServers.getState().ensureActive(s.serverId)) {
      toast.erro(m.servidor_nao_existe());
      return;
    }
    fn();
  }, []);

  const abrir = useCallback((s: AggSession) => router.push(`/s/${s.serverId}/${s.name}` as never), [router]);
  const abrirGit = useCallback((s: AggSession) => router.push(`/s/${s.serverId}/${s.name}/files` as never), [router]);
  const abrirLoop = useCallback((s: AggSession) => router.push(`/s/${s.serverId}/${s.name}/loop` as never), [router]);

  const excluir = useCallback(
    (s: AggSession) =>
      Alert.alert(m.sessao_excluir(), s.name, [
        { text: m.comum_cancelar(), style: 'cancel' },
        {
          text: m.sessao_excluir_curto(),
          style: 'destructive',
          onPress: () =>
            comServidor(s, () => {
              deleteSession(s.name)
                .then((r) => {
                  // A sessão morreu, mas um companheiro do grupo pode não ter sido avisado: o
                  // motivo aparece em vez de a linha sumir calada (mesmo tratamento do desktop).
                  if (r.warning) toast.erro(formataErro(r.warning) ?? String(r.warning));
                  else toast.ok(s.name);
                })
                .catch((e: unknown) => toast.erro(mensagemDe(e)));
            }),
        },
      ]),
    [comServidor],
  );

  const renomear = useCallback(
    (novo: string) => {
      const s = renomeando;
      setRenomeando(null);
      if (!s) return;
      comServidor(s, () => {
        // o backend sanitiza o nome — mostra o que ficou, não o que foi digitado
        renameSession(s.name, novo)
          .then((r) => toast.ok(r.name))
          .catch((e: unknown) => toast.erro(mensagemDe(e)));
      });
    },
    [comServidor, renomeando],
  );

  const retomar = useCallback(
    (s: AggSession) =>
      comServidor(s, () => {
        resumeSession(s.name)
          .then((r) => {
            // Vários transcripts candidatos: escolher qual é decisão de quem lê a conversa.
            if ('ambiguous' in r) toast.erro(m.sessao_retomar_qual());
            else toast.ok(s.name);
          })
          .catch((e: unknown) => toast.erro(mensagemDe(e)));
      }),
    [comServidor],
  );

  const onRefresh = useCallback(() => {
    setRefreshing(true);
    useSessions.getState().reconnect();
    // A reconexão é síncrona no store; damos um tick pro SSE emitir antes de parar o spinner.
    setTimeout(() => setRefreshing(false), 600);
  }, []);

  const modo = effectiveGroupBy(agrupar, servers.length);
  const busca = filtro.trim().toLowerCase();
  // Filtrar + ordenar + agrupar a lista inteira a cada render é caro, e arrastar um slider de
  // material re-renderiza todo mundo que lê o tema — inclusive esta tela.
  const grupos = useMemo(() => {
    const visiveis = busca
      ? rows.filter((r) => r.name.toLowerCase().includes(busca) || (r.cwd ?? '').toLowerCase().includes(busca))
      : rows;
    return agruparSessoes(visiveis, modo);
  }, [rows, busca, modo]);
  const mostrarServidor = modo !== 'server' && servers.length > 1;

  const cabecalho = (
    <View style={styles.topo}>
      <AttentionStrip sessions={rows} onOpen={abrir} />
      <View style={styles.barra}>
        <View style={[styles.campo, { backgroundColor: superficie(theme, 0.8), borderColor: theme.tokens.border.subtle }]}>
          <Icon name="Search" size={16} color={theme.tokens.text.muted} />
          <TextInput
            value={filtro}
            onChangeText={setFiltro}
            placeholder={m.lista_filtro_placeholder()}
            placeholderTextColor={theme.tokens.text.muted}
            returnKeyType="search"
            autoCapitalize="none"
            autoCorrect={false}
            accessibilityLabel={m.lista_filtrar()}
            style={[styles.input, { color: theme.tokens.text.primary }]}
          />
          {filtro ? (
            <Pressable onPress={() => setFiltro('')} hitSlop={8} accessibilityRole="button" accessibilityLabel={m.comum_cancelar()}>
              <Icon name="X" size={16} color={theme.tokens.text.muted} />
            </Pressable>
          ) : null}
        </View>
        <MenuView
          onPressAction={({ nativeEvent }) => {
            if (ehGroupBy(nativeEvent.event)) useAparencia.getState().setAgrupar(nativeEvent.event);
          }}
          actions={(['server', 'project', 'none'] as GroupBy[]).map((g) => ({
            id: g,
            title: ROTULO_AGRUPAR[g](),
            state: g === agrupar ? ('on' as const) : ('off' as const),
          }))}
        >
          <View style={styles.agrupar} accessibilityRole="button" accessibilityLabel={m.lista_agrupar()}>
            <Icon name="ListFilter" size={18} color={theme.tokens.text.secondary} />
          </View>
        </MenuView>
      </View>
    </View>
  );

  if (!ready) {
    return (
      <View style={styles.empty}>
        <Text style={styles.emptyTxt}>{m.comum_carregando()}</Text>
      </View>
    );
  }

  if (servers.length === 0) {
    return (
      <View style={styles.empty}>
        <Text style={styles.emptyTitle}>{m.lista_nenhum_servidor()}</Text>
        <Text style={styles.emptyTxt}>{m.lista_pareie_qr()}</Text>
      </View>
    );
  }

  if (loading && rows.length === 0) {
    return (
      <View style={styles.empty}>
        <Text style={styles.emptyTxt}>{m.lista_carregando()}</Text>
      </View>
    );
  }

  return (
    <>
      <SectionList
        sections={grupos.map((g) => ({ ...g, data: g.sessions }))}
        keyExtractor={(item) => `${item.serverId}::${item.name}`}
        stickySectionHeadersEnabled
        ListHeaderComponent={cabecalho}
        renderSectionHeader={({ section }) =>
          modo === 'none' ? null : (
            <Glass variant="chrome" style={styles.header}>
              <Text style={[styles.headerTxt, { color: section.color ?? theme.tokens.text.secondary }]} numberOfLines={1}>
                {section.label}
              </Text>
            </Glass>
          )
        }
        renderItem={({ item }) => (
          <SessionRow
            session={item}
            mostrarServidor={mostrarServidor}
            onPress={() => abrir(item)}
            onGit={() => abrirGit(item)}
            onExcluir={() => excluir(item)}
            onRenomear={() => setRenomeando(item)}
            onLoop={() => abrirLoop(item)}
            onResume={() => retomar(item)}
            aoAbrir={trocarAberta}
          />
        )}
        onScrollBeginDrag={() => trocarAberta(null)}
        ItemSeparatorComponent={() => <View style={[styles.sep, { backgroundColor: theme.tokens.border.subtle }]} />}
        ListEmptyComponent={
          <View style={styles.empty}>
            <Text style={styles.emptyTitle}>{busca ? m.lista_vazia_filtro() : m.lista_nenhuma_ativa()}</Text>
            {busca ? null : <Text style={styles.emptyTxt}>{m.lista_toque_criar()}</Text>}
          </View>
        }
        contentContainerStyle={styles.listContent}
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={onRefresh} />}
      />
      <RenameSheet nome={renomeando?.name ?? null} onConfirmar={renomear} onFechar={() => setRenomeando(null)} />
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  listContent: { paddingBottom: theme.base.space[6] },
  topo: { paddingBottom: theme.base.space[2] },
  barra: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[3] },
  campo: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: 6, paddingHorizontal: 10, borderRadius: theme.base.radius.md, borderWidth: 1, minHeight: 40 },
  input: { flex: 1, fontSize: theme.base.text.sm, paddingVertical: 8 },
  agrupar: { width: 40, height: 40, alignItems: 'center', justifyContent: 'center' },
  header: { borderRadius: 0, borderWidth: 0, paddingHorizontal: theme.base.space[3], paddingVertical: 6 },
  headerTxt: { fontSize: theme.base.text.xxs, fontWeight: '700', letterSpacing: 0.4, textTransform: 'uppercase' },
  sep: { height: 1, marginLeft: 34 },
  empty: { flex: 1, justifyContent: 'center', alignItems: 'center', padding: theme.base.space[6], gap: 8 },
  emptyTitle: { fontSize: theme.base.text.lg, fontWeight: '600', color: theme.tokens.text.primary },
  emptyTxt: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center' },
}));
