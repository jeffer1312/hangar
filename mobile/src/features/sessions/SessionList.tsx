import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Alert, Pressable, RefreshControl, SectionList, Text, TextInput, View, ActivityIndicator } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { MenuView } from '@react-native-menu/menu';
import { useRouter } from 'expo-router';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import {
  agruparSessoes,
  deleteSession,
  effectiveGroupBy,
  formataErro,
  renameSession,
  setPairAccess,
  resumeSession,
  type AggSession,
  type GroupBy,
  type Server,
} from '@hangar/core';
import { ownServers, useServers } from '../../stores/servers';
import { useSessions } from '../../stores/sessions';
import { useAparencia, ehGroupBy } from '../../stores/aparencia';
import { HangarMark } from '../../ui/HangarMark';
import { LiveVoiceChatButton } from '../voice/LiveVoiceChatButton';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { SessionRow, type PairAction } from './SessionRow';
import { placeOutside } from './outsideRows';
import { PairInviteSheet, type PairTarget } from '../pair/PairInviteSheet';
import { PairAcceptSheet, type PairAcceptTarget } from '../pair/PairAcceptSheet';
import { leaveExternal } from '../pair/leaveExternal';
import { ConversationList } from './ConversationList';
import { splitAttention } from './AttentionStrip';
import { RenameSheet } from './RenameSheet';
import { WorktreeSheet } from '../worktrees/WorktreeSheet';
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

// Um grupo do painel: "Precisa de você" no topo, depois os grupos do Agrupar por. `total` sobrevive
// ao grupo recolhido (aí `data` fica vazio e o cabeçalho continua contando).
interface Section {
  id: string;
  label: string;
  color: string | null;
  attention: boolean;
  total: number;
  collapsed: boolean;
  data: AggSession[];
}

interface Props {
  onClose: () => void;
  onOpenServers: () => void;
  onNewConversation?: () => void;
  variant?: 'drawer' | 'home';
}

export function SessionList({ onClose, onOpenServers, onNewConversation = onClose, variant = 'drawer' }: Props) {
  const home = variant === 'home';
  const { theme } = useUnistyles();
  const router = useRouter();
  const todos = useServers((s) => s.servers);
  // A entrada do par de fora não é máquina desta pessoa: não conta nem aparece como servidor.
  const servers = useMemo(() => ownServers(todos), [todos]);
  const ready = useServers((s) => s.ready);
  const rows = useSessions((s) => s.rows);
  const loading = useSessions((s) => s.loading);
  const agrupar = useAparencia((s) => s.agrupar);
  const organizar = useAparencia((s) => s.organizar);
  const conversas = !home && organizar === 'conversations';
  const [readyExpanded, setReadyExpanded] = useState(false);
  const activeId = useServers((s) => s.activeId);
  const byServer = useSessions((s) => s.byServer);
  const [filtro, setFiltro] = useState('');
  const insets = useSafeAreaInsets();
  // chave `modo:id`: trocar o Agrupar por não herda o recolhido de um grupo de outro tipo
  const [recolhidos, setRecolhidos] = useState<ReadonlySet<string>>(() => new Set());
  const [refreshing, setRefreshing] = useState(false);
  // guarda a sessão inteira, não o nome: dois servidores podem ter sessões de mesmo nome
  const [renomeando, setRenomeando] = useState<AggSession | null>(null);
  const [wt, setWt] = useState<{ server: Server; path: string } | null>(null);
  const [convite, setConvite] = useState<PairTarget | null>(null);
  const [aceite, setAceite] = useState<PairAcceptTarget | null>(null);
  const deleting = useRef(new Map<string, { confirmed: boolean }>());

  // 1 stream por servidor via refcount compartilhado
  useEffect(() => {
    const release = useSessions.getState().retain();
    return () => release();
  }, []);

  // A API fala com o servidor ATIVO — uma ação numa linha de outro servidor iria pro lugar errado.
  const comServidor = useCallback((s: AggSession, fn: () => void) => {
    if (!useServers.getState().ensureActive(s.serverId)) {
      toast.erro(m.servidor_nao_existe());
      return false;
    }
    fn();
    return true;
  }, []);

  const abrir = useCallback((s: AggSession) => {
    onClose();
    router.push(`/s/${s.serverId}/${s.name}` as never);
  }, [router, onClose]);
  const abrirGit = useCallback((s: AggSession) => {
    onClose();
    router.push(`/s/${s.serverId}/${s.name}/files` as never);
  }, [router, onClose]);
  // Convite fica de fora: o backend recusa worktrees ao convidado.
  const proprios = useMemo(() => new Set(servers.filter((x) => !x.invite).map((x) => x.id)), [servers]);
  // A folha fala com o servidor da linha, não com o ativo: não precisa trocar de servidor.
  const abrirWorktree = useCallback((s: AggSession, path: string) => {
    const server = useServers.getState().servers.find((x) => x.id === s.serverId);
    if (server) setWt({ server, path });
    else toast.erro(m.servidor_nao_existe());
  }, []);

  const excluir = useCallback(
    (s: AggSession) => {
      const server = useServers.getState().servers.find((server) => server.id === s.serverId);
      if (server?.invite) {
        useServers.getState().remove(server.id);
        return;
      }
      const key = `${s.serverId}::${s.name}`;
      if (deleting.current.has(key)) return;
      const reservation = { confirmed: false };
      deleting.current.set(key, reservation);
      const release = () => {
        if (deleting.current.get(key) === reservation) deleting.current.delete(key);
      };
      const cancel = () => {
        if (!reservation.confirmed) release();
      };
      Alert.alert(m.sessao_excluir(), s.name, [
        { text: m.comum_cancelar(), style: 'cancel', onPress: cancel },
        {
          text: m.sessao_excluir_curto(),
          style: 'destructive',
          onPress: () => {
            if (deleting.current.get(key) !== reservation || reservation.confirmed) return;
            reservation.confirmed = true;
            if (!comServidor(s, () => {
              useSessions.getState().markDeleting(s.serverId, s.name);
              deleteSession(s.name)
                .then((r) => {
                  // A sessão morreu, mas um companheiro do grupo pode não ter sido avisado: o
                  // motivo aparece em vez de a linha sumir calada (mesmo tratamento do desktop).
                  if (r.warning) toast.erro(formataErro(r.warning) ?? String(r.warning));
                  else toast.ok(s.name);
                })
                .catch((e: unknown) => {
                  useSessions.getState().unmarkDeleting(s.serverId, s.name);
                  toast.erro(mensagemDe(e));
                })
                .finally(release);
            })) release();
          },
        },
      ], { cancelable: true, onDismiss: cancel });
    },
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

  // O par é desfeito e liberado no servidor PRÓPRIO, por endereço explícito: não troca a ativa.
  const fazerPar = useCallback((s: AggSession, a: PairAction) => {
    const own = (id: string) => useServers.getState().servers.find((x) => x.id === id && !x.invite);
    const server = own(s.outside?.localServerId ?? s.serverId);
    if (!server) { toast.erro(m.servidor_nao_existe()); return; }
    const sair = (alias: string, session: string) => leaveExternal(alias, session, server);
    if (a.kind === 'invite') {
      setConvite({ server, name: s.name, members: (s.pair_peers ?? []).filter((p) => !p.includes('::')) });
    } else if (a.kind === 'accept') {
      setAceite({ server, name: s.name, sessions: useSessions.getState().rows.filter((r) => r.serverId === server.id) });
    } else if (a.kind === 'leave' && s.outside) {
      const { alias, owner } = s.outside;
      Alert.alert(m.pair_leave_outside({ dono: owner }), m.pair_leave_outside_body({ dono: owner }), [
        { text: m.comum_cancelar(), style: 'cancel' },
        { text: m.pair_leave_outside({ dono: owner }), style: 'destructive', onPress: () => sair(alias, s.name) },
      ]);
    } else if (a.kind === 'undo') {
      const { alias, owner, session } = a.pair;
      Alert.alert(m.pair_undo_own({ dono: owner }), m.pair_undo_anchor_body({ dono: owner }), [
        { text: m.comum_cancelar(), style: 'cancel' },
        { text: m.pair_undo_own({ dono: owner }), style: 'destructive', onPress: () => sair(alias, session) },
      ]);
    } else if (a.kind === 'release') {
      const { alias, owner } = a.pair;
      Alert.alert(m.pair_release({ dono: owner }), m.pair_full_confirm({ sessao: s.name, dono: owner }), [
        { text: m.comum_cancelar(), style: 'cancel' },
        { text: m.pair_release({ dono: owner }), onPress: () => {
          setPairAccess(s.name, alias, 'full', server).catch((e: unknown) => toast.erro(mensagemDe(e)));
        } },
      ]);
    }
  }, []);

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
  const sections = useMemo<Section[]>(() => {
    if (home) return (['awaiting_input', 'working', 'idle'] as const).flatMap((state) => {
      const anchors = rows.filter((row) => !row.outside && row.state === state);
      if (!anchors.length) return [];
      const visible = state === 'idle' && !readyExpanded ? anchors.slice(0, 1) : anchors;
      const data = visible.flatMap((anchor) => [anchor, ...rows.filter((row) => row.state !== 'dead' &&
        row.outside?.localServerId === anchor.serverId && row.outside.localSession === anchor.name)]);
      const label = state === 'awaiting_input' ? m.home_sessions_awaiting()
        : state === 'working' ? m.home_sessions_working() : m.home_sessions_ready();
      return [{ id: state, label, color: null, attention: true, total: anchors.length, collapsed: false, data }];
    });
    const visiveis = busca
      ? rows.filter((r) => r.name.toLowerCase().includes(busca) || (r.cwd ?? '').toLowerCase().includes(busca))
      : rows;
    // A sessão de fora não pede nada a esta pessoa: fica sob a sessão do par, nunca em "Precisa de você".
    const { attention, rest } = splitAttention(visiveis.filter((r) => !r.outside));
    const out: Section[] = [];
    if (attention.length) {
      out.push({ id: 'attention', label: m.native_sidebar_awaiting(), color: null, attention: true, total: attention.length, collapsed: false, data: attention });
    }
    for (const g of placeOutside(agruparSessoes(rest, modo), visiveis.filter((r) => r.outside), modo, rows)) {
      if (!g.sessions.length) continue;
      const collapsed = modo !== 'none' && recolhidos.has(`${modo}:${g.id}`);
      out.push({ id: g.id, label: g.label, color: g.color, attention: false, total: g.sessions.length, collapsed, data: collapsed ? [] : g.sessions });
    }
    return out;
  }, [rows, busca, modo, recolhidos, home, readyExpanded]);
  const variosServidores = (home ? todos.length : servers.length) > 1;

  const alternarGrupo = useCallback(
    (id: string) =>
      setRecolhidos((atual) => {
        const novo = new Set(atual);
        const chave = `${modo}:${id}`;
        if (novo.has(chave)) novo.delete(chave);
        else novo.add(chave);
        return novo;
      }),
    [modo],
  );

  // "Todas as sessões" desfaz tudo que esconde linha: filtro e grupos recolhidos.
  const mostrarTodas = useCallback(() => {
    setFiltro('');
    setRecolhidos(new Set());
  }, []);

  const abrirConfig = useCallback(() => {
    onClose();
    router.push('/config' as never);
  }, [router, onClose]);

  const ativo = servers.find((s) => s.id === activeId) ?? null;
  const baldeAtivo = byServer.find((b) => b.server.id === activeId);
  const activeError = baldeAtivo?.error && baldeAtivo.error !== 'offline' ? baldeAtivo.error : null;
  const corAtivo = baldeAtivo?.error ? theme.tokens.status.error : baldeAtivo?.loaded ? theme.tokens.status.success : theme.tokens.text.muted;
  const rotuloAtivo = ativo
    ? baldeAtivo?.error
      ? baldeAtivo.error === 'offline' ? m.lista_servidor_offline({ label: ativo.label }) : `${ativo.label}, ${baldeAtivo.error}`
      : baldeAtivo?.loaded
        ? `${ativo.label}, ${m.native_connected()}`
        : ativo.label
    : '';

  const topo = (
    <View style={styles.topo}>
      <HangarMark size={20} color={theme.tokens.text.primary} />
      <Text style={[styles.marca, { color: theme.tokens.text.primary }]} numberOfLines={1} accessibilityRole="header">
        {m.native_brand()}
      </Text>
      {/* Agrupar não se aplica à lista de conversas, que é só por recência. */}
      <View style={styles.topoAcoes}>
        {ativo ? <LiveVoiceChatButton serverId={ativo.id} /> : null}
        {conversas ? null : <MenuView
          onPressAction={({ nativeEvent }) => {
            if (ehGroupBy(nativeEvent.event)) useAparencia.getState().setAgrupar(nativeEvent.event);
          }}
          actions={(['server', 'project', 'none'] as GroupBy[]).map((g) => ({
            id: g,
            title: ROTULO_AGRUPAR[g](),
            state: g === agrupar ? ('on' as const) : ('off' as const),
          }))}
        >
          <View style={styles.icone} accessible accessibilityRole="button" accessibilityLabel={m.lista_agrupar()} accessibilityValue={{ text: ROTULO_AGRUPAR[agrupar]() }}>
            <Icon name="ListFilter" size={20} color={theme.tokens.text.secondary} />
          </View>
        </MenuView>}
      </View>
    </View>
  );

  // Campo sempre à vista, como Claude/ChatGPT: buscar é o primeiro gesto de quem tem muitas sessões.
  const campoBusca = (
    <View style={[styles.campo, { backgroundColor: superficie(theme, 0.6) }]}>
      <Icon name="Search" size={16} color={theme.tokens.text.muted} />
      <TextInput
        value={filtro}
        onChangeText={setFiltro}
        placeholder={m.lista_buscar()}
        placeholderTextColor={theme.tokens.text.muted}
        returnKeyType="search"
        autoCapitalize="none"
        autoCorrect={false}
        accessibilityLabel={m.lista_filtrar()}
        style={[styles.input, { color: theme.tokens.text.primary }]}
      />
      {filtro ? (
        <Pressable onPress={() => setFiltro('')} hitSlop={10} accessibilityRole="button" accessibilityLabel={m.lista_filtro_limpar()}>
          <Icon name="X" size={16} color={theme.tokens.text.muted} />
        </Pressable>
      ) : null}
    </View>
  );

  // `atual`: a gaveta só existe sobre a tela inicial, então "Nova conversa" é sempre o lugar atual.
  const itemNav = (icon: 'SquarePen' | 'List', label: string, onPress: () => void, contagem?: number, atual = false) => (
    <Pressable
      onPress={onPress}
      style={({ pressed }) => [styles.nav, (pressed || atual) && { backgroundColor: superficie(theme, atual ? 0.8 : 0.6) }]}
      accessibilityRole="button"
      accessibilityState={atual ? { selected: true } : undefined}
      accessibilityLabel={contagem === undefined ? label : `${label}, ${contagem}`}
    >
      <View style={styles.navIcone}>
        <Icon name={icon} size={20} color={atual ? theme.tokens.text.primary : theme.tokens.text.secondary} />
      </View>
      <Text style={[styles.navTxt, atual && styles.navAtual, { color: theme.tokens.text.primary }]} numberOfLines={1}>{label}</Text>
      {contagem === undefined ? null : <Text style={[styles.navConta, { color: theme.tokens.text.muted }]}>{contagem}</Text>}
    </Pressable>
  );

  const rodape = (
    <View style={[styles.rodape, { borderTopColor: theme.tokens.border.subtle }]}>
      {ativo ? (
        <Pressable onPress={onOpenServers} style={styles.ativo} accessibilityRole="button" accessibilityLabel={rotuloAtivo}>
          <View style={[styles.ponto, { backgroundColor: corAtivo }]} />
          <Text style={[styles.ativoTxt, { color: theme.tokens.text.secondary }]} numberOfLines={activeError ? undefined : 1}>{activeError ? `${ativo.label}: ${activeError}` : ativo.label}</Text>
        </Pressable>
      ) : <View style={styles.ativo} />}
      <Pressable onPress={abrirConfig} style={styles.icone} accessibilityRole="button" accessibilityLabel={m.config_modal_titulo()}>
        <Icon name="Settings" size={20} color={theme.tokens.text.secondary} />
      </Pressable>
    </View>
  );

  // `noTopo`: com a busca aberta o teclado cobre a metade de baixo, e o aviso centrado sumia atrás dele.
  const vazio = (titulo: string | null, texto: string | null, noTopo = false, girando = false) => (
    <View style={[styles.empty, noTopo && styles.emptyTopo]}>
      {girando ? <ActivityIndicator /> : null}
      {titulo ? <Text style={styles.emptyTitle}>{titulo}</Text> : null}
      {texto ? <Text style={styles.emptyTxt}>{texto}</Text> : null}
    </View>
  );

  const machineErrors = byServer.filter((bucket) => bucket.error);
  const machineStatus = home ? byServer.filter((bucket) => bucket.error || !bucket.loaded).map((bucket) => (
    <View key={bucket.server.id} style={styles.machineStatus}>
      {bucket.error ? (
        <Text style={styles.machineError} accessibilityRole="alert" selectable>
          {bucket.error === 'offline' ? m.lista_servidor_offline({ label: bucket.server.label }) : `${bucket.server.label}: ${bucket.error}`}
        </Text>
      ) : <Text style={styles.emptyTxt}>{`${bucket.server.label}: ${m.lista_carregando()}`}</Text>}
    </View>
  )) : null;

  let corpo: ReactNode;
  if (!ready) corpo = vazio(null, m.comum_carregando(), false, true);
  else if ((home ? todos.length : servers.length) === 0) corpo = vazio(m.lista_nenhum_servidor(), m.lista_pareie_qr());
  else if (conversas) {
    corpo = <ConversationList header={itemNav('SquarePen', m.native_new_chat_title(), onNewConversation, undefined, true)} onClose={onClose} />;
  } else if (loading && (home ? sections.length === 0 : rows.length === 0)) corpo = vazio(null, m.lista_carregando(), false, true);
  else {
    corpo = (
      <SectionList
        sections={sections}
        keyExtractor={(item) => `${item.serverId}::${item.name}`}
        // Sem cabeçalho fixo: o painel é vidro, e um cabeçalho fixo transparente deixaria as linhas
        // passarem por baixo do texto dele.
        stickySectionHeadersEnabled={false}
        ListHeaderComponent={home ? null : (
          <View style={styles.navs}>
            {itemNav('SquarePen', m.native_new_chat_title(), onNewConversation, undefined, true)}
            {itemNav('List', m.native_sidebar_all_sessions(), mostrarTodas, rows.length)}
          </View>
        )}
        renderSectionHeader={({ section }) => {
          if (!section.label) return null;
          const cor = home ? theme.tokens.pill[section.id === 'awaiting_input' ? 'input' : section.id === 'working' ? 'working' : 'idle'].fg
            : section.attention ? theme.tokens.pill.input.fg : (section.color ?? theme.tokens.text.muted);
          const conteudo = (
            <>
              {section.attention ? null : (
                <Icon name={section.collapsed ? 'ChevronRight' : 'ChevronDown'} size={12} color={theme.tokens.text.muted} />
              )}
              <View style={[styles.ponto, { backgroundColor: cor }]} />
              <Text style={[styles.grupoTxt, { color: section.attention ? cor : theme.tokens.text.secondary }]} numberOfLines={1}>{section.label}</Text>
              <Text style={[styles.grupoConta, { color: theme.tokens.text.muted }]}>{section.total}</Text>
            </>
          );
          return section.attention ? (
            <View style={styles.grupo} accessible accessibilityRole="header" accessibilityLabel={`${section.label}, ${section.total}`}>
              {conteudo}
            </View>
          ) : (
            <Pressable
              onPress={() => alternarGrupo(section.id)}
              style={styles.grupo}
              accessibilityRole="button"
              accessibilityLabel={`${section.label}, ${section.total} ${m.lista_sessoes_plural()}`}
              accessibilityState={{ expanded: !section.collapsed }}
              accessibilityHint={section.collapsed ? m.sessao_expandir() : m.sessao_recolher()}
            >
              {conteudo}
            </Pressable>
          );
        }}
        renderSectionFooter={({ section }) => home && section.id === 'idle' && section.total > 1 ? (
          <Pressable onPress={() => setReadyExpanded((value) => !value)} style={styles.moreReady}
            accessibilityRole="button" accessibilityState={{ expanded: readyExpanded }}>
            <Text style={styles.moreReadyTxt}>{readyExpanded ? m.sessao_recolher() : m.home_sessions_more_ready({ n: section.total - 1 })}</Text>
          </Pressable>
        ) : null}
        renderItem={({ item, section }) => (
          <SessionRow
            session={item}
            compact={home}
            mostrarServidor={variosServidores && (home || section.attention || modo !== 'server')}
            onPress={abrir}
            onGit={abrirGit}
            onExcluir={excluir}
            onRenomear={setRenomeando}
            onResume={retomar}
            onWorktree={proprios.has(item.serverId) ? abrirWorktree : undefined}
            onPair={proprios.has(item.serverId) || item.outside ? fazerPar : undefined}
          />
        )}
        ListEmptyComponent={home && machineErrors.length ? null
          : busca ? vazio(m.lista_vazia_filtro(), null, true) : vazio(m.lista_nenhuma_ativa(), null)}
        contentContainerStyle={styles.listContent}
        keyboardShouldPersistTaps="handled"
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={onRefresh} />}
      />
    );
  }

  return (
    <>
      {/* Sem caixa: a lista fica direto sobre o fundo escolhido em Aparência, como no app de PC. */}
      {/* A gaveta passa por baixo da barra de status e do indicador de início: a margem segura é daqui. */}
      <View style={[styles.painel, !home && { paddingTop: insets.top + 8, paddingBottom: Math.max(insets.bottom, 8) }]}>
        {home ? machineStatus : topo}
        {home || conversas ? null : campoBusca}
        <View style={styles.corpo}>{corpo}</View>
        {home ? null : rodape}
      </View>
      <RenameSheet nome={renomeando?.name ?? null} onConfirmar={renomear} onFechar={() => setRenomeando(null)} />
      <WorktreeSheet server={wt?.server ?? null} path={wt?.path ?? null} onClose={() => setWt(null)} />
      <PairInviteSheet target={convite} onClose={() => setConvite(null)} />
      <PairAcceptSheet target={aceite} onClose={() => setAceite(null)} />
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  painel: { flex: 1, paddingHorizontal: theme.base.space[2] },
  machineStatus: { paddingHorizontal: 12, paddingVertical: 6 },
  machineError: { color: theme.tokens.status.error, fontSize: theme.base.text.sm },
  moreReady: { minHeight: 44, justifyContent: 'center', paddingLeft: 46, paddingRight: 12 },
  moreReadyTxt: { color: theme.tokens.accent.base, fontSize: theme.base.text.sm },
  // Tudo alinha na mesma coluna de 12 pt das linhas: marca, ícones da navegação e marcas de estado.
  topo: { flexDirection: 'row', alignItems: 'center', gap: 10, minHeight: 48, paddingLeft: 13 },
  // encolhe a marca, não os botões: com texto ampliado o nome empurrava as ações para fora
  marca: { flexShrink: 1, fontSize: theme.base.text.xl, fontWeight: '700' },
  topoAcoes: { flexDirection: 'row', alignItems: 'center', marginLeft: 'auto' },
  icone: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
  campo: { flexDirection: 'row', alignItems: 'center', gap: 8, marginTop: 4, marginBottom: 8, paddingHorizontal: 12, borderRadius: theme.base.radius.lg, minHeight: 40 },
  input: { flex: 1, fontSize: theme.base.text.base, paddingVertical: 8 },
  corpo: { flex: 1 },
  listContent: { flexGrow: 1, paddingBottom: theme.base.space[2] },
  navs: { paddingBottom: 8, gap: 2 },
  nav: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: 48, paddingHorizontal: 12, borderRadius: theme.base.radius.lg },
  navIcone: { width: 22, alignItems: 'center' },
  navTxt: { flexShrink: 1, fontSize: theme.base.text.base },
  navAtual: { fontWeight: '600' },
  navConta: { marginLeft: 'auto', fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'] },
  grupo: { flexDirection: 'row', alignItems: 'center', gap: 6, minHeight: 36, paddingTop: 12, paddingBottom: 4, paddingHorizontal: 12, backgroundColor: 'transparent' },
  grupoTxt: { flexShrink: 1, fontSize: theme.base.text.xs, fontWeight: '600' },
  grupoConta: { marginLeft: 'auto', fontSize: theme.base.text.xs, fontVariant: ['tabular-nums'] },
  ponto: { width: 7, height: 7, borderRadius: 4 },
  rodape: { flexDirection: 'row', alignItems: 'center', gap: 8, paddingTop: 4, paddingLeft: 12, borderTopWidth: StyleSheet.hairlineWidth },
  ativo: { flex: 1, flexDirection: 'row', alignItems: 'center', gap: 8, minWidth: 0, minHeight: 44 },
  ativoTxt: { flexShrink: 1, fontSize: theme.base.text.sm },
  empty: { flex: 1, justifyContent: 'center', alignItems: 'center', padding: theme.base.space[6], gap: 8 },
  emptyTopo: { flex: 0, justifyContent: 'flex-start', paddingTop: theme.base.space[4] },
  emptyTitle: { fontSize: theme.base.text.lg, fontWeight: '600', color: theme.tokens.text.primary, textAlign: 'center' },
  emptyTxt: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center' },
}));
