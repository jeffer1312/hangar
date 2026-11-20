import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Pressable, Text, View, useWindowDimensions, type NativeScrollEvent, type NativeSyntheticEvent } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { LegendList, type LegendListRef } from '@legendapp/list/react-native';
import Animated, { FadeIn, useReducedMotion } from 'react-native-reanimated';
import { arrivedCount } from './arrivals';
import { Icon } from '../ui/Icon';
import { superficie } from '../theme/superficie';
import { UserBubble } from './UserBubble';
import { QueuedBubble } from './QueuedBubble';
import { AssistantBubble } from './AssistantBubble';
import { OrqTimelineEntry } from './OrqTimelineEntry';
import { ClaudeNotice } from './ClaudeNotice';
import { isNoticeLine } from '../features/notices/notices';
import { PreviewBubble } from './PreviewBubble';
import { ToolGroup } from './tools/ToolGroup';
import { ToolCard } from './tools/ToolCard';
import { AgentesRodando, PensamentoVivo, WorkingLine } from './LiveWork';
import type { AgentSubagents } from '../features/activity/useAgentSubagents';
import { projectConversation, type ConversationProjection, type MessageRow } from './conversationProjection';
import { conversationInset } from './conversationColumn';
import { ToolDetailSheet, type ToolDetailHandle } from './tools/ToolDetailSheet';
import type { ConversationRow } from './tools/fold';
import { HtmlPageCard } from './tools/HtmlPageCard';
import { locateInMarkdown, type QuoteRef } from '@hangar/core';
import { useCiteDialog } from '../stores/citeDialog';
import { contextRefsKey } from '../stores/contextRefs';
import { flashQuoteSource } from '../stores/quoteFlash';
import { echoInQueuePanel, hexParaRgb, planDisplayText, type AgentRun, type ChatEvent, type QueuePanel, type SessionInfo, type StateEvent } from '@hangar/core';
import { useAparencia } from '../stores/aparencia';
import { TaskList } from './TaskList';
import { sameMessage, type PendingMsg } from './pending';
import { FlightDestination, flightRoute } from './SendFlight';
import * as m from '../paraglide/messages';

// Lista de bolhas do chat. A janela de render da PWA (WINDOW=120 eventos montados) aqui é
// a virtualização nativa do LegendList: ele só monta o visível + buffer, então o store
// pode manter os events completos e a lista continua barata — mesmo objetivo, menos código.
//
// onStartReached dispara loadOlder (busca o histórico anterior sob demanda); os avisos
// olderFailed ficam no rodapé da lista (réguas: falha não some calada).

interface Props {
  events: ChatEvent[];
  eventsRevision?: number;
  preview: string;
  previewMd?: boolean; // prévia do agente = markdown (default true = comportamento antigo)
  previewFull?: boolean;
  session?: Pick<SessionInfo, 'provider' | 'headless' | 'plugin_tool_servers'> | null;
  olderFailed: '' | 'failed' | 'unjoinable';
  onLoadOlder: () => void;
  pending?: PendingMsg[];
  optionsSlot?: ReactNode;
  sessionName?: string;
  serverId?: string;
  // Altura da caixa que flutua por cima do fim da lista: o último item rola até ficar acima dela.
  bottomInset?: number;
  alignItemsAtEnd?: boolean;
  preserveToolGroupPosition?: boolean;
  nestedScrollEnabled?: boolean;
  maxReading?: number;
  // Bloco "trabalhando" do fim (ausente = lista só de leitura, como a do subagente).
  stateEvent?: StateEvent | null;
  turnSeen?: number | null;
  pensamento?: string;
  ferramenta?: { nome: string; input: Record<string, unknown> } | null;
  // Subagentes rodando de verdade, pelo fold de atividade: o cartão deles sai do meio da conversa
  // e fica grudado no fim até acabarem.
  agentesRodando?: AgentRun[];
  agentSubagents?: AgentSubagents;
  onAbrirAgentes?: () => void;
  // Mensagens que esperam a vez no painel "Na fila": a bolha delas sai da conversa até a entrega.
  queuePanel?: QueuePanel;
  // A tela tem campo de mensagem: só então o menu da seleção oferece "Citar" (sem campo, a citação
  // iria para uma bandeja que ninguém envia).
  canQuote?: boolean;
}

const SEM_AGENTES: AgentRun[] = [];

const keyOf = (row: ConversationRow) => row.id;

export function MessageList({
  events,
  eventsRevision,
  preview,
  previewMd = true,
  previewFull = false,
  session,
  olderFailed,
  onLoadOlder,
  pending = [],
  optionsSlot,
  sessionName,
  serverId,
  bottomInset = 0,
  alignItemsAtEnd = true,
  preserveToolGroupPosition = false,
  nestedScrollEnabled,
  maxReading,
  stateEvent = null,
  turnSeen = null,
  pensamento = '',
  ferramenta = null,
  agentesRodando = SEM_AGENTES,
  agentSubagents,
  onAbrirAgentes,
  queuePanel,
  canQuote = false,
}: Props) {
  const codex = session?.provider === 'codex';
  const visiblePreview = codex ? planDisplayText(preview) : preview;
  const detail = useRef<ToolDetailHandle>(null);
  const pref = useAparencia((s) => s.pensamentoTools);
  const look = useAparencia((s) => s.ferramentas);
  // Terminal e Claude Desktop agrupam toda sequência de chamadas e guardam o grupo aberto.
  const everyRun = look === 'terminal' || look === 'claudeDesktop';
  const preserveExpansion = preserveToolGroupPosition || everyRun;
  const tarefasLigadas = useAparencia((s) => s.tarefas);
  const folha = useAparencia((s) => (s.leitura === 'sheet' ? s.solidezFolha : null));
  const { theme } = useUnistyles();
  const { width } = useWindowDimensions();
  const [boxWidth, setBoxWidth] = useState<number | null>(null);
  const previous = useRef<ConversationProjection | null>(null);
  // O eco preso a uma entrada da fila é a mensagem na tela: a bolha `queued-` dela fica fora, como a do painel.
  const hiddenQueue = useMemo(() => {
    const held = pending.flatMap((p) => (p.entry ? [{ id: p.entry.slice('queued-'.length), text: p.text, ts: null }] : []));
    return held.length ? { entries: [...(queuePanel?.entries ?? []), ...held], actions: queuePanel?.actions ?? false } : queuePanel;
  }, [pending, queuePanel]);
  const projection = projectConversation(previous.current, {
    events, eventsRevision, pref, merge: look === 'tree', everyRun, tasksEnabled: tarefasLigadas,
    pagesEnabled: !!sessionName, running: new Set(agentesRodando.map((a) => a.id)), queuePanel: hiddenQueue,
  });
  // Render descartado ou repetido não publica o seu cache nem modifica o snapshot anterior.
  useLayoutEffect(() => { previous.current = projection; }, [projection]);
  const data = projection.data;
  // Linha que chega com a conversa aberta entra esmaecendo, uma vez; a resposta que toma o lugar da
  // prévia continua o mesmo texto e aparece direto. Quem entrou fica com o invólucro: tirá-lo
  // remontaria a bolha.
  const reducedMotion = useReducedMotion();
  const arrivals = useRef<{ keys: string[] | null; at: Map<string, number>; preview: number; sent: string[] }>({ keys: null, at: new Map(), preview: 0, sent: [] });
  const keys = useMemo(() => data.map(keyOf), [data]);
  if (visiblePreview) arrivals.current.preview = Date.now();
  if (arrivals.current.keys !== keys) {
    const now = Date.now();
    for (const row of data.slice(data.length - arrivedCount(arrivals.current.keys, keys))) {
      // A mensagem que toma o lugar do eco dela já estava na tela, como a resposta que toma o lugar da prévia.
      const heir = row.type === 'event' && (row.ev.kind === 'assistant_msg' ? now - arrivals.current.preview < 500
        : row.ev.kind === 'user_msg' && arrivals.current.sent.some((text) => sameMessage(text, row.ev.text ?? '')));
      if (!heir) arrivals.current.at.set(row.id, now);
    }
    arrivals.current.keys = keys;
  }
  arrivals.current.sent = pending.flatMap((p) => (p.queue ? [] : [p.text]));
  const route = sessionName && serverId ? flightRoute(serverId, sessionName) : null;
  const results = projection.results;
  const resultDe = useCallback((t: ChatEvent) => results.get(t.tool_use_id ?? '') ?? null, [results]);
  const starts = projection.starts;
  const inicioDoAgente = useCallback((id: string) => starts.get(id) ?? null, [starts]);
  const abrirDetalhe = useCallback((ev: ChatEvent) => detail.current?.abrir(ev), []);
  // Toque numa citação: rola até a resposta de origem pelo índice da linha (a lista é virtualizada,
  // então não há nó para procurar). Origem fora do histórico carregado: nada acontece.
  const lista = useRef<LegendListRef>(null);
  const reading = useRef({ y: 0, atEnd: true });
  const [toolGroupPaused, setToolGroupPaused] = useState(false);
  const toolGroupPausedRef = useRef(false);
  const toolGroupMeasurement = useRef<{ before?: number; height?: number }>({});
  const pauseForToolGroup = useCallback(() => {
    reading.current.atEnd = false;
    toolGroupPausedRef.current = true;
    toolGroupMeasurement.current = { before: lista.current?.getState().contentLength };
    setToolGroupPaused(true);
  }, []);
  const resumeFollowing = useCallback(() => {
    reading.current.atEnd = true;
    toolGroupPausedRef.current = false;
    setToolGroupPaused(false);
  }, []);
  // Com o dedo na lista ou o impulso andando, nada a leva ao fim: a posição lida chega com atraso
  // e, logo depois de abrir a conversa, ainda diz "no fim".
  const dragging = useRef(false);
  const dataRef = useRef(data);
  useEffect(() => { dataRef.current = data; }, [data]);
  // Medidas lidas na hora do toque, por referência: como dependência, cada mudança de altura do
  // campo de mensagem recriava o `goToQuote` e, com ele, o `renderItem` de todas as linhas.
  const layoutRef = useRef({ quoteGap: theme.base.space[3], bottomInset, animated: look !== 'terminal' });
  useEffect(() => { layoutRef.current = { quoteGap: theme.base.space[3], bottomInset, animated: look !== 'terminal' }; }, [theme.base.space, bottomInset, look]);
  const goToQuote = useCallback((ref: QuoteRef) => {
    const rows = dataRef.current;
    const { quoteGap } = layoutRef.current;
    const i = ref.source ? rows.findIndex((r) => r.type === 'event' && r.ev.id === ref.source) : -1;
    const row = rows[i];
    if (i < 0 || row?.type !== 'event' || !lista.current) return;
    const source = row.ev.text ?? '';
    flashQuoteSource(ref.source);
    // Começo da resposta no topo (viewPosition 0): com 0.3, resposta mais alta que a tela ficava
    // com o começo, e o trecho, acima da vista.
    void lista.current.scrollToIndex({ index: i, animated: false, viewPosition: 0, viewOffset: quoteGap }).then(() => {
      // Resposta mais alta que a tela: desce até onde o trecho está. Sem o layout do texto, a
      // posição sai da proporção do trecho na fonte, o que basta para deixá-lo à vista.
      setTimeout(() => {
        const list = lista.current;
        if (!list) return;
        const st = list.getState();
        const size = st.sizeAtIndex(i);
        // A lista passa por baixo do campo de mensagem: o que se vê é a altura menos essa faixa.
        // Com a altura inteira, o trecho no fim de uma resposta alta parava atrás do campo.
        const visible = st.scrollLength - layoutRef.current.bottomInset;
        if (!(size > visible * 0.6)) return;
        const at = locateInMarkdown(source, ref.text, ref.before, ref.after);
        if (!at || !source.length) return;
        const top = st.scroll + quoteGap;
        const target = top + size * (at.start / source.length) - visible * 0.25;
        const offset = Math.min(Math.max(target, st.scroll), top + size - visible);
        if (offset > st.scroll + 1) void list.scrollToOffset({ offset, animated: layoutRef.current.animated });
      }, 80);
    });
  }, []);
  const working = stateEvent?.state === 'working';
  const desde = !working ? null : projection.lastSent === null ? turnSeen
    : turnSeen === null ? projection.lastSent : Math.max(projection.lastSent, turnSeen);
  const ferramentaEv = useMemo<ChatEvent | null>(
    () => (ferramenta ? { kind: 'tool_use', id: 'ferramenta-viva', tool_name: ferramenta.nome, tool_input: ferramenta.input } : null),
    [ferramenta],
  );

  const renderRow = useCallback((item: MessageRow) => {
    switch (item.type) {
      case 'fold':
        return <ToolGroup parts={item.parts} summary={item.summary} resultOf={item.resultOf!} onAbrir={abrirDetalhe} onExpand={preserveExpansion ? pauseForToolGroup : undefined} look={look} pluginServers={session?.plugin_tool_servers ?? undefined} />;
      case 'tasks':
        return <TaskList tasks={item.tasks} />;
      case 'page':
        return <HtmlPageCard page={item.page} sessionName={sessionName ?? ''} serverId={serverId} />;
      case 'event': {
        const ev = item.ev;
        if (ev.kind === 'user_msg') {
          if (ev.id.startsWith('queued-') || ev.id.startsWith('held:') || ev.desistiu || ev.hook_error != null) {
            return <QueuedBubble event={ev} working={working} problem={stateEvent?.problema}
              provider={session?.provider} headless={session?.headless} sessionName={sessionName} serverId={serverId} onQuoteClick={goToQuote} />;
          }
          return <UserBubble text={ev.text ?? ''} sessionName={sessionName} serverId={serverId} ts={ev.ts} eventId={ev.id} imageCount={ev.image_count ?? 0} onQuoteClick={goToQuote} />;
        }
        if (isNoticeLine(ev) && (ev.kind === 'assistant_msg' || ev.kind === 'notice')) return <ClaudeNotice event={ev} serverId={serverId} />;
        if (ev.kind === 'assistant_msg') {
          return <AssistantBubble text={ev.text ?? ''} sessionName={sessionName} serverId={serverId} ts={ev.ts} eventId={ev.id} canQuote={canQuote} />;
        }
        if (ev.kind === 'notice') {
          if (ev.orq) return <OrqTimelineEntry event={ev} serverId={serverId} sessionName={sessionName} dayTs={item.dayTs} />;
          // Código conhecido vira frase do idioma da tela; desconhecido mostra o que veio, pra um
          // aviso novo do harness não sumir calado.
          const conhecido = ev.text === 'interrupted' || ev.text === 'turn_aborted';
          const frase = conhecido ? m.notice_interrupted()
            : ev.text === 'compacted' ? m.notice_compacted()
            : ev.text === 'hook_prompt' || ev.text === 'hook_failed' ? [ev.text === 'hook_failed' ? m.notice_hook_failed() : m.notice_hook_prompt(), ev.hook_error].filter(Boolean).join('\n')
            // ponytail: só a linha; abrir o SKILL.md no app nativo fica pra quando pedirem.
            : ev.text === 'skill_loaded' && ev.skill ? m.notice_skill_loaded({ name: ev.skill.name })
            : ev.text;
          return <NoticeLine text={frase ?? ''} />;
        }
        return null;
      }
      default: {
        const nunca: never = item;
        return nunca;
      }
    }
  }, [abrirDetalhe, sessionName, serverId, look, working, session?.provider, session?.headless, session?.plugin_tool_servers, stateEvent?.problema, goToQuote, canQuote, preserveExpansion, pauseForToolGroup]);
  const renderItem = useCallback(({ item }: { item: MessageRow }) => {
    const row = renderRow(item);
    const arrived = arrivals.current.at.get(item.id);
    if (arrived === undefined || !row) return row;
    const fresh = !reducedMotion && look !== 'terminal' && Date.now() - arrived < 1000;
    return <Animated.View entering={fresh ? FadeIn.duration(220) : undefined}>{row}</Animated.View>;
  }, [renderRow, reducedMotion, look]);

  // "Ir pro fim" (como o web): aparece com mais de uma tela entre o que se vê e o fim. Mensagem que
  // chega nesse meio-tempo muda o botão pra aviso, senão a conversa anda sem a pessoa saber.
  const [longeDoFim, setLongeDoFim] = useState(false);
  const vistoAte = useRef(events.length);
  useEffect(() => {
    if (!longeDoFim) vistoAte.current = events.length;
  }, [longeDoFim, events.length]);
  const temNovas = longeDoFim && events.length > vistoAte.current;
  const onScroll = useCallback((e: NativeSyntheticEvent<NativeScrollEvent>) => {
    const { contentOffset, contentSize, layoutMeasurement } = e.nativeEvent;
    let y = contentOffset.y;
    let height = contentSize.height;
    let viewport = layoutMeasurement.height;
    if (toolGroupPausedRef.current) {
      const state = lista.current?.getState();
      if (!state || !Number.isFinite(state.contentLength) || !(state.contentLength > 0)
        || !Number.isFinite(state.scroll) || !Number.isFinite(state.scrollLength) || !(state.scrollLength > 0)) return;
      // O evento pode ter sido retido antes da expansão, até antes da nova medida.
      const measurement = toolGroupMeasurement.current;
      if (!(Number.isFinite(measurement.before) && state.contentLength !== measurement.before)
        && state.contentLength !== measurement.height) return;
      y = state.scroll;
      height = state.contentLength;
      viewport = state.scrollLength;
    }
    if (!Number.isFinite(y) || !Number.isFinite(height) || !(viewport > 0)) return;
    const falta = height - y - viewport;
    reading.current = { y, atEnd: falta < 64 };
    if (reading.current.atEnd && toolGroupPausedRef.current) resumeFollowing();
    setLongeDoFim(falta > viewport);
  }, [resumeFollowing]);
  const onScrollBeginDrag = useCallback(() => {
    dragging.current = true;
    reading.current.atEnd = false;
  }, []);
  const onMomentumScrollBegin = useCallback(() => { dragging.current = true; }, []);
  const settleScroll = useCallback((e: NativeSyntheticEvent<NativeScrollEvent>) => {
    dragging.current = false;
    onScroll(e);
  }, [onScroll]);
  const onContentSizeChange = useCallback((_width: number, height: number) => {
    if (!toolGroupPausedRef.current || !Number.isFinite(height) || !(height > 0)) return;
    toolGroupMeasurement.current.height = height;
    const state = lista.current?.getState();
    if (!state || !Number.isFinite(state.scroll) || !Number.isFinite(state.scrollLength) || !(state.scrollLength > 0)) return;
    // O sinal de proximidade da lista ainda pode ser o de antes da expansão.
    const remaining = height - state.scroll - state.scrollLength;
    reading.current = { y: state.scroll, atEnd: remaining < 64 };
    setLongeDoFim(remaining > state.scrollLength);
    if (reading.current.atEnd) resumeFollowing();
  }, [resumeFollowing]);
  // A caixa de baixo muda de altura (a bandeja some no envio, o campo encolhe) e a bolha enviada
  // entra no rodapé: quem estava no fim continua no fim. Sem isto, com o teclado aberto, a bolha e
  // a resposta ficavam atrás do campo. De novo depois da medida da bolha formatada. Linha nova e
  // prévia que aparece também: com o teclado aberto, a resposta chegava atrás do campo.
  const pendingCount = pending.length;
  const rowCount = data.length;
  const hasPreview = !!visiblePreview;
  useEffect(() => {
    if (!reading.current.atEnd || toolGroupPausedRef.current || dragging.current) return;
    const keepAtEnd = () => {
      if (reading.current.atEnd && !toolGroupPausedRef.current && !dragging.current) void lista.current?.scrollToEnd({ animated: false });
    };
    const frame = requestAnimationFrame(keepAtEnd);
    const later = setTimeout(keepAtEnd, 250);
    return () => { cancelAnimationFrame(frame); clearTimeout(later); };
  }, [bottomInset, pendingCount, rowCount, hasPreview]);
  // Modal "Responder ao trecho" desta conversa: guarda o ponto ao abrir (o teclado e a bandeja mudam
  // a altura da lista enquanto ele está aberto) e volta a ele ao fechar; "Finalizar" vai ao fim.
  useEffect(() => {
    if (!sessionName) return;
    const ownKey = contextRefsKey(serverId, sessionName);
    let saved: { y: number; atEnd: boolean } | null = null;
    return useCiteDialog.subscribe((s, prev) => {
      if (s.request?.draftKey === ownKey && !prev.request) { saved = { ...reading.current }; return; }
      if (s.request || prev.request?.draftKey !== ownKey || !saved) return;
      const point = saved;
      saved = null;
      // Sem animação também no "Finalizar": o campo ganha foco logo depois e o teclado encolhe a
      // lista; com a rolagem ainda andando, a lista não estava no fim e a última mensagem ficava
      // atrás do campo de mensagem.
      const restore = () => {
        if (s.finished || point.atEnd) void lista.current?.scrollToEnd({ animated: false });
        else void lista.current?.scrollToOffset({ offset: point.y, animated: false });
      };
      requestAnimationFrame(restore);
      // De novo depois que o teclado baixa e o campo de mensagem é medido com a bandeja nova.
      setTimeout(restore, 350);
    });
  }, [serverId, sessionName]);
  const irProFim = useCallback(() => {
    if (toolGroupPausedRef.current) resumeFollowing();
    setLongeDoFim(false);
    void lista.current?.scrollToEnd({ animated: look !== 'terminal' });
  }, [resumeFollowing, look]);

  // Largura da coluna (Aparência › Texto da conversa): abaixo de 100% a conversa estreita no meio.
  const recuo = conversationInset(boxWidth ?? width, theme.conversa.coluna, theme.base.space[4], maxReading);
  const elevado = hexParaRgb(theme.tokens.bg.elevated);

  return (
    <View style={styles.area} onLayout={(e) => setBoxWidth(e.nativeEvent.layout.width)}>
    {/* Leitura Folha: a folha atrás da conversa; a Solidez da folha diz quanto ela tapa o fundo. */}
    {folha !== null && elevado ? (
      <View
        pointerEvents="none"
        style={[styles.folha, { backgroundColor: `rgba(${elevado.join(',')},${folha})`, left: recuo + theme.base.space[2], right: recuo + theme.base.space[2] }]}
      />
    ) : null}
    <LegendList
      ref={lista}
      onScroll={onScroll}
      onScrollBeginDrag={onScrollBeginDrag}
      onScrollEndDrag={settleScroll}
      onMomentumScrollBegin={onMomentumScrollBegin}
      onMomentumScrollEnd={settleScroll}
      onContentSizeChange={preserveExpansion ? onContentSizeChange : undefined}
      scrollEventThrottle={100}
      nestedScrollEnabled={nestedScrollEnabled}
      data={data}
      keyExtractor={keyOf}
      renderItem={renderItem}
      recycleItems={false}
      estimatedItemSize={72}
      alignItemsAtEnd={alignItemsAtEnd}
      initialScrollAtEnd
      maintainScrollAtEnd={!toolGroupPaused}
      maintainVisibleContentPosition
      keyboardShouldPersistTaps="handled"
      // Android: o toque longo que seleciona texto foca o TextView, e o ScrollView do RN rolava até
      // ele (requestChildFocus → scrollToChild); numa resposta mais alta que a tela a lista pulava
      // centenas de px e a seleção se estendia pelo que passou sob o dedo.
      scrollsChildToFocus={false}
      // Sem isto o teclado aberto não tinha como fechar no chat.
      keyboardDismissMode="on-drag"
      onStartReached={onLoadOlder}
      onStartReachedThreshold={1}
      contentContainerStyle={[styles.content, recuo > 0 && { paddingHorizontal: theme.base.space[4] + recuo }]}
      // O histórico antigo carrega pelo topo: o aviso da falha fica lá, não perto do composer.
      ListHeaderComponent={
        olderFailed === 'failed' ? (
          <Text style={styles.gap} onPress={onLoadOlder} accessibilityRole="button">
            {m.chat_historico_antigo()}
          </Text>
        ) : olderFailed === 'unjoinable' ? (
          <Text style={styles.gap}>{m.chat_sem_historico_anterior()}</Text>
        ) : null
      }
      ListFooterComponent={
        <View style={styles.footer}>
          {/* A mensagem enviada com a sessão parada é a que abre o turno: fica acima do trabalho dele, onde a
              mensagem real vai entrar, e não pula quando ela chega. */}
          {pending.filter((p) => p.solid && !p.queue).map((p) => (
            <PendingEcho key={p.id} echo={p} route={route} sessionName={sessionName} serverId={serverId} onQuoteClick={goToQuote} />
          ))}
          {/* Mesma ordem do fim da conversa na PWA: o trabalho em voo, depois o que a pessoa mandou. */}
          {pensamento ? <PensamentoVivo texto={pensamento} /> : null}
          {ferramentaEv ? <ToolCard use={ferramentaEv} result={null} onPress={abrirDetalhe} look={look} onExpand={preserveExpansion ? pauseForToolGroup : undefined} escrevendo /> : null}
          {visiblePreview ? <PreviewBubble text={visiblePreview} md={previewMd} full={previewFull} streaming={working} /> : null}
          {working && !pensamento && !ferramenta ? <WorkingLine label={stateEvent?.label} since={desde} /> : null}
          {/* Rótulo passageiro do servidor (some sozinho em segundos): não é mensagem da conversa. */}
          {!working && stateEvent?.turn_note ? <Text style={styles.turnNote}>{stateEvent.turn_note}</Text> : null}
          {agentesRodando.length || agentSubagents?.error ? <AgentesRodando agentes={agentesRodando} inicio={inicioDoAgente} onAbrir={onAbrirAgentes} serverId={serverId} sessionName={sessionName} events={events} eventsRevision={eventsRevision} subagentSnapshot={agentSubagents} /> : null}
          {/* O eco que o painel da fila já mostra some: a mesma mensagem não aparece duas vezes. O de envio com a
              sessão parada (acima) fica: a passagem dele pela fila não aparece no painel. */}
          {pending.filter((p) => !p.queue && !p.solid && !(queuePanel && echoInQueuePanel(p.text, queuePanel))).map((p) => (
            <PendingEcho key={p.id} echo={p} route={route} sessionName={sessionName} serverId={serverId} onQuoteClick={goToQuote} />
          ))}
          {optionsSlot ?? null}
          {bottomInset > 0 ? <View style={{ height: bottomInset }} /> : null}
        </View>
      }
      accessibilityLabel={m.msg_aria_mensagens()}
    />
    {longeDoFim ? (
      <Pressable
        onPress={irProFim}
        style={[styles.toBottom, { bottom: bottomInset + theme.base.space[3] }, temNovas && { borderColor: theme.tokens.accent.base }]}
        accessibilityRole="button"
        accessibilityLabel={temNovas ? m.msg_novas_abaixo() : m.msg_ir_ultima()}
      >
        <Icon name="ChevronDown" size={20} color={temNovas ? theme.tokens.accent.base : theme.tokens.text.secondary} />
        {temNovas ? <View style={[styles.novasDot, { backgroundColor: theme.tokens.accent.base, borderColor: theme.tokens.bg.base }]} /> : null}
      </Pressable>
    ) : null}
    <ToolDetailSheet ref={detail} resultOf={resultDe} sessionName={sessionName} serverId={serverId} />
    </View>
  );
}

/** O eco de um envio no fim da conversa: é nele que a cópia que saiu do campo pousa. */
function PendingEcho({ echo, route, sessionName, serverId, onQuoteClick }: {
  echo: PendingMsg; route: string | null; sessionName?: string; serverId?: string; onQuoteClick?: (ref: QuoteRef) => void;
}) {
  const bubble = useRef<View>(null);
  return (
    <FlightDestination route={route} text={echo.text} target="chat" dim={!echo.solid && !echo.conversation} measured={bubble}
      style={[styles.pending, (echo.solid || echo.conversation) && styles.pendingSolid]}>
      <UserBubble text={echo.text} sessionName={sessionName} serverId={serverId} onQuoteClick={onQuoteClick} bubbleRef={bubble} />
    </FlightDestination>
  );
}

/** Aviso do harness no meio da conversa (interrompida, compactada); a miniatura das novidades usa o mesmo. */
export function NoticeLine({ text }: { text: string }) {
  return <Text style={styles.notice}>{text}</Text>;
}

const styles = StyleSheet.create((theme) => ({
  area: { flex: 1 },
  folha: { position: 'absolute', top: theme.base.space[2], bottom: 0, borderRadius: theme.base.radius.lg },
  // Resposta sem bolha ocupa a largura: a margem lateral é o que separa o texto da borda da tela.
  content: {
    paddingHorizontal: theme.base.space[4],
    paddingVertical: theme.base.space[3],
    gap: theme.base.space[2],
  },
  footer: {
    gap: theme.base.space[3],
  },
  gap: {
    fontSize: theme.base.text.xs,
    color: theme.tokens.status.warning,
    textAlign: 'center',
    paddingVertical: theme.base.space[1],
    minHeight: 44,
    lineHeight: 44,
  },
  notice: {
    fontSize: theme.base.text.xs,
    color: theme.tokens.text.muted,
    textAlign: 'center',
    paddingVertical: theme.base.space[1],
  },
  turnNote: {
    fontSize: theme.base.text.xs,
    color: theme.tokens.status.warning,
    paddingVertical: theme.base.space[1],
  },
  pending: {
    opacity: 0.5,
  },
  pendingSolid: {
    opacity: 1,
  },
  toBottom: {
    position: 'absolute',
    right: theme.base.space[4],
    width: 40,
    height: 40,
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    backgroundColor: superficie(theme, 0.9),
    alignItems: 'center',
    justifyContent: 'center',
  },
  novasDot: {
    position: 'absolute',
    top: -2,
    right: -2,
    width: 10,
    height: 10,
    borderRadius: theme.base.radius.full,
    borderWidth: 2,
  },
}));
