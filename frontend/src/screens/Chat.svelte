<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { ctxPanel, LARGURA_TRILHO, reclamparLargura } from '../lib/ctxPanel.svelte';
  import NavBar from '../components/NavBar.svelte';
  import MessageList from '../components/MessageList.svelte';
  import Composer from '../components/Composer.svelte';
  import SessionSwitcherSheet from '../components/SessionSwitcherSheet.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
  import UsageSheet from '../components/UsageSheet.svelte';
  import Git from '../components/Git.svelte';
  import PreviewSheet from '../components/PreviewSheet.svelte';
  import ActivitySheet from '../components/ActivitySheet.svelte';
  import TerminalMirror from '../components/TerminalMirror.svelte';
  import TerminalMobile from '../components/TerminalMobile.svelte';
  import AskQuestionSheet from '../components/AskQuestionSheet.svelte';
  import RunSheet from '../components/RunSheet.svelte';
  import MoreSheet from '../components/MoreSheet.svelte';
  import AttachmentsSheet from '../components/AttachmentsSheet.svelte';
  import CodexLimitsSheet from '../components/CodexLimitsSheet.svelte';
  import ForwardSheet from '../components/ForwardSheet.svelte';
  import PairSheet from '../components/PairSheet.svelte';
  import OrquestracaoSheet from '../components/OrquestracaoSheet.svelte';
  import { prefetchOrq } from '../lib/queries';
  import { aoAquecer, segurarAquecimento, soltarAquecimento } from '../lib/aquecimento';
  // Ciclo de import de propósito (PairChatModal importa este Chat): é o mesmo Chat montado por
  // dentro. Só o render é recursivo — o modal só existe com `peerChat` preenchido, e ele nunca
  // abre outro modal (o PairSheet de lá abre o dele, mas o `peerChat` é por instância).
  import PairChatModal from '../components/PairChatModal.svelte';
  import LoopSheet from '../components/LoopSheet.svelte';
  import DesktopSessionContext from '../components/DesktopSessionContext.svelte';
  import NavegadorPane from '../components/NavegadorPane.svelte';
  import FileViewer from '../components/files/FileViewer.svelte';
  import { filesStores } from '../lib/filesStore.svelte';
  import { loopBadge, LOOP_TONE_COLOR } from '../lib/loop';
  import {
    getHistory,
    sendInput,
    steerSession,
    broadcast,
    selectOption,
    submitSelected,
    interrupt,
    openEventStream,
    getSessions,
    createSession,
    getWorkflows,
    getSubagents,
    answerQuestions,
    getRunners,
    isAbortError,
    isTimeoutError,
    getPlan,
    getConfig,
    uploadUrl,
  } from '../lib/api';
  import { formataErro } from '../lib/errosApi';
  import { appendTail, hasSeam, prependOlder } from '../lib/history';
  import { especificidade, donoDaLinha } from '../lib/covers';
  import { parseStatusLine } from '../lib/statusline';
  import { listServers, getActiveId } from '../lib/auth';
  import { createActivityFolder } from '../lib/activity';
  import type { ChatEvent, StateEvent, StatsEvent, State, SessionInfo, AskQuestionPayload, AnswerItem, Provider, PlanDetail, UploadFile } from '../lib/types';
  import type { WorkspaceAction } from '../lib/workspaceCommands';
  import { countAwaiting, nextAwaiting, providerName, untrackedReason, stateColors } from '../lib/format';
  import * as diag from '../lib/diag';
  import { ttsPlayer } from '../lib/ttsPlayer.svelte';
  import * as m from '../paraglide/messages';
  import { ouvirTexto } from '../lib/ouvir';
  import { textoFalavelComCodigo } from '../lib/speakable';

  interface Props {
    sessionName: string;
    onBack: () => void;
    onNavigateToChat: (name: string) => void;
    desktop?: boolean;   // montado no DesktopShell -> header sem "voltar"/switcher + atalhos de teclado
    onOpenSplit?: (name: string) => void; // desktop: abre o chat do PAR lado a lado (split view)
    // Painel de terminal real (xterm.js) na faixa do DesktopShell. So o DesktopShell sabe montar o
    // painel (e qual dos 3 <Chat> pediu); no celular fica undefined e abrirTerminalReal cai no espelho.
    onOpenTerminalPanel?: () => void;
    // True quando ESTA sessao ja tem o painel de terminal REAL aberto (o DesktopShell so sabe qual
    // dos 3 <Chat> e o dono). abrirTerminalReal, no desktop, nao mexe em mirrorOpen -- sem isto a
    // pilula "toque pra abrir" e o pulso do botao continuavam ativos com o painel ja aberto embaixo.
    terminalPanelOpen?: boolean;
    // Capacidade do SERVIDOR (GET /api/config, `somente_leitura.terminal_panel`): `pty` e POSIX-only,
    // entao no Windows o painel nao existe. Default true (assume capaz) pra nao piscar pro espelho
    // enquanto o DesktopShell ainda esta buscando a config -- so vira false quando o servidor confirma
    // que nao da. O mobile nunca passa isto (o painel real ja e desktop-only por outro motivo).
    terminalPanelDisponivel?: boolean;
    // Chrome global do DesktopShell: reserva espaço acima da 1ª mensagem e delega Cmd/Ctrl+K à
    // paleta cross-server. O mobile não passa nenhum dos dois e mantém o comportamento anterior.
    topInset?: number;
    onOpenWorkspacePalette?: () => void;
    showContextPanel?: boolean;
    // Follow-up visual: existe um toggle do painel de contexto FORA dele (barra de abas no modo
    // 'tabs', ou rodapé do rail no modo 'rail' com a sidebar recolhida) — repassado ao
    // DesktopSessionContext pra ele não duplicar o botão nem virar aba vertical.
    ctxToggleExterno?: boolean;
    publishWorkspaceActions?: boolean;
    onWorkspaceActionsChange?: (actions: WorkspaceAction[]) => void;
    // Este Chat já está DENTRO do modal do par. Corta a recursão: sem isso o par de lá é esta
    // mesma sessão, e abrir "o par num modal" empilhava modal sobre modal (3+ SSE abertos).
    nested?: boolean;
    // Split do desktop: o pane é estreito e a NavBar inteira vira ruído de mobile — no lugar dela
    // entra uma aba fina (bolinha de estado + nome + ✕). As ações da NavBar ficam acessíveis
    // fechando o split (o painel de contexto volta) — decisão de 2026-08-21.
    splitTab?: boolean;
    // ✕ da aba (só os panes de split têm; o principal não fecha sozinho).
    onCloseSplit?: () => void;
  }
  let {
    sessionName, onBack, onNavigateToChat, desktop = false, onOpenSplit, onOpenTerminalPanel,
    terminalPanelOpen = false, terminalPanelDisponivel = true,
    topInset = 0, onOpenWorkspacePalette, showContextPanel = false, ctxToggleExterno = false,
    publishWorkspaceActions = false, onWorkspaceActionsChange, nested = false,
    splitTab = false, onCloseSplit,
  }: Props = $props();

  // Sessão abrindo: os aquecimentos de cache (aqui e no Composer) esperam a conversa pintar antes
  // de tocar no backend. AQUI no corpo do script, e não num $effect: o corpo do pai roda antes de
  // o Composer existir, e um $effect rodaria DEPOIS dos efeitos dele — tarde demais pra segurar.
  // Solto no `finally` do loadHistory; ver lib/aquecimento pros números que motivaram isto.
  // O nome fica preso numa const: é a MESMA chave que o `finally` do loadHistory precisa usar pra
  // soltar. Um portão por sessão porque o app monta vários Chat ao mesmo tempo (split, chat do par).
  // svelte-ignore state_referenced_locally
  // Capturar o valor inicial é o certo AQUI, não um descuido: `sessionName` não muda dentro de uma
  // instância — TODO `<Chat>` do app vive dentro de um `{#key}` pela sessão (App.svelte:483,
  // DesktopShell.svelte:481/505, PairChatModal.svelte:23), justamente porque SSE e histórico ficam
  // amarrados a ela. E o `finally` do loadHistory PRECISA soltar exatamente a mesma chave.
  const sessaoDoPortao = sessionName;
  segurarAquecimento(sessaoDoPortao);

  let events = $state<ChatEvent[]>([]);

  // Store da aba Arquivos — MESMA instância do FilesPanel (registry por identidade
  // serverId::sessionName). Quem desenha o arquivo aberto no DESKTOP é este Chat (mock 2: o
  // arquivo cobre a conversa, a árvore fica viva no painel); o FilesPanel só marca a seleção.
  // No celular quem monta o visor é o próprio FilesPanel (Task 12) — daí o `desktop` abaixo.
  // retain/release: mesmo padrão do FilesPanel, o App remonta este Chat por {#key} a cada troca
  // de sessão e um store do mount morreria com o estado.
  // svelte-ignore state_referenced_locally — captura intencional: o App remonta este Chat por
  // {#key} a cada troca de sessao, entao o store do mount e o store da sessao — se a prop
  // mudasse no meio (nao muda), o FilesStore novo substituiria o velho e a regua de pastas
  // abertas morreria. Mesmo padrao do FilesPanel. A chave e a MESMA identidade do shell
  // (serverId::nome) que o FilesPanel usa — nunca calculada diferente por caller.
  const filesChave = `${getActiveId() ?? ''}::${sessionName}`;
  // svelte-ignore state_referenced_locally — a linha acima ja tem o ignore no comentario de
  // bloco; esta referência a sessionName (retain/release) e a mesma captura intencional.
  const filesStore = filesStores.retain(filesChave, sessionName);
  onDestroy(() => filesStores.release(filesChave));

  // Breakpoint do visor de arquivo e do painel de contexto: abaixo de 1280px os dois sao
  // display:none (CSS, mesma regua do DesktopSessionContext), entao um visor "aberto" nessa
  // largura seria conversa inerte com arquivo invisivel (B5, rodada 3). Mesmo padrao do isWide.
  let isDesktopLargo = $state(
    typeof window !== 'undefined' && window.matchMedia('(min-width: 1280px)').matches,
  );

  // O painel de contexto esta VISIVEL? (B1, Task 12): o Git desktop so esconde a aba Arquivos
  // quando este host existe — a MESMA condicao do visorAberto (incluindo a faixa de 1280px em
  // que o painel e display:none). Derivada aqui e passada ao Git, nunca recalculada no modal.
  const filesInContext = $derived(desktop && isDesktopLargo && showContextPanel && !ctxPanel.recolhido);

  // O visor segue o painel de contexto: some quando o painel fecha ou recolhe — e abaixo de
  // 1280px, largura em que os dois sao display:none — e o estado fica no store; reabrir o
  // painel restaura o arquivo (mesma regua de "pasta aberta continua aberta").
  const visorAberto = $derived(filesInContext && filesStore.selecionado !== null);
  // Path do arquivo aberto — variável LOCAL de propósito: dentro do {#if visorAberto} o template
  // estreita o tipo pra string (filesStore.selecionado é string | null e o TS não acompanha o if).
  const arquivoAberto = $derived(filesStore.selecionado);

  // Foco do visor (B5): o arquivo cobre a conversa sem véu, então o Tab não pode alcançar os
  // controles escondidos (MessageList/composer) — o underlay fica `inert` — e o foco tem dono:
  // captura o treeitem/resultado que abriu, move pro Fechar do visor, e devolve ao fechar
  // (×, voltar ou Esc). Sem isto o foco morria no elemento destruído ao fechar.
  let abridorEl: HTMLElement | null = null;
  let visorFocou = false;
  $effect(() => {
    if (!visorAberto) return;
    // O clique que abriu ainda tem o foco (treeitem/resultado) — captura antes de mover.
    abridorEl = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // Move o foco pro Fechar DEPOIS do mount do FileViewer (mesmo flush, tick garante).
    visorFocou = false;
    void tick().then(() => {
      if (!visorAberto || visorFocou) return;   // fechou/trocou no meio do tick
      visorFocou = true;
      (screenEl?.querySelector<HTMLElement>('.arq-visor .fechar'))?.focus();
    });
  });

  // Única saída do visor (×, voltar e Esc passam por aqui): limpa a seleção e devolve o foco
  // ao elemento que abriu — depois do tick, quando o visor já desmontou. O alvo so recebe o
  // foco se AINDA estiver conectado e visivel: no resize para <1280px o abridor (treeitem do
  // painel) esta display:none junto com o painel, e focar elemento oculto devolve o foco pro
  // body em vez de "devolver" (B5, rodada 4). O visor fechou: a marca de ownership morre (B1,
  // rodada 5) — uma transicao de resize posterior nao pode limpar nada em nome deste visor.
  function fecharVisor() {
    chatVisorEsteveAberto = false;
    const alvo = abridorEl;
    filesStore.selecionado = null;
    abridorEl = null;
    void tick().then(() => {
      if (alvo?.isConnected && alvo.getClientRects().length > 0) alvo.focus();
    });
  }

  // Resize para desktop estreito (B5, Task 11): abaixo de 1280px o visor e o painel sao
  // display:none e o visorAberto desliga — sem limpar a selecao, a conversa inteira ficaria
  // inerte com o arquivo invisivel. Limpa a selecao (voltar a largo monta o visor so com nova
  // selecao) e devolve o foco a um controle vivo do Chat: o Fechar do visor acabou de ser
  // escondido junto, e o abridor (treeitem do painel, display:none) tambem.
  // O guard de OWNERSHIP (B1/B2, Task 12): este Chat e o GitTabs do desktop sem contexto
  // compartilham o MESMO FilesStore. A limpeza so vale quando o visor era DO CHAT — a marca
  // chatVisorEsteveAberto e capturada no listener do matchMedia (transicao real, antes do
  // flush). Montagem ja estreita ou visor do Git: a marca fica false, nada e limpo e o foco
  // nao sai do modal.
  // O foco so pode ir DEPOIS do flush: o inert do underlay sai junto com a limpeza da selecao
  // acima, e focar sob inert e no-op (bloqueado no navegador e no happy-dom). Sessao morta nao
  // tem Composer (o bottom-dock vira o .dead-footer com o botao voltar) — o alvo cai no botao
  // que o usuario VE (B5, rodada 4).
  let chatVisorEsteveAberto = $state(false);
  $effect(() => {
    if (!desktop || isDesktopLargo) return;
    if (!chatVisorEsteveAberto) return;   // o visor nao era do Chat (Git, ou montagem estreita)
    chatVisorEsteveAberto = false;        // consome a transicao: limpa UMA vez, nao toda selecao
    if (filesStore.selecionado === null) return;
    fecharVisor();
    void tick().then(() => {
      // Modal aberto por cima (o Git do desktop e um BottomSheet role=dialog): o foco e DELE —
      // mover para o Composer atras do veu deixaria o usuario digitando invisivel (mesmo guard
      // do Composer.svelte e do DesktopShell, com o :not(.board-overlay) junto). A limpeza da
      // selecao acima ja aconteceu; so o foco nao viaja. O role=dialog so existe no DOM com a
      // folha aberta, por isso a leitura e aqui, apos o flush.
      if (document.querySelector('[role="dialog"]:not(.board-overlay)')) return;
      if (composerRef) composerRef.focus();
      else screenEl?.querySelector<HTMLElement>('.dead-footer .back-btn')?.focus();
    });
  });
  // O visor do Chat desmontou sem transicao (fechou, ou o contexto recolheu): a marca morre —
  // declarado DEPOIS do effect de transicao para nao competir com ele no mesmo flush.
  $effect(() => {
    if (!visorAberto) chatVisorEsteveAberto = false;
  });

  // Cache de prompt: o ULTIMO turno do assistente manda. Usar a cache renova o prazo, entao a
  // conta corre da hora do turno mais recente, e o TTL vem MEDIDO do transcript (1h ou 5min) —
  // sem os dois campos o Composer nao mostra prazo nenhum, em vez de inventar um.
  const lastCache = $derived.by(() => {
    // Duas buscas distintas de proposito. A ANCORA e o ultimo turno que tocou a cache (leu ou
    // gravou), porque usar renova o prazo. O TTL so aparece no turno que GRAVA — um turno que
    // apenas le vem sem ele. Exigir os dois no mesmo evento ancorava a conta num turno de escrita
    // antigo e subestimava o tempo restante (ate mostrar "expirou" com a cache recem-renovada).
    let ts: number | null = null;
    let read = 0;
    let ttl: number | null = null;
    for (let i = events.length - 1; i >= 0; i--) {
      const e = events[i];
      if (e.kind !== 'assistant_msg') continue;
      if (ts === null && e.ts && ((e.cache_read ?? 0) > 0 || e.cache_ttl_s)) {
        ts = e.ts;
        read = e.cache_read ?? 0;
      }
      if (ttl === null && e.cache_ttl_s) ttl = e.cache_ttl_s;
      if (ts !== null && ttl !== null) break;
    }
    return ts !== null && ttl !== null ? { ts, ttl, read } : null;
  });
  // Índice id->posição em `events`. O SSE re-emite o transcript INTEIRO a cada (re)conexão; sem isto
  // o dedup fazia findIndex O(n) por evento = O(n²) por reconexão -> em conversa longa (n grande), no
  // celular (reconecta a cada background/foreground), congelava a main thread. Map = lookup O(1).
  const idIndex = new Map<string, number>();
  function rebuildIndex() {
    idIndex.clear();
    for (let i = 0; i < events.length; i++) idIndex.set(events[i].id, i);
  }
  // Ids de assistant_msg que SUBSTITUIRAM um preview visivel: entram sem animacao (o texto ja
  // estava na tela via preview — re-animar era o "pisca" que fazia perder a posicao de leitura).
  const swapIds = new Set<string>();
  let stateEvent = $state<StateEvent | null>(null);
  // Faixa de estatísticas (evento SSE `stats`). Só o último snapshot importa (full-replace).
  let statsEvent = $state<StatsEvent | null>(null);
  let loading = $state(true);
  let error = $state('');
  let es: EventSource | null = null;
  let watchdog: ReturnType<typeof setTimeout> | undefined;     // liveness: reconecta se a conexao morrer calada
  // Última posição recebida do transcript; reenviada no reconnect pra retomar exatamente dali.
  let lastEventId: string | null = null;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  let screenEl: HTMLElement | undefined = $state();
  let pending = $state<{ id: string; text: string; solid?: boolean }[]>([]);
  // Draft do composer (bindable): o interrupt devolve a msg pendente aqui pra editar e reenviar.
  // PERSISTIDO por sessao no localStorage: o iOS mata/recarrega o PWA em background e o texto
  // digitado evaporava (ir buscar algo noutro app = perder o rascunho); trocar de sessao remonta
  // o Chat e zerava tambem. Restaura no mount; enviar limpa o campo -> remove a chave junto.
  // Snapshot do mount de proposito: o App remonta o Chat por {#key sessionName} a cada troca.
  // svelte-ignore state_referenced_locally
  const draftKey = `cp-draft:${sessionName}`;
  let composerText = $state(localStorage.getItem(draftKey) ?? '');
  $effect(() => {
    if (composerText) localStorage.setItem(draftKey, composerText);
    else localStorage.removeItem(draftKey);
  });
  // Preview AO VIVO do bloco de assistente em voo (lido do pane via SSE 'preview'). Texto-completo,
  // full-replace; some quando o assistant_msg canonico (do .jsonl) cobre o texto — sair de working
  // so agenda o drop (carencia abaixo), nunca apaga na hora.
  let previewText = $state('');
  // Texto da previa e markdown CRU (veio do agente: sidecar do Pi, deltas do Codex) e nao texto ja
  // pintado pela TUI. Decide se a bolha RENDERIZA -- ver AssistantBubble.
  let previewMd = $state(false);
  // Previa INCREMENTAL (so cresce no fim): sidecar/deltas, ou a costura do pane do Kimi
  // (_costurar no backend). Libera a bolha sem o teto de 10 linhas do texto raspado.
  let previewFull = $state(false);
  // Carencia entre o fim do turno e a bolha real: o Stop (hook) chega ANTES do tail do .jsonl
  // entregar o assistant_msg, entao apagar a previa na hora abria um buraco de ~1-2s no meio da
  // leitura e a bolha voltava re-animando. Sair de working AGENDA o drop; quem apaga de verdade
  // e o swap atomico do assistant_msg. O timer so vence se o bloco nunca vier (turno so de
  // ferramentas / interrompido) — a previa orfa nao pode ficar congelada pra sempre.
  let previewDropTimer: ReturnType<typeof setTimeout> | undefined;
  function dropPreviewSoon() {
    if (previewDropTimer !== undefined || !previewText) return;
    previewDropTimer = setTimeout(() => { previewDropTimer = undefined; previewText = ''; }, 5000);
  }
  function cancelPreviewDrop() {
    clearTimeout(previewDropTimer);
    previewDropTimer = undefined;
  }
  let dockEl: HTMLElement | undefined = $state();
  // Altura real do dock (composer) -> vira padding da lista pra ultima msg sempre limpar o glass.
  let dockH = $state(150);
  let navEl: HTMLElement | undefined = $state();
  // Altura real da navbar (overlay colado no topo) -> --nav-h: padding-top da lista pra 1a msg limpar a
  // navbar e o resto rolar POR BAIXO dela (= efeito glass). Mesmo modelo do dock.
  let navH = $state(56);


  // ── Switcher de sessoes (NavBar -> sheet) + criar nova sem voltar ──────────
  let switcherOpen = $state(false);
  let createOpen = $state(false);
  let usageOpen = $state(false);
  let gitOpen = $state(false);
  let runOpen = $state(false);
  let runRunning = $state(false);
  // Só acende o indicador do botão Rodar — nada na tela depende dele pra abrir. Espera a conversa.
  onMount(() => {
    void aoAquecer(sessionName).then(() =>
      getRunners(sessionName).then((r) => (runRunning = !!r.running)).catch(() => {}));
  });
  let previewOpen = $state(false);
  let activityOpen = $state(false);
  // Menu "⋯" do celular: Rodar/Atividade saíram da NavBar pra sobrar largura pro nome da sessão.
  let moreOpen = $state(false);
  // Galeria de anexos: "⋯" no celular, botao inline no desktop.
  let anexosOpen = $state(false);
  let limitsOpen = $state(false);  // Task B: sheet de limites de uso Codex (badge da NavBar)
  let askPayload = $state<AskQuestionPayload | null>(null);
  let askOpen = $state(false);
  // Pergunta nativa sintetizada do transcript (Pi: tool `question`; Kimi: `AskUserQuestion`): qual
  // tool_use_id abriu o sheet e qual o usuario ja DISPENSOU sem responder (fechou o sheet -> nao
  // reabre; o OptionButtons cru, lido do pane, fica como fallback). null = nenhuma.
  let askPiId = $state<string | null>(null);
  let askPiDismissed = $state<string | null>(null);
  // Viewport largo → pergunta vira card inline no chat (contexto visível); estreito → bottom-sheet.
  let isWide = $state(typeof window !== 'undefined' && window.matchMedia('(min-width: 768px)').matches);
  onMount(() => {
    const mq = window.matchMedia('(min-width: 768px)');
    const on = () => (isWide = mq.matches);
    mq.addEventListener('change', on);
    const mqLargo = window.matchMedia('(min-width: 1280px)');
    const onLargo = () => {
      // Ownership do visor do Chat (B1, rodada 5): quem garante a marca e a ORDEM destas duas
      // linhas — a de cima le visorAberto (derivada do isDesktopLargo ANTERIOR) e so a de
      // baixo escreve o valor novo. Invertidas, a derivada recalcularia no mesmo flush e a
      // marca nunca seria capturada: o Git do desktop sem contexto compartilha o MESMO
      // FilesStore, e sem a marca a transicao largo->estreito limparia a selecao do Git e
      // focaria o Composer atras do modal aberto. So o visor DO CHAT (filesInContext com
      // selecao) captura a marca.
      if (!mqLargo.matches && visorAberto) chatVisorEsteveAberto = true;
      isDesktopLargo = mqLargo.matches;
    };
    mqLargo.addEventListener('change', onLargo);
    // O teto do painel é função da largura da janela: encolher a janela invalida uma largura que
    // era legítima — reaplica o clamp na hora (sem salvar: a escolha grande volta no monitor
    // grande). Mesma régua da carga em tela menor.
    const onResize = () => reclamparLargura();
    window.addEventListener('resize', onResize);
    return () => {
      mq.removeEventListener('change', on);
      mqLargo.removeEventListener('change', onLargo);
      window.removeEventListener('resize', onResize);
    };
  });
  let allSessions = $state<SessionInfo[]>([]);
  // Detalhe do plano (Task 5b): NÃO usa o sessionsStore (mesmo motivo do loopChip acima — reter o
  // store aqui abria 1 stream de lista por servidor no celular). `allSessions` já é populada por
  // getSessions() (loadSessionsForNav, a cada 5s nas DUAS views) — reusa ela pra achar plan_name.
  const planSession = $derived(allSessions.find((s) => s.name === sessionName) ?? null);
  let planDetail = $state<PlanDetail | null>(null);
  let planLoading = $state(false);
  let planError = $state(false);
  // Chave do detalhe que está em `planDetail` agora. `let` cru (não $state) de propósito: só serve
  // pra comparar dentro do efeito — se fosse reativo, o efeito leria e escreveria a mesma coisa.
  let planDetailKey: string | null = null;
  // Nome do plano como PRIMITIVO, não o objeto `planSession` — mesmo bug do `pairPeersKey` umas
  // linhas abaixo: `allSessions` troca de referência a CADA poll de 5s (getSessions), então um
  // $effect que lê `planSession?.plan_name` direto re-executava em TODO poll, mesmo com o mesmo
  // plano (medido: 4 fetches em 4 polls idênticos). O /plan devolve o markdown inteiro (66 KB
  // neste repo) e passa pelo mesmo scan de tmux+/proc do registry que o poll da lista — refazer
  // isso a cada 5s dobrava a taxa de scan e ~47 MB/h de tráfego à toa no celular via Tailscale.
  const planName = $derived(planSession?.plan_name ?? null);
  // Busca só quando o plano MUDA de nome E o painel que o mostra PODE estar visível — os MESMOS
  // sinais que já decidem a renderização (`desktop && showContextPanel` no DesktopSessionContext;
  // `activityOpen` na ActivitySheet mobile). Sem isto o fetch rodava até com a sessão em split/
  // dentro do PairChatModal (nested), onde nenhum dos dois painéis chega a montar.
  const planPanelVisible = $derived((desktop && showContextPanel) || (!desktop && activityOpen));
  // Nome + quantos steps já foram marcados. O nome sozinho não bastava: no desktop o
  // DesktopSessionContext fica montado o tempo todo, então o efeito rodava UMA vez e a lista de ✓
  // congelava enquanto a barra (que vem do poll da lista) continuava andando. Com o `plan_done`
  // dentro da chave, o detalhe é refeito quando um step muda — e só aí, não a cada poll de 5s.
  const planKey = $derived(planName ? `${planName}:${planSession?.plan_done ?? 0}` : null);
  $effect(() => {
    const key = planKey;
    if (!key) { planDetail = null; planDetailKey = null; planError = false; return; }
    // Plano trocou com o painel fechado: joga o detalhe velho fora ANTES do gate de visibilidade,
    // senão ao reabrir o painel mostra o nome/barra do plano novo sobre as Tasks do plano velho
    // (o `{#if loading && !detail}` do PlanPanel não salva — `detail` não está null).
    if (planDetailKey !== key) { planDetail = null; planDetailKey = null; }
    else return;                     // já temos o detalhe desta chave: reabrir o painel não refaz
    if (!planPanelVisible) return;   // plano existe, mas nada o mostra agora: não busca à toa
    planLoading = true;
    planError = false;
    getPlan(sessionName)
      .then((d) => {
        if (planKey !== key) return;   // chegou tarde: já tem fetch mais novo no ar, descarta
        planDetail = d;
        planDetailKey = key;
        planLoading = false;
      })
      .catch((e) => {
        // 404 (sem plano) o getPlan já devolve como null; aqui é falha de verdade — 500, rede,
        // token vencido. Não pode virar "sem detalhe" mudo: loga e o painel diz que não deu.
        console.error('/plan falhou', e);
        if (planKey !== key) return;
        planDetail = null;
        planDetailKey = null;
        planError = true;
        planLoading = false;
      });
  });
  // Bolha sendo encaminhada pra outra sessao (long-press/hover ↗); null = sheet fechado.
  let forwardText = $state<string | null>(null);
  // Pareamento ("trabalhando juntas"): sheet + par atual derivado da lista já carregada.
  let pairOpen = $state(false);
  let orqOpen = $state(false);
  // Aquece o painel de Orquestração ao ENTRAR na sessão: o GET da política lê o disco e já foi
  // medido em ~3s frio, então buscá-lo no toque do botão é o que fazia o painel abrir em spinner.
  // Mesmo padrão do prefetch de modelos no Composer.
  // DEPOIS do histórico (ver lib/aquecimento): disparado junto, esse mesmo GET de 3s era o maior
  // ladrão da abertura — a conversa esperava a política que ninguém tinha pedido ainda.
  $effect(() => {
    const sn = sessionName;
    void aoAquecer(sn).then(() => prefetchOrq(sn));
  });
  // Membro do grupo aberto no modal (null = fechado). É string, não lista, de propósito: um modal
  // por vez mantém o teto em 2 SSE (este chat + o do par) e o navegador corta em ~6 por host.
  let peerChat = $state<string | null>(null);
  // Grupo de trabalho: os OUTROS membros (null = sem grupo). Estado vivo só quando o grupo tem 1
  // par (bolinha de 1 sessão faz sentido; de N vira ruído).
  const pairPeers = $derived(allSessions.find((s) => s.name === sessionName)?.pair_peers ?? null);
  // Chave PRIMITIVA do grupo: pair_peers é um array NOVO por referência a cada poll de 5s do
  // getSessions — efeitos que dependessem do array re-rodavam sem o grupo ter mudado (toggle se
  // autodesligava, sheet resetava). String igual não re-notifica.
  const pairPeersKey = $derived(pairPeers?.join(',') ?? '');
  const pairedState = $derived(pairPeers?.length === 1
    ? (allSessions.find((s) => s.name === pairPeers[0])?.state ?? null) : null);

  async function openSwitcher() {
    switcherOpen = true;
    try {
      allSessions = await getSessions();
    } catch {
      // sem lista -> o sheet ainda oferece "Nova sessão"
    }
  }

  function pickSession(name: string) {
    switcherOpen = false;
    if (name !== sessionName) onNavigateToChat(name);
  }

  function startNew() {
    switcherOpen = false;
    bastaoAlvo = null;
    createOpen = true;
  }

  // Passagem de bastão pelo "⋯" do celular (a lista mobile não tem menu por sessão). Mesma folha de
  // criar, aberta pra CONTINUAR esta conversa; o cwd sai da linha da sessão na lista agregada.
  let bastaoAlvo = $state<{ name: string; cwd: string; serverId: string } | null>(null);
  function passarBastaoDaqui() {
    bastaoAlvo = { name: sessionName, cwd: planSession?.cwd ?? '', serverId: getActiveId() ?? '' };
    createOpen = true;
  }

  async function handleCreate(name: string, cwd?: string, configDir?: string | null, provider?: Provider,
                              engine?: string | null, model?: string | null, effort?: string | null,
                              permissionMode?: string | null) {
    await createSession(name, cwd, configDir, provider, engine, model, effort, permissionMode);
    onNavigateToChat(name);
  }

  // ── Atalhos de teclado (so desktop) ────────────────────────────────────────
  let composerRef = $state<{ focus: () => void; ditarArquivo: (f: File) => void } | undefined>();

  // Anexo de audio de volta pro ditado: busca o arquivo que ja esta no servidor e entrega ao
  // Composer, que transcreve de novo e abre a barra de versoes. O download acontece AQUI porque a
  // sheet nao conhece o Composer, e o Composer so sabe lidar com File.
  async function usarAnexoNoDitado(f: UploadFile) {
    try {
      const res = await fetch(uploadUrl(sessionName, f.filename));
      if (!res.ok) throw new Error(`${res.status}`);
      const blob = await res.blob();
      composerRef?.ditarArquivo(new File([blob], f.filename, { type: blob.type }));
    } catch (e) {
      error = `${m.anexos_erro_listar()} ${e instanceof Error ? e.message : String(e)}`;
    }
  }

  // Lista pra navegar sessao com Ctrl/Cmd+setas (desktop) e pra pilula "N aguardando" (mobile,
  // feature #4). Carregada no mount nos dois; no mobile reconsulta a cada 5s pra o contador da
  // pilula refletir sessoes que entram/saem de awaiting_input enquanto o usuario fica parado aqui
  // (esta tela nao tem SSE agregado de sessoes — reusa o mesmo REST de sempre, so com poll).
  let navInFlight = false;   // socket pendurado empilhava 1 fetch por tick de 5s ate esgotar o host
  async function loadSessionsForNav() {
    if (navInFlight) return;
    navInFlight = true;
    try { allSessions = await getSessions(); } catch { /* sem lista -> setas/pilula viram no-op */ }
    finally { navInFlight = false; }
  }
  onMount(() => {
    loadSessionsForNav();
    // Poll nos DOIS views (era só mobile): o chip 🤝 mostra o estado vivo do PAR — sem reconsultar,
    // a bolinha congelava no desktop. Mesmo REST leve de sempre, a cada 5s.
    const id = setInterval(loadSessionsForNav, 5000);
    return () => clearInterval(id);
  });

  function switchRelative(delta: number) {
    const names = allSessions.map((s) => s.name);
    if (names.length < 2) { loadSessionsForNav(); return; }
    const i = names.indexOf(sessionName);
    const next = names[((i < 0 ? 0 : i + delta) + names.length) % names.length];
    if (next && next !== sessionName) onNavigateToChat(next);
  }

  // Pilula de triage "N aguardando" (mobile only, feature #4): conta e pula direto pra proxima
  // sessao awaiting_input (wrap-around), reusando o MESMO onNavigateToChat do switchRelative acima
  // — so a escolha do alvo muda (filtrada+ordenada por nextAwaiting em vez de delta sequencial).
  // $derived de allSessions -> nunca cacheia a contagem; some sozinha quando ninguem mais aguarda.
  const awaitingCount = $derived(countAwaiting(allSessions));
  function goNextAwaiting() {
    const next = nextAwaiting(allSessions, sessionName);
    if (next && next !== sessionName) onNavigateToChat(next);
  }

  const anyOverlayOpen = () =>
    switcherOpen || createOpen || usageOpen || gitOpen || runOpen || previewOpen || activityOpen || limitsOpen || mirrorOpen || xtermOpen || askOpen || moreOpen || anexosOpen;
  function closeOverlays() {
    switcherOpen = createOpen = usageOpen = gitOpen = runOpen = previewOpen = activityOpen = limitsOpen = moreOpen = anexosOpen = false;
    if (mirrorOpen) closeMirror();
    xtermOpen = false;
    askOpen = false;
  }

  function onGlobalKey(e: KeyboardEvent) {
    if (!desktop) return;
    const mod = e.ctrlKey || e.metaKey;
    if (e.key === 'Escape' && anyOverlayOpen()) {
      // Dialog/sheet components own Escape at their boundary. If the event
      // originated inside one, do not let this page-level fallback dismiss
      // every tracked overlay behind it.
      const target = e.target;
      if (target instanceof Element && target.closest('[role="dialog"]')) return;
      if (e.defaultPrevented) return;
      e.preventDefault();
      closeOverlays();
      return;
    }
    // O visor do arquivo (Task 11) fecha com Esc, como um sheet — sem véu, é a saída de teclado
    // que espelha o × e o "voltar à conversa". Overlay aberto por cima tem prioridade (bloco
    // acima): o Esc fecha o sheet primeiro, o próximo fecha o visor.
    if (e.key === 'Escape' && visorAberto) {
      e.preventDefault();
      fecharVisor();
      return;
    }
    if (mod && (e.key === 'k' || e.key === 'K')) {
      e.preventDefault();
      e.stopPropagation();
      if (onOpenWorkspacePalette) onOpenWorkspacePalette();
      else openSwitcher();
      return;
    }
    if (mod && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      e.preventDefault(); switchRelative(e.key === 'ArrowDown' ? 1 : -1); return;
    }
    // e.repeat: segurar o atalho alternaria pause/play em rajada (o loading guard so protege o
    // inicio). stopImmediatePropagation: com 2 Chats montados (split view do DesktopShell), os dois
    // onGlobalKey receberiam o MESMO keydown — dois toggle() se anulavam (pause->play no mesmo
    // evento) e dois ouvirTexto() disparavam juntos. O primeiro Chat montado vence; e
    // deterministico, embora no split o atalho sempre aja no painel principal.
    if (mod && e.shiftKey && e.code === 'Space') {
      if (e.repeat) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      ouvirUltimaRespostaVisivel();
      return;
    }
    const el = e.target as HTMLElement | null;
    const typing = !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
    if (typing) return;   // "/" foca o composer so quando NAO ja digitando num campo
    if (e.key === '/') {
      // Modal aberto (BottomSheet role=dialog): o foco e dele — mesmo guard do Composer/DesktopShell.
      if (document.querySelector('[role="dialog"]:not(.board-overlay)')) return;
      e.preventDefault();
      composerRef?.focus();
    }
  }

  // Atalho "ouvir a resposta" (desktop, Ctrl/Cmd+Shift+Espaco): toca a ultima bolha de assistente
  // VISIVEL na tela — se o usuario rolou pra cima pra reler uma resposta antiga, e ELA que toca,
  // nao a mais recente do transcript. Sem nenhuma visivel (scroll num trecho so de ferramentas),
  // cai na ultima do DOM. Ja tocando, o atalho vira pause/continua em vez de empilhar outra leitura.
  // Toca "como esta" (ouvirTexto direto, sem Groq): textoFalavelComCodigo ja troca <pre> por
  // "trecho de codigo omitido", entao o atalho nao gasta LLM. Quem quer "explicar o codigo" segue
  // tendo o botao da bolha, que abre o painel de opcoes.
  function ouvirUltimaRespostaVisivel() {
    // Mesmo guard do DesktopShell: sheet/modal aberto por cima -> o atalho nao dispara "por tras".
    // (O overlay do board usa role="region", nem casa com o seletor — o :not(.board-overlay) e
    // cinto-e-suspensorios pro dia que ele virar dialog.)
    if (document.querySelector('[role="dialog"]:not(.board-overlay)')) return;
    if (ttsPlayer.loading) return;                 // sintese em voo: nem pausa nem comeca outra
    if (ttsPlayer.active) { ttsPlayer.toggle(); return; }
    const lista = screenEl?.querySelector('.message-list');
    const bolhas = lista
      ? [...lista.querySelectorAll<HTMLElement>('.assistant-msg:not(.preview) .prose')]
      : [];
    // O limite inferior desconta o dock (dockH, medido pelo Chat): uma bolha escondida ATRAS do
    // composer nao conta como visivel — tocar uma mensagem que a pessoa nao consegue ver era o bug.
    const limiteInf = window.innerHeight - (dockH || 0);
    const visiveis = bolhas.filter((b) => {
      const r = b.getBoundingClientRect();
      return r.bottom > 0 && r.top < limiteInf;
    });
    const alvo = visiveis.at(-1) ?? bolhas.at(-1);
    if (!alvo) {
      // Chat vazio ou so mensagens tuas: o atalho nao pode morrer em silencio (regra do projeto) —
      // unlock() monta a barra e o fail() deixa o motivo nela.
      ttsPlayer.unlock('');
      ttsPlayer.fail(m.chat_tts_sem_resposta());
      return;
    }
    const { texto } = textoFalavelComCodigo(alvo);
    ouvirTexto(texto, (msg) => Promise.resolve(window.confirm(msg)), '');
  }

  const currentState = $derived<State>(stateEvent?.state ?? 'idle');
  // Provider desta sessao (allSessions ja carregada pro switcher/nav — sem round-trip extra).
  // "claude" e o caso comum e some do header; os demais ganham badge (providerBadge abaixo) e o
  // "codex" alem disso esconde controles Claude-only.
  const sessionProvider = $derived(allSessions.find((s) => s.name === sessionName)?.provider);
  // Motor da sessão (null = conta Anthropic) — o Composer usa no placeholder ("Mensagem para …").
  const sessionEngine = $derived(allSessions.find((s) => s.name === sessionName)?.engine ?? null);
  const isCodex = $derived(sessionProvider === 'codex');
  const sessionTracked = $derived(allSessions.find((s) => s.name === sessionName)?.tracked);
  // Kimi "sem id" e o estado NORMAL pre-1o-prompt: o Kimi so cria a sessao (id + wire.jsonl) no
  // primeiro envio. /history e /events 404am ate la — NAO e erro: o chat mostra um hint e o composer
  // segue usavel (o POST /input vai por tmux, nao precisa de jsonl). `kimiSemTranscript` cobre a
  // janela em que o 404 do /history chegou ANTES da lista reportar tracked=false (poll de 5s) —
  // mas tracked=true sempre desempata (transcript existe, por mais que o 404 tenha vindo antes).
  let kimiSemTranscript = $state(false);
  const kimiPreNascimento = $derived(sessionProvider === 'kimi'
    && (sessionTracked === false || (kimiSemTranscript && sessionTracked !== true)));
  // Nascimento da sessao kimi: o hook grava o ticket ~1s apos o 1o prompt e o poll da lista traz
  // tracked=true — carrega history e conecta o SSE (que o guard de kimiPreNascimento no connectSSE
  // segurou ate aqui). Chave em PRIMITIVOS: allSessions troca de referencia a cada poll de 5s,
  // entao efeito lendo o objeto re-rodaria em todo poll (ver pairPeersKey).
  let kimiEstavaSemId = false;
  $effect(() => {
    const nasceu = sessionProvider === 'kimi' && sessionTracked === true;
    if (kimiEstavaSemId && nasceu) {
      kimiSemTranscript = false;
      loadHistory().then(() => { if (alive) connectSSE(); });
    }
    kimiEstavaSemId = kimiPreNascimento;
  });
  // Badge do provider na NavBar (mobile): so aparece quando NAO e Claude — antes so o Codex tinha
  // rotulo e uma sessao Pi ficava sem badge nenhum, indistinguivel de uma Claude no celular.
  // O TAP continua so do Codex: a sheet de limites de uso e da API do Codex, o Pi nao tem.
  const providerBadge = $derived(
    sessionProvider && sessionProvider !== 'claude' ? providerName(sessionProvider) : null);
  // Espelho do pane (overlays so-TUI: /status, /config, /help, pickers). MANUAL: o usuario abre pelo
  // botao da NavBar ou pela pilula de aviso — NUNCA toma a tela sozinho (auto-takeover assustava +
  // prendia). tuiOverlay = ha um overlay aberto que SO da pra interagir pela TUI (sem opcoes nativas;
  // awaiting_input vira OptionButtons na lista). Serve so pra DESTACAR (pulsar o botao + mostrar a
  // pilula), nao pra abrir.
  // login: sessao parada no welcome/login do Claude Code (sem .jsonl -> chat vazio). Reusa o mesmo
  // affordance do overlay (pulsa o botao do terminal + pill -> abre o espelho), so com texto proprio.
  const needsLogin = $derived(!!stateEvent?.login && currentState !== 'awaiting_input');
  const tuiOverlay = $derived((!!stateEvent?.overlay || needsLogin) && currentState !== 'awaiting_input');
  let mirrorOpen = $state(false);
  // Terminal de VERDADE no celular (xterm sobre o PTY do backend). O espelho continua vivo pro
  // servidor sem `pty` (Windows), onde este abriria morto.
  let xtermOpen = $state(false);
  // Capacidade do SERVIDOR (GET /api/config, `somente_leitura.terminal_panel`). Default true
  // (assume capaz) pra o primeiro toque nao cair no espelho so porque a config ainda nao chegou;
  // falha de rede tambem mantem true e deixa o proprio terminal mostrar o erro real.
  let terminalCapazMobile = $state(true);
  $effect(() => {
    if (desktop) return;
    let vivo = true;
    getConfig()
      .then((c) => { if (vivo) terminalCapazMobile = c.somente_leitura.terminal_panel !== false; })
      .catch(() => {});
    return () => { vivo = false; };
  });

  // Pergunta nativa do Pi (tool `question`). O Pi nao tem o hook de AskUserQuestion do Claude, mas
  // nao precisa: o toolCall cai no transcript com o payload COMPLETO (pergunta, header, opcoes com
  // descricao) no instante da pergunta. Aqui o app sintetiza o MESMO AskQuestionPayload do Claude e
  // abre o sheet/card nativo; o /answer do backend ramifica por provider e dirige o picker do Pi.
  // Pendente = tool_use 'question' sem tool_result com o mesmo id. (2026-08-04)
  // Kimi: mesmo desenho, mas o parser emite tool_name 'AskUserQuestion' com tool_input JA no shape
  // do Claude ({questions: [{question, header, options, multi_select}]}) — o mapeamento abaixo
  // ramifica por provider. (2026-08-11)
  const pendingPiQuestion = $derived.by(() => {
    const toolName = sessionProvider === 'pi' ? 'question'
      : sessionProvider === 'kimi' ? 'AskUserQuestion' : null;
    if (!toolName) return null;
    // Varredura UNICA: coleciona os resultados e lembra o ultimo tool_use question; pendente =
    // esse ultimo sem resultado. O(n) por evento novo, loop simples (o fold caro que o projeto
    // baniu era o deriveActivity; aqui e so Set+ultimo — se pesar, medir antes de otimizar).
    const answered = new Set<string>();
    let last: ChatEvent | null = null;
    for (const ev of events) {
      if (ev.kind === 'tool_result' && ev.tool_use_id) answered.add(ev.tool_use_id);
      else if (ev.kind === 'tool_use' && ev.tool_name === toolName && ev.tool_use_id) last = ev;
    }
    return last && !answered.has(last.tool_use_id ?? '') ? last : null;
  });

  $effect(() => {
    const q = pendingPiQuestion;
    if (!q) {
      // A resposta aterrissou no transcript (pelo app ou pelo terminal) -> fecha o sheet se foi
      // uma pergunta do Pi/Kimi que o abriu.
      if (askPiId) { askPiId = null; askOpen = false; }
      return;
    }
    if (askOpen || askPiDismissed === q.id) return;
    const args = (q.tool_input ?? {}) as Record<string, unknown>;
    const mapOpts = (opts: unknown) => (Array.isArray(opts) ? opts : []).map((o) => ({
      label: String((o as Record<string, unknown> | null)?.label ?? ''),
      description: String((o as Record<string, unknown> | null)?.description ?? ''),
    })).filter((o) => o.label);
    if (sessionProvider === 'kimi') {
      // Shape do Claude: lista de perguntas pronta, so falta snake_case -> camelCase.
      const qs = (Array.isArray(args.questions) ? args.questions : []).map((item) => {
        const it = item as Record<string, unknown> | null;
        return {
          header: String(it?.header ?? ''),
          question: String(it?.question ?? ''),
          multiSelect: it?.multi_select === true,
          options: mapOpts(it?.options),
        };
      }).filter((item) => item.question && item.options.length);
      if (!qs.length) {
        console.warn(m.chat_kimi_payload(), args);
        return;
      }
      askPayload = { questions: qs };
    } else {
      const options = mapOpts(args.options);
      if (!options.length || !args.question) {
        // Shape inesperado (o Pi mudou o tool?) — sem o warn o sheet simplesmente parava de abrir um
        // dia, calado. O OptionButtons cru segue como saida.
        console.warn(m.chat_pi_payload(), args);
        return;
      }
      askPayload = {
        questions: [{
          header: String(args.header ?? ''),
          question: String(args.question),
          multiSelect: args.multiSelect === true,
          options,
        }],
      };
    }
    askPiId = q.id;
    askOpen = true;
  });

  // Fechar o sheet SEM responder: se era pergunta do Pi, registra a dispensa pra ele nao reabrir
  // sozinho (o OptionButtons cru segue disponivel como saida).
  function closeAsk() {
    if (askPiId) askPiDismissed = askPiId;
    askOpen = false;
  }
  function openMirror() { mirrorOpen = true; }
  // "Voltar ao chat" = SO esconde o espelho. NAO manda Escape -> a TUI fica como esta (nao fecha o
  // painel que o usuario queria ler). Sair do overlay de proposito = tecla Esc na barra do espelho.
  function closeMirror() { mirrorOpen = false; }
  // Painel de verdade no desktop; espelho no celular -- e tambem no desktop quando o SERVIDOR nao tem
  // a capacidade (Windows: `pty` e POSIX-only, o painel abriria morto). NAO reusar isto no onFallback
  // do AskUserQuestion: o fallback existe pra destravar picker, e o painel bloqueia o /answer (Task 3).
  function abrirTerminalReal() {
    if (desktop && onOpenTerminalPanel && terminalPanelDisponivel) onOpenTerminalPanel();
    else if (!desktop && terminalCapazMobile) xtermOpen = true;
    else mirrorOpen = true;
  }
  // Statusline crua -> campos tipados (modelo, contexto, custo, tempo de sessao).
  const status = $derived(parseStatusLine(stateEvent?.status_line ?? null));

  // Header: breadcrumb desktop (servidor › sessao › branch) e subtítulo mobile (nome do servidor
  // sob o título — com N servidores, sessões homônimas ficavam indistinguíveis no celular).
  const serverLabel = $derived(listServers().find((s) => s.id === getActiveId())?.label ?? '');

  // Chip de loop no header: dentro do chat não havia NENHUM sinal de loop ativo (só a lista tinha
  // badge). Os campos vêm do sessionsStore (singleton refcounted — zero SSE novo); tap abre o sheet.
  let loopSheetOpen = $state(false);
  // Navegador embutido ao lado do chat (WebContentsView no shell; iframe fora dele). Só desktop:
  // no celular quem cobre o caso é o PreviewSheet (túnel da porta pra URL alcançável).
  let navOpen = $state(false);
  // Campos de loop vêm do SSE DA PRÓPRIA SESSÃO (stateEvent), não do sessionsStore: reter o
  // store aqui abria 1 stream de lista POR SERVIDOR no celular (com offline = retry eterno) e
  // derrubava a conexão do pocket — regressão real vista no iPhone, revertida.
  const loopChip = $derived(loopBadge(stateEvent?.loop_status, stateEvent?.loop_iter, stateEvent?.loop_max));
  const crumbs = $derived(
    desktop ? { server: serverLabel, session: sessionName, branch: status?.branch, dirty: status?.dirty ?? false } : null
  );

  function action(id: string, title: string, run: () => void): WorkspaceAction {
    const metadata: Record<string, Pick<WorkspaceAction, 'detail' | 'keywords' | 'group'>> = {
      git: {
        detail: m.chat_acao_git_detalhe(),
        keywords: ['git', m.chat_alteracoes(), 'diff', 'branch'],
        group: m.lista_ferramentas(),
      },
      loop: {
        detail: m.chat_acao_loop_detalhe(),
        keywords: ['loop', m.chat_automacao(), m.chat_iteracoes()],
        group: m.lista_ferramentas(),
      },
      pair: {
        detail: m.chat_acao_parear_detalhe(),
        keywords: ['parear', 'pair', 'par', m.sessao_singular(), 'split'],
        group: m.lista_colaboracao(),
      },
      run: {
        detail: m.chat_acao_run_detalhe(),
        keywords: ['executar', 'workflow', 'run'],
        group: m.lista_ferramentas(),
      },
      terminal: {
        detail: currentState === 'dead'
          ? m.chat_acao_terminal_indisponivel()
          : m.chat_acao_terminal_detalhe(),
        keywords: ['terminal', 'espelho', 'tui', 'pane'],
        group: m.lista_ferramentas(),
      },
      navegador: {
        detail: m.chat_acao_navegador_detalhe(),
        keywords: ['navegador', 'browser', 'localhost', 'site'],
        group: m.lista_ferramentas(),
      },
    };
    return {
      id,
      title,
      ...metadata[id],
      disabled: id === 'terminal' && currentState === 'dead',
      run,
    };
  }

  $effect(() => {
    const publish = onWorkspaceActionsChange;
    if (!desktop || !publishWorkspaceActions || !publish) return;
    publish([
      action('git', m.sessao_git(), () => (gitOpen = true)),
      action('loop', m.chat_loop(), () => (loopSheetOpen = true)),
      action('pair', m.chat_parear_sessao(), () => (pairOpen = true)),
      action('run', m.chat_executar_workflow(), () => (runOpen = true)),
      action('terminal', m.ctx_terminal(), abrirTerminalReal),
      action('navegador', m.ctx_navegador(), () => (navOpen = true)),
    ]);
    // Ao trocar a key servidor-aware ou desmontar este Chat, nenhum callback pode sobreviver.
    return () => publish([]);
  });

  // Anuncio de estado pra screen reader: a transicao que pede acao humana (awaiting_input) nao
  // tinha NENHUM sinal nao-visual. role="status" (aria-live polite) num no visualmente escondido.
  let stateAnnounce = $state('');
  let prevAnnounced: State | null = null;
  $effect(() => {
    const s = currentState;
    if (prevAnnounced !== null && s !== prevAnnounced) {
      if (s === 'awaiting_input') stateAnnounce = m.chat_sessao_aguardando_resposta({ n: sessionName });
      else if (s === 'dead') stateAnnounce = m.chat_sessao_encerrada_anuncio({ n: sessionName });
      else stateAnnounce = '';
    }
    prevAnnounced = s;
  });
  // Painel de atividade: tarefas (TaskCreate/Update) + agentes rodando. Fold INCREMENTAL: o handler
  // do SSE dá push evento a evento — deriveActivity(events) como $derived re-varria o histórico
  // INTEIRO a cada mensagem (O(n) por evento em sessão longa).
  const actFolder = createActivityFolder();
  let activity = $state(actFolder.snapshot());
  // O shell de fundo conta junto: pro app "o que está rodando aqui" é uma coisa só. O terminal já
  // mostrava ("5 shells still running") e o app não mostrava nada.
  const activityBadge = $derived(activity.inProgress + activity.runningAgents + activity.runningShells);

  // Subagentes que existem NO DISCO (`<session-dir>/subagents/agent-*.jsonl`), contados pelo
  // backend. Sem isto o painel só abria quando o transcript trazia a ferramenta `Agent` — e o uso
  // real mudou: skill que forka entra como `Skill`, e agente de FUNDO não entra como ferramenta
  // nenhuma. Medido em 18/08/2026 numa sessão do usuário: 0 `Agent` no transcript, 3 subagentes no
  // disco, e o botão de Atividade nunca aparecia — com os dados prontos numa rota que já existia.
  let subagentesNoDisco = $state(0);
  const hasActivity = $derived(
    activity.tasks.length > 0 || activity.agents.length > 0 || activity.runningShells > 0
    || subagentesNoDisco > 0,
  );

  // Quando perguntar: enquanto TRABALHA (é quando nasce subagente) e uma vez ao parar, pra pegar o
  // último que terminou junto com o turno. Sessão parada não fica batendo no backend.
  $effect(() => {
    const trabalhando = currentState === 'working';
    let vivo = true;
    async function contar() {
      try {
        const lista = await getSubagents(sessionName);
        if (vivo) subagentesNoDisco = lista.length;
      } catch { /* offline / sessão sem transcript -> mantém o que tinha */ }
    }
    // A 1ª contagem espera a conversa pintar (ver lib/aquecimento): ela só acende o ponto do botão
    // de Atividade. Depois que o histórico chega a espera já está resolvida e o ciclo de 5s corre
    // no ritmo de sempre.
    void aoAquecer(sessionName).then(() => { if (vivo) void contar(); });
    const id = trabalhando ? setInterval(contar, 5000) : undefined;
    return () => { vivo = false; if (id !== undefined) clearInterval(id); };
  });

  // Workflow roda em BACKGROUND -> nao da pra inferir "rodando" so pelos eventos (activity.ts marca
  // workflow running:false). Pergunta ao backend (le os arquivos do run) SÓ com motivo: sheet de
  // atividade aberto ou run ainda ativo; um workflow NOVO no transcript (wfCount muda) dispara UM
  // poll (kick) que, se estiver rodando, liga o loop de 4s até terminar. Antes: qualquer workflow
  // no histórico (mesmo finalizado há dias) pollava a cada 4s pra sempre.
  let workflowRunning = $state(false);
  const wfCount = $derived(activity.agents.filter((a) => a.kind === 'workflow').length);
  const activityRunning = $derived(workflowRunning || activity.runningAgents > 0);
  $effect(() => {
    if (!wfCount) { workflowRunning = false; return; }
    const sustain = activityOpen || workflowRunning;
    let alive = true;
    async function poll() {
      try {
        const ws = await getWorkflows(sessionName);
        if (alive) workflowRunning = ws.some((w) => w.running);
      } catch { /* offline / sem run -> ignora */ }
    }
    poll(); // kick: roda 1x a cada mudança de wfCount/activityOpen/workflowRunning
    const id = sustain ? setInterval(poll, 4000) : undefined;
    return () => { alive = false; if (id !== undefined) clearInterval(id); };
  });

  // Contadores/folds incrementais re-semeados junto com `events` (reseed completo).
  function reseedDerived() {
    actFolder.reset(events);
    activity = actFolder.snapshot();
    asstCount = countAssts(events);
    // Reseed NÃO é "commitou bloco novo": trocar o conjunto de eventos (history em dois tempos,
    // /clear, resume) mexe no contador sem nenhum turno ter fechado. Sem sincronizar aqui, abrir a
    // conversa no meio de um turno longo MATAVA a prévia — ela chega antes do /history (o SSE
    // manda o texto do pane na hora da inscrição, o history é fetch), o reseed disparava o
    // "committed" do effect abaixo, e o broker só reemite em MUDANÇA do pane: num painel de
    // tarefas parado, nunca mais. Quem de fato detecta commit é o handler do SSE, que já zera o
    // preview no mesmo flush do append (swap atômico).
    _asstSeen = asstCount;
  }

  // Classifica o erro de carga: 404 / "not found" = transcript trocado ou backend reiniciou (o caso
  // mais comum) -> copy propria + saida. O resto mostra a mensagem crua.
  const errorInfo = $derived({ notFound: /(^|\D)404(\D|$)/.test(error) || /not found/i.test(error) });

  // Carga em DOIS TEMPOS. O /history sem limite lê o jsonl inteiro (medido: 1596 eventos, ~542ms
  // num transcript de 136MB) só pro MessageList montar as últimas 120 linhas — e pagava isso ao
  // ABRIR a conversa e a cada volta do segundo plano (no iPhone, o tempo todo). Agora a cauda vem
  // primeiro (a tela pinta com ela) e o histórico antigo chega em segundo plano, de modo que rolar
  // pra cima continua revelando páginas EM MEMÓRIA, sem chamada nova ao backend (invariante do
  // CLAUDE.md).
  // 400 e não 120: (a) o MessageList janela em WINDOW=120 eventos CRUS e filtra tool_result, então
  // 120 crus podem virar meia dúzia de bolhas numa sessão de ferramentas; 400 dão ~2 páginas de
  // PAGE=100 pra rolar antes de a carga de fundo chegar; (b) 400 > _BACKFILL_LINES=200 do SSE, o
  // que faz a mesma cauda servir pra fechar o buraco do resume (onVisible) sem re-baixar tudo.
  const TAIL_FIRST = 400;
  // Uma geração por carga: troca de sessão, /clear, resume ou destroy invalidam o que está em voo,
  // pra resposta velha nunca cair na sessão errada (padrão que já existia como `visGen`).
  let histGen = 0;
  // ...e a geração nova ABORTA o fetch da anterior. O guard de geração já descartava a resposta
  // velha, mas o download seguia até o fim: pular de sessão em sessão no switcher (cada uma é um
  // Chat NOVO por {#key}), abrir o PairChatModal/split (outro Chat, outra carga de fundo) ou dar
  // /clear no meio deixava vários /history completos disputando a rede — no celular, justamente o
  // caso que a carga em dois tempos existe pra resolver.
  let histAbort: AbortController | null = null;
  function newHistLoad(): AbortSignal {
    histGen++;
    histAbort?.abort();
    histAbort = new AbortController();
    return histAbort.signal;
  }
  // A carga de FUNDO não completou. 'failed' = erro de rede/backend, e tocar tenta de novo.
  // 'unjoinable' = o histórico veio de OUTRO transcript (nenhum id em comum, /clear no meio do
  // voo): a conversa fica truncada na cauda e o usuário precisa saber — mas repetir a busca daria
  // o mesmo resultado, então esta não convida a tentar. Sumir calada é que não pode.
  let histGap = $state<'' | 'failed' | 'unjoinable'>('');

  // Primeira carga: teto CURTO e uma segunda tentativa. Medido no iPhone em 17/08/2026 — abrir uma
  // sessao caia num skeleton parado por minutos e o pedido NAO aparecia no log do backend (nem
  // chegou a sair do celular). Com o teto unico de 45s, esse tempo todo era tela de esqueleto sem
  // nada pra fazer. Agora: 10s, segunda tentativa (conexao nova) com 15s, e ai a tela de erro com o
  // botao de tentar de novo — que ja existia e ninguem alcancava.
  const TAIL_TIMEOUT_1 = 10_000;
  const TAIL_TIMEOUT_2 = 15_000;
  let histRetentando = $state(false);

  // `g` (a geracao da carga) entra aqui pelo mesmo motivo de todo o resto do loadHistory: uma carga
  // velha nao pode escrever na tela da carga nova. Sem isso, o aviso ficava aceso pela ordem em que
  // os `finally` calham de rodar — que hoje funciona e nao e garantia de nada.
  async function tailComRetentativa(signal: AbortSignal, g: number) {
    try {
      return await getHistory(sessionName, TAIL_FIRST, signal, TAIL_TIMEOUT_1);
    } catch (err) {
      // So o TETO justifica repetir. Cancelamento (troca de sessao, /clear) e erro do servidor
      // (404/500) sobem: repetir os dois seria pedir de novo o que ja falhou de verdade.
      if (!isTimeoutError(err)) throw err;
      if (g === histGen) histRetentando = true;
      try {
        return await getHistory(sessionName, TAIL_FIRST, signal, TAIL_TIMEOUT_2);
      } finally {
        if (g === histGen) histRetentando = false;
      }
    }
  }

  async function loadHistory() {
    const signal = newHistLoad();
    const g = histGen;
    histGap = '';
    histRetentando = false;   // carga nova comeca sem o aviso da anterior, igual ao histGap
    try {
      const tail = await tailComRetentativa(signal, g);
      if (g !== histGen) return;   // outra carga assumiu no meio do voo: esta resposta é velha
      events = tail;
      rebuildIndex();
      reseedDerived();
      error = '';
      kimiSemTranscript = false;   // transcript existe -> sai do modo "kimi pre-1o-prompt"
      // Veio menos que o pedido = o transcript inteiro coube na cauda; não há o que buscar.
      if (tail.length >= TAIL_FIRST) loadOlderInBackground(g);
    } catch (err) {
      if (isAbortError(err) || g !== histGen) return;   // cancelado ≠ falhou: nada na tela
      // Teto estourado vira frase traduzida: o texto que o navegador poe no TimeoutError e
      // "signal timed out", que nao diz nada pra quem le a tela (mesma troca que o
      // apiFetchForServer ja faz em lib/api.ts).
      const msg = isTimeoutError(err) ? m.chat_historico_sem_resposta()
        : err instanceof Error ? err.message : m.chat_erro_carregar_historico();
      // Kimi pre-1o-prompt: o 404 do /history e ESPERADO (sem jsonl ainda) -> vira hint, nao a
      // tela de erro "Não encontrei o transcript" (que apavorava num estado que e por design).
      // Pelo `.status` que o apiFetch anexa, NAO por regex na frase do backend: o texto do detail
      // e prosa (traduzir/reescrever ele apagaria esta tela sem quebrar teste nenhum). Mesmo
      // padrao do 409 do terminal, linha ~1324, e do Composer.
      if (sessionProvider === 'kimi' && (err as { status?: number } | null)?.status === 404) {
        kimiSemTranscript = true;
        return;
      }
      error = msg;
    } finally {
      if (g === histGen) loading = false;
      // Conversa na tela (ou desistimos dela): o trabalho especulativo pode correr. Vale também no
      // ramo de ERRO — histórico que falhou não é motivo pra a pílula de modelo ficar sem catálogo.
      // Sem o `if`: uma carga abortada é sempre sucedida por outra, que solta na vez dela.
      if (g === histGen) soltarAquecimento(sessaoDoPortao);
    }
  }

  // Fase 2: o histórico ANTERIOR à cauda, em segundo plano. Não devolve promise de propósito —
  // ninguém espera por ela, a tela já está utilizável. Anda junto com a carga da geração `g`: usa o
  // MESMO controller (não cria um novo), então quem invalida a geração aborta as duas fases.
  function loadOlderInBackground(g: number) {
    getHistory(sessionName, undefined, histAbort?.signal)
      .then((full) => {
        if (g !== histGen || !alive) return;   // resposta velha/pós-destroy: NÃO aplica
        // prependOlder só ACRESCENTA o que é mais antigo que a nossa primeira bolha: o que o SSE
        // entregou durante o fetch fica intacto, e nada que o dedup removeu volta.
        const merged = prependOlder(full, events);
        if (!merged) {
          // null tem dois motivos e só um é problema: sem ponto de costura a conversa segue
          // truncada (avisa); "já temos desde o começo" é o caso feliz (silêncio).
          histGap = hasSeam(full, events) ? '' : 'unjoinable';
          return;
        }
        events = merged;
        rebuildIndex();
        reseedDerived();
        histGap = '';
      })
      .catch((err) => {
        if (isAbortError(err) || g !== histGen || !alive) return;   // cancelado ≠ falhou
        histGap = 'failed';
      });
  }

  // Watchdog de liveness: o backend manda um evento 'ping' a cada 10s. 25s sem NADA (msg/state/ping)
  // = conexao morta sem aviso (half-open: mobile trocou de rede / app no background / backend caiu).
  // O EventSource.onerror NAO dispara em half-open -> sem isto o front congela no ultimo estado.
  function armWatchdog() {
    clearTimeout(watchdog);
    // Mesmo guard do onerror: sessao 'dead' nao ganha reconexao infinita de 25s (o estado final
    // ja chegou; reviver e acao do usuario via resume, nao do watchdog).
    watchdog = setTimeout(() => {
      // Conexão MEIO-ABERTA: 25s sem um único evento, nem o ping de 10s do backend. É o caso que
      // não dispara `onerror` e que, sem registro, some sem deixar rastro.
      diag.registrar({ evento: 'sse.mudo', nivel: 'aviso', tela: 'chat', sessao: sessionName,
                       ms: 25000 });
      if (currentState !== 'dead') connectSSE();
    }, 25000);
  }
  // Qualquer evento recebido = conexao viva: rearma o watchdog E zera o backoff do onerror.
  function noteAlive() {
    sseRetryDelay = SSE_RETRY_MIN;
    armWatchdog();
  }
  // Backoff do reconnect por erro (3s -> 30s). Auditoria: sem isto, com a VPN caida, o retry
  // nativo do EventSource + o setTimeout de 3s martelavam ~2 conexoes a cada 3s pra sempre.
  const SSE_RETRY_MIN = 3000;
  const SSE_RETRY_MAX = 30000;
  let sseRetryDelay = SSE_RETRY_MIN;
  // Componente vivo? connectSSE pos-destroy criava EventSource FANTASMA (watchdog proprio,
  // reconectando pra sempre, nada nunca fecha) — 1 leak por ciclo background->foreground->navegar.
  let alive = false;

  function connectSSE() {
    if (!alive) return;
    // Kimi pre-1o-prompt: /events 404 (sem jsonl) -> nao conecta ate o flip tracked (efeito mais
    // abaixo dispara). Sem este guard o onerror virava retry com backoff martelando pra sempre um
    // endpoint que so passa a existir depois do primeiro envio.
    if (kimiPreNascimento) return;
    clearTimeout(reconnectTimer);
    if (es) { es.close(); es = null; }

    es = openEventStream(sessionName, lastEventId);
    // Ciclo de vida da conexão no diário de uso. É o que faltava nos relatos de "a conversa parou"
    // e "as sessões sumiram": sem isto não dá pra distinguir queda de rede, reconexão em laço e
    // conexão viva com a lista congelada, e a análise vira chute.
    diag.registrar({ evento: 'sse.abrir', tela: 'chat', sessao: sessionName,
                     provider: sessionProvider });
    armWatchdog();

    es.addEventListener('message', (e: MessageEvent) => {
      noteAlive();
      // Chegou conversa: o aviso de "não carregou o histórico" não pode continuar na frente dela.
      // A tela de erro SUBSTITUI a lista inteira ({:else if error}), então um erro aceso por uma
      // carga que falhou ficava preso mesmo depois de o SSE se recuperar sozinho e voltar a
      // entregar mensagens — o mesmo sintoma que este trabalho veio consertar, entrando por outra
      // porta. Quem apagava o aviso era só o toque em "tentar de novo".
      if (error) error = '';
      // Guarda a posição de retomada. Só o transcript carrega id ("<stem>:<offset>"); state/preview/
      // ping vêm sem, de propósito — o último id visto tem que ser sempre o do transcript, senão a
      // retomada apontaria pro lugar errado e pularia mensagens.
      if (e.lastEventId) lastEventId = e.lastEventId;
      try {
        const ev = JSON.parse(e.data) as ChatEvent;
        // Dedup cruzado fila<->transcript: a fila duravel emite user_msg sintetico (id "queued-").
        // Quando o Claude Code grava o prompt real, chega o user_msg real -> tira o sintetico de
        // mesmo texto (por linha, pq ele pode fundir varias). E nao adiciona sintetico se o real
        // ja existe. (covers: a "cobre" b se forem iguais, b for linha de a, ou b for prefixo
        // de linha de a — o eco com sufixo, ver lib/covers.ts.)
        if (ev.kind === 'user_msg' && ev.text) {
          // Textos das bolhas da fila que ja estao na tela — as candidatas a dona de uma linha do
          // transcript. Cobrir nao basta: quem sai da tela e a DONA (ver lib/covers.ts).
          const filas: { i: number; text: string }[] = [];
          for (let i = 0; i < events.length; i++) {
            const x = events[i];
            if (x.kind === 'user_msg' && x.id.startsWith('queued-') && x.text) filas.push({ i, text: x.text });
          }
          if (ev.id.startsWith('queued-')) {
            // Dedup INTEGRAL (todos os events): o follow re-emite a fila INTEIRA a cada reconexao;
            // limitar a janela (tentado em 2026-07-02) deixava entradas antigas escaparem e
            // aparecerem soltas no fim do chat. O falso-positivo raro (um "ok" antigo engolindo a
            // bubble de um "ok" novo na fila) e o custo aceito — cosmetico e a entrega nao muda.
            // O sintetico so e engolido se ELE for o dono da linha real: se outra bolha da fila a
            // reivindica de forma mais especifica, a linha e dela e esta aqui continua pendente
            // (senao o aviso "não chegou" da mais curta sumia pela linha da mais longa).
            const candidatos = [...filas.map((f) => f.text), ev.text];
            if (events.some((x) => x.kind === 'user_msg' && !x.id.startsWith('queued-') && x.text
                  && especificidade(x.text, ev.text!) >= 0
                  && donoDaLinha(x.text, candidatos) === candidatos.length - 1)) {
              return; // real ja cobre este texto, e ele e o dono -> ignora o sintetico
            }
          } else {
            // Remove SO a bolha DONA desta linha (nao todas, e nao a 1a que casar): com duas "ok"
            // na fila e uma real commitada, a 2a continua pendente e visivel.
            const dono = donoDaLinha(ev.text, filas.map((f) => f.text));
            if (dono >= 0) {
              const qi = filas[dono].i;
              events = [...events.slice(0, qi), ...events.slice(qi + 1)];
              rebuildIndex();
            }
          }
        }
        // Dedupe by id: the SSE replays the whole transcript on every (re)connect and
        // loadHistory() also seeds events — without this, messages double up and the
        // keyed {#each} chokes on duplicate ids.
        const i = idIndex.get(ev.id);
        if (i !== undefined) {
          const next = events.slice();
          next[i] = ev;
          events = next;
        } else {
          idIndex.set(ev.id, events.length);
          events = [...events, ev];
          // Folds incrementais: evento NOVO alimenta o painel de atividade e o contador de
          // assistant_msg (replaces do replay não passam aqui -> não contam dobrado).
          if (ev.kind === 'tool_use' || ev.kind === 'tool_result') {
            actFolder.push(ev);
            activity = actFolder.snapshot();
          } else if (ev.kind === 'assistant_msg' && ev.text) {
            asstCount += 1;
            // Swap preview->bolha ATOMICO: o bloco real entra SEM animacao (swapIds) e o preview
            // zera AQUI, sincrono, no mesmo flush do append -> UM paint so, sem frame vazio nem
            // texto duplicado. A bolha nasce no mesmo y do preview: o que ja foi lido nao se move.
            // (Antes: append num flush + limpeza do preview num $effect pos-render = 2 repaints,
            // bolha re-animando e scroll pulando — o usuario perdia o ponto da leitura.)
            if (previewText) {
              swapIds.add(ev.id);
              cancelPreviewDrop();
              previewText = '';
            }
          }
        }
      } catch {}
    });

    es.addEventListener('state', (e: MessageEvent) => {
      noteAlive();
      try {
        stateEvent = JSON.parse(e.data) as StateEvent;
        // Turno acabou sem bloco de assistente (só ferramentas, ou interrompido): ninguém mais viria
        // apagar a prévia, porque o "" deixou de apagá-la enquanto working (ver o handler de
        // preview). Sair de `working` é o outro dono — mas via CARÊNCIA (dropPreviewSoon), nunca
        // na hora: o assistant_msg do .jsonl chega DEPOIS deste evento, e zerar aqui era o pisca
        // (prévia some -> buraco -> bolha volta re-animando).
        if (stateEvent?.state === 'working') cancelPreviewDrop();
        else if (previewText) dropPreviewSoon();
        // Pergunta respondida em OUTRO aparelho (ou direto no terminal): o pane sai do
        // `awaiting_input` e ninguem mais fechava o stepper AQUI — ele ficava na tela pedindo
        // resposta de algo ja respondido. Pergunta de Pi/Kimi tem dono proprio (o $effect do
        // `pendingPiQuestion`, que fecha pelo tool_result) e o estado do pane dela nao segue essa
        // regra -> so o caso do Claude, que abre pelo evento SSE.
        if (askOpen && !askPiId && stateEvent?.state !== 'awaiting_input') askOpen = false;
      } catch (err) {
        // Mesmo motivo do handler de `preview` logo abaixo: engolir aqui congela a prévia na tela
        // (este handler virou o OUTRO dono dela) e ainda deixa o `stateEvent` preso no valor
        // antigo. O erro não pode derrubar o SSE, mas tem que dar pra ver no dev.
        if (import.meta.env.DEV) console.debug('state: evento ilegivel', err);
      }
    });

    // Faixa de estatísticas da sessão (app/stats.py). Full-replace; ausência de evento = sem faixa.
    es.addEventListener('stats', (e: MessageEvent) => {
      try { statsEvent = JSON.parse(e.data) as StatsEvent; } catch {}
    });

    // Heartbeat do backend: so prova de vida (reseta o watchdog numa conexao ociosa, sem msgs).
    es.addEventListener('ping', () => noteAlive());

    // Stepper nativo AskUserQuestion: abre o sheet com as perguntas recebidas via SSE
    es.addEventListener('ask_question', (e: MessageEvent) => {
      try { askPayload = JSON.parse(e.data); askOpen = true; } catch {}
    });

    // Preview ao vivo (best-effort) do bloco de assistente em voo. Full-replace; tambem e prova de
    // vida (mas NAO a unica — entre turnos nao ha preview, por isso o ping ancora o watchdog).
    es.addEventListener('preview', (e: MessageEvent) => {
      noteAlive();
      try {
        const ev = JSON.parse(e.data) as { text?: string; md?: boolean; full?: boolean };
        const t = ev.text ?? '';
        // Guard de monotonicidade: frame TRANSITORIO do pane (mid-redraw) as vezes chega como
        // PREFIXO do texto ja mostrado -> ignorar, senao o texto recua e re-cresce (stuttering).
        // Vazio (drop) e conteudo realmente novo passam.
        // O guard so vale DENTRO da mesma fonte. Quando `md` ou `full` vira (a extensao do agente
        // caiu no meio do turno e a previa voltou pro pane, ou a costura recomecou e o frame de
        // troca veio com full=False), o texto novo costuma ser MENOR e prefixo do anterior — e
        // descartar esse evento congelaria a bolha numa fonte que ja nao existe, sem nenhum sinal.
        // Troca de fonte passa sempre.
        if (t && !!ev.md === previewMd && !!ev.full === previewFull
            && t.length < previewText.length && previewText.startsWith(t)) return;
        // VAZIO enquanto a sessão TRABALHA não apaga a bolha. Medido em 11/08/2026, 200ms de
        // amostragem numa sessão Claude: entre uma ferramenta e outra o extrator não acha prosa
        // no pane e manda "", a bolha desmontava, e cada ciclo tirava e repunha ~53px (o maior,
        // 139px) da altura da lista — 7 ciclos em 18s. Como a conversa fica ancorada no fim, tudo
        // o que estava sendo lido subia e descia junto: o "pulo".
        // Não vira bolha fantasma porque quem apaga a prévia de verdade são os DOIS donos que já
        // existem: o `assistant_msg` real (swap atômico, ~30 linhas acima) e a saída de `working`
        // (logo abaixo, no handler de state). O "" só perdeu o papel de terceiro dono.
        if (!t) {
          // "" do Stop com o estado já idle: mesma carência do handler de state — o bloco real
          // ainda está a caminho pelo tail do .jsonl.
          if (stateEvent?.state !== 'working') dropPreviewSoon();
          return;
        }
        cancelPreviewDrop();
        previewText = t;
        previewMd = !!ev.md;
        previewFull = !!ev.full;
      } catch (err) {
        // Engolir aqui congela a previa (texto E flag) no ultimo frame bom, sem rastro nenhum. O
        // erro nao pode derrubar o handler do SSE, mas tem que dar pra ver no dev.
        if (import.meta.env.DEV) console.debug('preview: evento ilegivel', err);
      }
    });

    // Reset de sessao (ex: /clear): o backend trocou de transcript. O dedup-por-id NAO limparia as
    // bolhas antigas (ids diferentes) -> zera tudo e recarrega o history do jsonl novo (vem limpo).
    es.addEventListener('reset', () => {
      noteAlive();
      // No diário também, não só no journal do servidor: o journal só existe no Linux, e este
      // evento é o único que APAGA a conversa da tela — sem ele registrado, "ficou vazio" e "nunca
      // carregou" são indistinguíveis no arquivo que a pessoa manda.
      diag.registrar({ evento: 'chat.reset', tela: 'chat', sessao: sessionName });
      lastEventId = null;   // transcript trocado (/clear): id do arquivo antigo não vale mais
      events = [];
      idIndex.clear();
      reseedDerived();          // zera activity/asstCount junto (loadHistory re-semeia com o novo)
      cancelPreviewDrop();
      previewText = '';
      stateEvent = null;
      statsEvent = null;      // transcript novo -> a faixa zera junto (o backend recomeça o fold)
      loadHistory();
    });

    es.onerror = () => {
      // Erro REAL (TCP RST). FECHA o es (senao o auto-retry nativo vira uma 2ª maquina de retry
      // martelando em paralelo) e reagenda com backoff. Half-open nao cai aqui -> watchdog cobre.
      const estadoSSE = es?.readyState ?? 2;   // lido ANTES do close, que o zera pra CLOSED
      es?.close(); es = null;
      clearTimeout(watchdog);
      // `readyState` é o que separa os dois motivos que dão o mesmo `onerror`: CONNECTING (0) = o
      // navegador está tentando de novo (rede caiu, servidor reiniciou); CLOSED (2) = o servidor
      // recusou de vez (401, 404, sessão morreu). Sem ele o diário registrava "caiu" e a análise
      // parava aí.
      const motivo = estadoSSE === 0 ? 'reconectando (rede/servidor)' : 'fechado pelo servidor';
      diag.registrar({ evento: 'sse.caiu', nivel: 'erro', tela: 'chat', sessao: sessionName,
                       codigo: String(estadoSSE), ms: sseRetryDelay, detalhe: motivo });
      if (currentState === 'dead' || !alive) return;
      clearTimeout(reconnectTimer);
      reconnectTimer = setTimeout(connectSSE, sseRetryDelay);
      sseRetryDelay = Math.min(sseRetryDelay * 2, SSE_RETRY_MAX);
    };
  }

  // App voltou pro foreground (mobile suspende a conexao no background). Agora o backfill do SSE so
  // traz o TAIL (ultimas _BACKFILL_LINES linhas), entao um background LONGO pode ter perdido mais que
  // isso. Re-seed do history (REST, completo e ordenado) ANTES de reconectar fecha o buraco; o backfill
  // tail do SSE so faz a ponte ate a subscricao (dedup por id, sem reordenar). Falha aqui NAO trava a
  // tela (o connectSSE/onerror re-sincroniza) -> ignora e segue. Reconexoes de blip (watchdog/onerror)
  // continuam SO com o tail-K: cobrem poucos segundos sem re-shippar o arquivo inteiro.
  async function onVisible() {
    if (document.visibilityState !== 'visible') return;
    // Segura watchdog/retry DURANTE o re-seed: no wake do iOS o watchdog vencido disparava um
    // connectSSE proprio e o onVisible outro logo atras — 2 reconexoes + replay em toda volta.
    clearTimeout(watchdog);
    clearTimeout(reconnectTimer);
    sseRetryDelay = SSE_RETRY_MIN;   // rede provavelmente voltou: reconexao rapida de novo
    const signal = newHistLoad();   // aborta a carga de fundo que ficou pendurada no background
    const g = histGen;
    try {
      // So a CAUDA: o buraco do background e no FIM da conversa, e o historico antigo ja esta em
      // memoria — re-baixar o jsonl inteiro a cada volta pro foreground era o custo que sobrava.
      const fresh = await getHistory(sessionName, TAIL_FIRST, signal);
      if (g !== histGen || !alive) return;   // resposta velha/pos-destroy: NAO sobrescreve nem conecta
      const head = events[0]?.id;
      events = appendTail(fresh, events);
      rebuildIndex();
      reseedDerived();
      // Sem sobreposicao a appendTail re-ancorou na cauda (background longo demais): o historico
      // antigo saiu da lista e volta em segundo plano, como na abertura.
      if (events[0]?.id !== head) loadOlderInBackground(g);
    } catch (err) {
      // Com bolha na tela, seguir calado está certo: é um blip, e o SSE re-sincroniza.
      // Com a lista VAZIA, não: o `connectSSE` abaixo retoma pelo `lastEventId`, e quando um
      // `reset` acabou de zerar a lista esse id também foi zerado — não há de onde retomar e
      // ninguém mais recarrega. O resultado era a tela sem uma bolha, sem aviso e sem o botão de
      // tentar de novo, até sair da sessão e voltar (relatado em 26/08/2026).
      if (!isAbortError(err) && g === histGen && alive && events.length === 0) {
        error = isTimeoutError(err) ? m.chat_historico_sem_resposta()
          : err instanceof Error ? err.message : m.chat_erro_carregar_historico();
      }
    }
    if (g !== histGen || !alive) return;
    connectSSE();
  }

  onMount(async () => {
    alive = true;
    await loadHistory();
    // Pos-await: o componente pode ter morrido durante o loadHistory (troca rapida de sessao via
    // {#key}). Sem o guard, o addEventListener rodava DEPOIS do removeEventListener do destroy ->
    // listener orfao preso pra sempre fazendo getHistory fantasma a cada visibilitychange.
    if (!alive) return;
    connectSSE();
    document.addEventListener('visibilitychange', onVisible);
  });

  onDestroy(() => {
    alive = false;   // connectSSE/onVisible em voo viram no-op — sem EventSource fantasma
    histGen++;
    histAbort?.abort();   // e o /history em voo para de baixar (nao so de ser aplicado)
    es?.close();
    clearTimeout(watchdog);
    clearTimeout(reconnectTimer);
    cancelPreviewDrop();
    document.removeEventListener('visibilitychange', onVisible);
  });

  // Layout teclado-safe: a .chat-screen acompanha a ALTURA da viewport visivel. Quando o
  // teclado abre, vv.height encolhe -> o container encolhe pra area acima do teclado, com a
  // NavBar colada no topo e o composer no rodape (ambos flex-shrink:0) e a MessageList (flex:1)
  // como UNICO scroller. offsetTop compensa o pan do iOS (senao o composer some pro topo).
  $effect(() => {
    const vv = window.visualViewport;
    if (!vv || !screenEl) return;
    // Dentro do modal do par a tela NÃO é a viewport: o `fit` fixava height=vv.height (900px medidos)
    // num modal de 858 e a última linha do composer ficava cortada. Lá quem manda é a altura do
    // modal (CSS 100%), e o teclado é problema do modal, como já é em qualquer sheet.
    // Desktop: nao ha teclado virtual, entao este fit nunca precisou rodar aqui — mas RODAVA, e
    // gravava screenEl.style.height = vv.height (a viewport INTEIRA), sobrepondo o "height: 100%"
    // que faz a tela acompanhar a pane (que encolhe quando o TerminalPanel abre no rodape do
    // DesktopShell). Resultado: o composer ficava atras do painel de terminal, clipado pelo
    // overflow:hidden da pane. Mesma classe de bug do modal do par, mesmo remedio.
    if (nested || desktop) return;
    function fit() {
      if (!screenEl || !vv) return;
      // Ignora valores transientes (a animacao do teclado reporta alturas minusculas por 1 frame).
      if (vv.height < 120) return;
      const h = vv.height + 'px';
      // offsetTop = quanto o iOS PANEIA a visual viewport ao abrir o teclado (body travado -> e pan
      // VISUAL). Compensamos via `top` em position:relative (sem transform: nao promove layer com
      // tiled-backing -> SEM retangulo preto; nao cria containing-block que prenda os sheets fixed).
      // EXPERIMENTO teclado iOS (#1): o pan (offsetTop) e bugado no iOS 26 (Apple #800125) e deixava o
      // composer com um vao acima do teclado. Mata o pan (scrollTo 0) e ancora top=0 -> a tela passa a
      // ser SO a altura visivel (vv.height), com o dock colado no rodape dela = topo do teclado.
      // Guard: so scrolla se houver scroll REAL. scrollTo a cada evento do viewport (toda tecla)
      // disparava o dialog "Desfazer" (shake-to-undo) do iOS toda hora.
      if (window.scrollY !== 0) window.scrollTo(0, 0);
      if (screenEl.style.height !== h) screenEl.style.height = h;
      if (screenEl.style.top !== '0px') screenEl.style.top = '0px';
      if (screenEl.style.transform) screenEl.style.transform = '';
      // Cola mais o composer no teclado: aberto -> zera o padding-bottom de safe-area (home indicator,
      // inutil com teclado) que deixava um vao; fechado -> volta a safe-area (fallback do --composer-pb).
      if (vv.height < window.innerHeight - 100) screenEl.style.setProperty('--composer-pb', 'var(--space-2)');
      else screenEl.style.removeProperty('--composer-pb');
    }
    function onFocusIn() {
      requestAnimationFrame(fit);
      setTimeout(fit, 300); // iOS as vezes so estabiliza apos a animacao do teclado
    }
    // iOS 26: offsetTop/height as vezes NAO zeram ao fechar o teclado. No blur sem outro campo focado,
    // forca estado limpo (senao sobra um vao no rodape).
    function onFocusOut() {
      setTimeout(() => {
        if (!screenEl) return;
        const a = document.activeElement;
        if (a && (a.tagName === 'TEXTAREA' || a.tagName === 'INPUT')) return;
        screenEl.style.top = '0px';
        screenEl.style.height = '';   // volta pro height do CSS (100vh)
        screenEl.style.transform = '';
      }, 50);
    }
    fit();
    vv.addEventListener('resize', fit);
    vv.addEventListener('scroll', fit);
    screenEl.addEventListener('focusin', onFocusIn);
    screenEl.addEventListener('focusout', onFocusOut);
    return () => {
      vv.removeEventListener('resize', fit);
      vv.removeEventListener('scroll', fit);
      screenEl?.removeEventListener('focusin', onFocusIn);
      screenEl?.removeEventListener('focusout', onFocusOut);
    };
  });

  // Mede a altura do dock (composer) e expoe via prop pra lista. ResizeObserver dispara SO quando
  // o composer muda de tamanho (anexo, multilinha, botao stop) — NAO na animacao do teclado
  // (composer mantem a altura) — entao nao reintroduz o glitch da NavBar.
  $effect(() => {
    if (!dockEl) return;
    let raf = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        if (!dockEl) return;
        const h = Math.round(dockEl.getBoundingClientRect().height);
        if (Math.abs(h - dockH) > 2) dockH = h;
      });
    });
    ro.observe(dockEl);
    return () => { cancelAnimationFrame(raf); ro.disconnect(); };
  });

  // A altura do dock e medida aqui, mas quem precisa dela tambem vive FORA desta arvore (a barra de
  // TTS e montada no App.svelte, que sobrevive a remontagem do Chat). Publicar na raiz e o que da
  // a ela o mesmo valor medido, em vez de um numero fixo que nao acompanha composer multi-linha nem
  // teclado do celular.
  $effect(() => {
    document.documentElement.style.setProperty('--cp-dock-h', `${dockH}px`);
  });

  // Mede a navbar (overlay) -> --nav-h, pra lista clarear a 1a msg e rolar por baixo. Igual ao dock.
  $effect(() => {
    if (!navEl) return;
    let raf = 0;
    const ro = new ResizeObserver(() => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        if (!navEl) return;
        const h = Math.round(navEl.getBoundingClientRect().height);
        if (Math.abs(h - navH) > 2) navH = h;
      });
    });
    ro.observe(navEl);
    return () => { cancelAnimationFrame(raf); ro.disconnect(); };
  });

  // Legenda canonica de uma msg (sem o marcador "📎 imagem:/arquivo: <path>" + o "—" que liga). Pro
  // dedup de pending/fila: o eco local carrega o marcador, mas o transcript grava SO a legenda -> sem
  // normalizar, msg COM ANEXO nunca casava e ficava pendente pra sempre.
  function _cap(text: string): string {
    const i = text.search(/(?:\s*—\s*)?📎\s*(?:imagem|arquivo):/u);
    return (i >= 0 ? text.slice(0, i) : text).trim();
  }

  // Quantas msgs estão ESPERANDO o turno atual — o número do chip de fila do Kimi. São as bolhas
  // translúcidas, e elas vêm de DUAS fontes: o eco local (`pending`, de quem acabou de mandar deste
  // aparelho) e o evento sintético "queued-" da fila durável do backend (visível em TODO cliente).
  // Medido em 14/08/2026: um envio pelo app vira "queued-" em ~1s e o dedup ali embaixo REMOVE o
  // pending correspondente — contar só o `pending` dava 0 com a bolha na tela e o chip nunca
  // aparecia. `desistiu` fora: aquela não está na fila, está perdida (a TUI engoliu as teclas).
  // Duas travas de propósito: (1) só Kimi — é o único provider com o chip, e sem isto TODA sessão
  // pagava um scan O(n) sobre `events` a cada evento novo do SSE (o arquivo já trocou o
  // `deriveActivity` por fold incremental pelo mesmo motivo); (2) `kind === 'user_msg'` — o prefixo
  // "queued-" tem DOIS produtores no backend: a fila durável (`pqueue.py`, user_msg) e o aviso de
  // subagente que terminou (`transcript.py`, `queued-task:<id>`, tool_result). Sem o kind, um
  // agente de fundo terminando contaria como mensagem na fila.
  const filaCount = $derived(
    sessionProvider !== 'kimi'
      ? 0
      : pending.length
        + events.filter((e) => e.kind === 'user_msg'
                          && e.id?.startsWith('queued-') && !e.desistiu).length,
  );

  // "mandar agora" (chip da fila): o ctrl-s promove a fila da TUI pro turno em curso. Com
  // promoted=true o backend JÁ baixou a fila durável — tira as bolhas "queued-" na hora, porque o
  // user_msg real só é gravado no wire no FIM do turno (medido: ~34s depois do ctrl-s) e até lá
  // nada mais derrubaria o chip: ele ficava aceso e clicável o turno inteiro sobre um no-op.
  async function steerAgora(): Promise<void> {
    const r = await steerSession(sessionName);
    if (!r.promoted) return;
    events = events.filter((e) => !(e.kind === 'user_msg' && e.id?.startsWith('queued-')));
    rebuildIndex();
  }

  let pendingSeq = 0;

  // Toggle "mandar pros dois" (pareada): quando ligado, o prompt vai pra ESTA sessão E pro par
  // via /broadcast (mesma esteira do /input por sessão, fila durável). Fica ligado até o usuário
  // desligar — mas RESETA quando o par muda (troca/despareamento): sem isto, desparear de B e
  // parear com C reacendia o toggle invisível e o próximo envio ia pra C sem o usuário pedir.
  let sendToPair = $state(false);
  $effect(() => {
    void pairPeersKey;
    sendToPair = false;
  });

  async function handleSend(text: string) {
    // Eco imediato SEMPRE (não só em 'working'): o transcript só grava a msg quando o TURNO dela
    // começa — sessão ocupada num turno longo deixava a msg invisível por minutos, e a corrida de
    // estado (flip idle->working no instante do envio) derrubava até o eco condicional antigo
    // ("mandei e sumiu"). O dedup abaixo reconcilia contra o evento real em qualquer estado.
    const pendingId: string | null = `pending-${pendingSeq++}`;
    pending = [...pending, { id: pendingId, text }];
    try {
      if (sendToPair && pairPeers?.length && !text.trimStart().startsWith('/')) {
        // Slash-command nunca em broadcast (o backend rejeita; mesmo racional do /api/broadcast).
        // /broadcast responde 200 com resultado POR sessão — falha individual (pane de membro
        // morto) não rejeita a promise; sem conferir, o envio pro grupo falhava calado.
        const results = await broadcast([sessionName, ...pairPeers], text);
        const failed = Object.entries(results).filter(([, r]) => !r.ok);
        if (failed.length) {
          const ok = Object.keys(results).filter((n) => results[n].ok);
          throw new Error(
            `${ok.length ? m.chat_chegou_mas({ n: ok.join(', ') }) : ''}${m.chat_nao_chegou_em()}` +
            `${failed.map(([n]) => n).join(', ')} (${formataErro(failed[0][1].error) ?? m.board_falha_envio()})`
          );
        }
      } else {
        await sendInput(sessionName, text);
      }
    } catch (err) {
      console.error('sendInput error:', err);
      // Falhou o envio -> remove o pending que adicionamos (nao ficou enfileirado).
      if (pendingId) pending = pending.filter((p) => p.id !== pendingId);
      throw err; // propaga pro Composer mostrar o erro e NAO limpar o input
    }
  }

  // Dedup: solta o pending quando o transcript (SSE) traz a msg real. Casa por texto NORMALIZADO
  // e tambem por LINHA — o Claude Code funde varias msgs enfileiradas numa so (separadas por \n),
  // entao "msg1\nmsg2" casa tanto "msg1" quanto "msg2". Idempotente (length estabiliza).
  $effect(() => {
    if (pending.length === 0) return;
    const committed = new Set<string>();
    for (const e of events) {
      if (e.kind !== 'user_msg' || !e.text) continue;
      const t = e.text.trim();
      committed.add(t);
      committed.add(_cap(t));                       // legenda canonica (msg com imagem grava so ela)
      for (const line of t.split('\n')) committed.add(line.trim());
    }
    // Remove o eco quando o texto cru OU a legenda (sem "📎 imagem: <path>") ja commitou. Legenda
    // vazia (imagem sem texto) nao casa por texto -> cai no solidify do idle, nao trava aqui.
    const next = pending.filter((p) => {
      const cap = _cap(p.text);
      return !committed.has(p.text.trim()) && !(cap && committed.has(cap));
    });
    if (next.length !== pending.length) pending = next;
  });

  // Solidificar a fila: msgs enviadas enquanto o Claude trabalha NEM sempre viram entrada no
  // transcript dele (so as enviadas com ele ocioso viram um prompt gravado). Entao nao da pra
  // apagar o eco (sumiria — era o bug). Quando ele volta a idle (consumiu a fila), SOLIDIFICA o
  // pending no lugar: vira bubble normal (sem opacidade), parte do fluxo. O reconcile acima ainda
  // remove os que casarem com o transcript (evita duplicar quando o Claude Code de fato grava).
  let prevState: State = 'idle';
  $effect(() => {
    const s = currentState;
    if (prevState !== 'idle' && s === 'idle' && pending.some((p) => !p.solid)) {
      pending = pending.map((p) => ({ ...p, solid: true }));
    }
    prevState = s;
  });

  // Dropa o preview quando: (a) um NOVO bloco de assistente COMMITA (vira bubble real), ou (b) sai de
  // working (turn acabou / só-tool / interrompido). (a) é o sinal timing-safe: o FRONT sabe o que já
  // mostrou — não depende de QUANDO o texto cai no .jsonl. Quando o bloco vira bubble, o preview dele
  // não é mais necessário e some; reaparece sozinho quando o PRÓXIMO bloco começa a streamar (o broker
  // reemite). Mata a duplicata (preview + bubble do mesmo bloco) na raiz, sem comparar texto.
  // Tira crase E marcadores de markdown (* _ ~ # >): o preview vem do pane JÁ RENDERIZADO (sem
  // markdown), o .jsonl tem o markdown cru -> sem tirar, "**Confirma**" != "Confirma" e o preview
  // duplicado de uma msg com formatação NÃO casava com a commitada (ficava como bolha fantasma).
  // Marcador de lista no início da linha sai junto: a TUI pinta `- item` como `• item` (mesma
  // regra do _norm do backend, em app/preview.py).
  const _norm = (s: string) => s.replace(/^\s*[-•◦▪]\s+/gm, '').replace(/[`*_~#>]/g, '').replace(/\s+/g, ' ').trim();
  // Contador INCREMENTAL de assistant_msg (mantido no handler do SSE + reseedDerived): o effect
  // abaixo rodava um loop no `events` inteiro A CADA frame de preview (~150ms em streaming) só pra
  // detectar um commit novo — O(n) por frame em sessão longa.
  let asstCount = $state(0);
  function countAssts(evs: ChatEvent[]): number {
    let c = 0;
    for (const e of evs) if (e.kind === 'assistant_msg' && e.text) c++;
    return c;
  }
  let _asstSeen = 0;
  $effect(() => {
    // CRÍTICO: ler previewText AQUI no topo, SEMPRE -> em Svelte 5 a dep só é rastreada se LIDA na
    // execução. Se a gente retornasse antes de ler (caminho idle), o effect não re-rodaria quando o
    // broker REEMITISSE o preview no idle -> o tail ficava (a duplicata que não saía). Lendo aqui, o
    // effect re-roda a cada update do preview e limpa.
    const pv = previewText;
    const committed = asstCount > _asstSeen;
    _asstSeen = asstCount;
    if (!pv) return;
    // (a) bloco novo commitou -> dropa na hora (o texto já está na tela como bolha).
    if (committed) { cancelPreviewDrop(); previewText = ''; return; }
    // (b) saiu de working -> CARÊNCIA, não drop imediato (o assistant_msg ainda vem pelo tail;
    // zerar aqui era um dos três donos do pisca de fim de turno). Segue pro (c): se o texto já é
    // bolha, o drop imediato de lá resolve a duplicata sem esperar o timer.
    // `stateEvent &&` porque "ainda não chegou estado nenhum" NÃO é "saiu de working": currentState
    // nasce 'idle' por default, e na abertura da conversa o preview chega ANTES do primeiro evento
    // `state` (o broker publica o texto do pane na inscrição; o state vem no tick seguinte). Sem o
    // guard, abrir a conversa no meio de um turno longo apagava a prévia na hora — e como o broker
    // só reemite em MUDANÇA do pane, um bloco parado (um painel de tarefas, p.ex.) não voltava mais.
    if (stateEvent && currentState !== 'working') dropPreviewSoon();
    // (c) residual coberto por QUALQUER das últimas msgs commitadas (não só a última): entre turnos o
    // pane ainda mostra o bloco anterior como "● tail" e o broker reemite -> dropa se já é bolha.
    const p = _norm(pv);
    if (p.length >= 16) {
      let seen = 0;
      for (let i = events.length - 1; i >= 0 && seen < 6; i--) {
        const e = events[i];
        if (e.kind === 'assistant_msg' && e.text) {
          seen++;
          if (_norm(e.text).includes(p)) { cancelPreviewDrop(); previewText = ''; return; }
        }
      }
    }
  });

  // Slash commands gerais do Claude Code (ex: /clear, /compact) -> sessao viva. Modelo e
  // esforco NAO passam por aqui: vao pelos popovers de modelo/esforco -> endpoint /model-effort.
  async function handleCommand(cmd: string) {
    try {
      await sendInput(sessionName, cmd);
    } catch (err) {
      console.error('sendInput (command) error:', err);
    }
  }

  // Recusa do backend ao responder (opção pelo picker ou pergunta pelo stepper). O caso comum é o
  // 409 do painel de terminal aberto ("Terminal aberto nesta sessao..."): antes o catch só fazia
  // console.error e tocar o botão não fazia ABSOLUTAMENTE NADA, em silêncio — o backend recusava e
  // a tela ficava igual. `err.message` já vem limpo (o `detail` do FastAPI, api.ts:errorDetail),
  // então mostra o texto do servidor: ele explica o motivo E a saída ("Feche o painel pra responder
  // por aqui"). Some sozinho depois de 8s, ou no toque — não é estado, é aviso.
  let avisoErr = $state('');
  let avisoErrTimer: ReturnType<typeof setTimeout> | undefined;

  function mostrarAviso(err: unknown) {
    clearTimeout(avisoErrTimer);
    avisoErr = typeof err === 'string' ? err
      : err instanceof Error ? err.message : m.chat_nao_deu_enviar_resposta();
    avisoErrTimer = setTimeout(() => (avisoErr = ''), 8000);
  }

  // Trava de um envio por vez (mesma do BoardCard): o /select agora le o cursor do picker, corrige
  // e so entao da Enter — dois toques rapidos leriam a mesma tela e se atropelariam no meio.
  let selBusy = $state(false);

  async function handleSelect(option: number) {
    if (selBusy) return;
    selBusy = true;
    clearTimeout(avisoErrTimer);
    avisoErr = '';
    try {
      await selectOption(sessionName, option);
    } catch (err) {
      console.error('selectOption error:', err);
      mostrarAviso(err);
    } finally {
      selBusy = false;
    }
  }

  // Múltipla escolha: enviar o que já foi marcado. Toque em opção só ALTERNA ali (ver
  // terminal_input.submeter_multipla) — sem isto dava pra marcar e não dava pra enviar.
  async function handleSubmitSelected() {
    if (selBusy) return;
    selBusy = true;
    clearTimeout(avisoErrTimer);
    avisoErr = '';
    try {
      await submitSelected(sessionName);
    } catch (err) {
      console.error('submitSelected error:', err);
      mostrarAviso(err);
    } finally {
      selBusy = false;
    }
  }

  async function handleInterrupt() {
    // Ao interromper, o Claude Code MANTEM a msg enfileirada no input -> proximo envio concatenava.
    // Se ha pendente, devolve o texto pro composer (editavel) e remove a bubble; pede clear ao backend
    // (2o Esc) pra limpar o input do terminal. Sem pendente: interrupt simples (sem clear -> sem rewind).
    const last = pending.length ? pending[pending.length - 1] : null;
    if (last) {
      composerText = last.text;
      pending = pending.filter((p) => p.id !== last.id);
    }
    try {
      await interrupt(sessionName, !!last);
    } catch (err) {
      console.error('interrupt error:', err);
    }
  }

  // 409 (mismatch de verificação, ou painel de terminal aberto) ou erro inesperado.
  async function handleAnswer(answers: AnswerItem[]) {
    try {
      const r = await answerQuestions(sessionName, answers);
      // Pergunta do Pi respondida com sucesso: o tool_result ainda demora ~1s pra aterrissar no
      // transcript — sem marcar a dispensa aqui, o sheet REABRIA nessa janela (pergunta ainda
      // pendente + askOpen false).
      if (askPiId) { askPiDismissed = askPiId; askPiId = null; }
      askOpen = false;
      // Plano B do backend: a resposta FOI entregue, mas como texto, e o Escape que fechou o
      // seletor vira "interrompido pelo usuário" em vermelho no transcript. Sem esta linha o
      // vermelho ficava sem legenda e parecia que a resposta tinha se perdido.
      if (r?.fallback) mostrarAviso(m.askq_enviada_como_texto());
    } catch (err) {
      if (askPiId) { askPiDismissed = askPiId; askPiId = null; }
      askOpen = false;
      // O `/answer` TAMBÉM é guardado pelo 409 do painel de terminal (api.py, _recusa_se_painel_
      // aberto). Antes o catch dispensava a pergunta e abria o espelho calado: o usuário perdia o
      // stepper e não ficava sabendo por quê. Mostra o texto do servidor — ele explica a saída.
      mostrarAviso(err);
      // Espelho só quando NÃO é 409: o 409 é recusa deliberada (nada foi digitado no pane) e o
      // espelho é um ModalDialog que cobriria justamente o aviso que diz o que fazer. Nos demais
      // erros o estado é incerto e o espelho continua sendo a saída pra finalizar na mão — como
      // ele tapa a pílula, o aviso segue lá quando o usuário fechar (dentro dos 8s).
      if ((err as { status?: number })?.status !== 409) openMirror();
    }
  }
</script>

<svelte:window onkeydown={onGlobalKey} />

<div
  class="chat-screen"
  class:desktop
  class:split-pane={splitTab}
  class:with-context={desktop && showContextPanel}
  class:with-nav={desktop && navOpen}
  style:--cp-ctx-w={`${ctxPanel.recolhido ? LARGURA_TRILHO : ctxPanel.largura}px`}
  bind:this={screenEl}
  style:--nav-h={navH + topInset + 'px'}
>
  <div class="sr-only" role="status">{stateAnnounce}</div>
  {#if splitTab}
    <!-- Aba fina do split: identidade + estado, nada de breadcrumb/ações (estilo "uma janela só").
         No FLUXO, antes do navbar-mount: conteúdo rola DENTRO do underlay e nunca passa por trás
         dela (a NavBar é overlay e paga esse preço; a aba não precisa). O mount fica vazio e mede
         0 → --nav-h zera e a lista começa logo abaixo da aba. -->
    <div class="split-tab">
      <span class="split-tab-dot" style:background={stateColors[currentState]} aria-hidden="true"></span>
      <span class="split-tab-nome">{sessionName}</span>
      {#if onCloseSplit}
        <button class="split-tab-fechar" onclick={onCloseSplit}
                aria-label={`${m.shell_fechar_painel_de()} ${sessionName}`} title={m.shell_fechar_painel()}>×</button>
      {/if}
    </div>
  {/if}
  <div class="navbar-mount" bind:this={navEl}>
    {#if !splitTab}
    <NavBar title={sessionName} subtitle={desktop ? null : serverLabel || null} showBack={!desktop} onBack={onBack} onTitleTap={desktop ? undefined : openSwitcher} {crumbs} state={desktop ? currentState : undefined} {status} onExpandUsage={() => (usageOpen = true)} limited={stateEvent?.limited ?? false} limitReset={stateEvent?.limit_reset ?? null} onOpenActivity={desktop && hasActivity ? () => (activityOpen = true) : undefined} {activityBadge} {activityRunning} onOpenTerminal={abrirTerminalReal} terminalAlert={tuiOverlay && !mirrorOpen && !xtermOpen && !terminalPanelOpen} onOpenNavegador={desktop ? () => (navOpen = !navOpen) : undefined} onOpenRun={desktop ? () => (runOpen = true) : undefined} {runRunning} onMenu={desktop ? undefined : () => (moreOpen = true)} onOpenAttachments={desktop ? () => (anexosOpen = true) : undefined} working={currentState === 'working'} providerLabel={providerBadge} onProviderTap={isCodex ? () => (limitsOpen = true) : undefined} loopLabel={loopChip?.label ?? null} loopColor={LOOP_TONE_COLOR[loopChip?.tone ?? 'muted']} onLoopTap={() => (loopSheetOpen = true)} />
    {/if}
  </div>

  <!-- LoopSheet FORA do .navbar-mount: no desktop largo o mount fica display:none (a info migra
       pro painel de contexto) e um filho dele sumiria junto. -->
  {#if loopSheetOpen}
    <LoopSheet open={true} sessionName={sessionName} onClose={() => (loopSheetOpen = false)} />
  {/if}

  {#if desktop && showContextPanel}
    <DesktopSessionContext
      toggleExterno={ctxToggleExterno}
      state={currentState}
      stateDetail={stateEvent?.label}
      {status}
      {pairPeers}
      {serverLabel}
      provider={sessionProvider}
      serverId={getActiveId() ?? ''}
      {sessionName}
      {events} {histGap} cwd={planSession?.cwd ?? null}
      onOpenTerminal={abrirTerminalReal}
      terminalAlert={tuiOverlay && !mirrorOpen && !xtermOpen && !terminalPanelOpen}
      onOpenRun={() => (runOpen = true)}
      {runRunning}
      onOpenAttachments={() => (anexosOpen = true)}
      onOpenActivity={hasActivity ? () => (activityOpen = true) : undefined}
      {activityBadge}
      {activityRunning}
      onExpandUsage={() => (usageOpen = true)}
      limited={stateEvent?.limited ?? false}
      limitReset={stateEvent?.limit_reset ?? null}
      working={currentState === 'working'}
      loopLabel={loopChip?.label ?? null}
      loopColor={LOOP_TONE_COLOR[loopChip?.tone ?? 'muted']}
      onLoopTap={() => (loopSheetOpen = true)}
      onProviderTap={isCodex ? () => (limitsOpen = true) : undefined}
      onOpenPair={() => (pairOpen = true)}
      onOpenOrq={() => (orqOpen = true)}
      onOpenPeerChat={nested ? undefined : (peer) => (peerChat = peer)}
      onOpenGit={() => (gitOpen = true)}
      session={planSession}
      {planDetail}
      {planLoading}
      {planError}
    />
  {/if}

  {#if desktop && navOpen}
    <NavegadorPane onClose={() => (navOpen = false)} />
  {/if}

  {#if visorAberto && arquivoAberto}
    <!-- O arquivo aberto (Task 11): cobre SÓ a área da conversa — a árvore continua viva e
         clicável no painel de 264px ao lado (mock 2, sem véu). Quem monta o FileViewer no
         desktop é este Chat; no celular é o próprio FilesPanel (Task 12). Clicar em outro
         arquivo troca o conteúdo sem fechar: o store muda selecionado e o FileViewer recebe o
         path novo. `data-arq-visor` é o marcador ESTÁVEL que o guard de captura do
         DesktopShell procura no Esc (B6): o overlay do board/canvas só fecha depois do visor. -->
    <div class="arq-visor" data-arq-visor>
      <FileViewer
        path={arquivoAberto}
        diff={filesStore.diff}
        conteudo={filesStore.conteudo}
        loading={filesStore.loading}
        onEscopo={(e) => filesStore.trocarEscopo(e)}
        onFechar={fecharVisor}
        onSalvar={(t) => filesStore.salvar(arquivoAberto, t)}
      />
    </div>
  {/if}

  <!-- Underlay da conversa (B5 do parecer): MessageList, pílulas e dock ficam INERTES com o
       visor aberto — Tab não alcança controles escondidos sob o arquivo. O painel de contexto
       (árvore viva) e o próprio visor ficam FORA deste wrapper de propósito. -->
  <div class="chat-underlay" inert={visorAberto}>
  {#if loading}
    <!-- Entrando na sessao: skeleton shimmer (familia Respiracao) enquanto o /history carrega. -->
    <div class="chat-skeleton" aria-label={m.chat_carregando_historico()} aria-busy="true">
      <div class="sk-line sk-r" style="width:46%"></div>
      <div class="sk-line" style="width:82%"></div>
      <div class="sk-line" style="width:64%"></div>
      <div class="sk-line sk-r" style="width:38%"></div>
      <div class="sk-line" style="width:90%"></div>
      <div class="sk-line" style="width:55%"></div>
      {#if histRetentando}
        <!-- A 1a tentativa estourou o teto. Sem esta linha, a 2a rodada era mais esqueleto parado e
             quem olha nao tem como saber se o app desistiu. -->
        <p class="sk-aviso">{m.chat_historico_demorando()}</p>
      {/if}
    </div>
  {:else if kimiPreNascimento}
    <!-- Kimi pre-1o-prompt: NAO e erro — a sessao (id + wire.jsonl) so nasce no primeiro envio.
         O composer la embaixo segue usavel: e justamente ele que faz a sessao nascer (o /input vai
         por tmux, sem jsonl). O flip tracked do poll da lista dispara a carga normal. -->
    <div class="chat-error">
      <p class="chat-error-title">{m.chat_sem_transcript_kimi()}</p>
      <p class="chat-error-hint">{untrackedReason('kimi')}</p>
      <p class="chat-error-hint">{m.chat_envie_primeira_kimi()}</p>
      {#if pending.length}
        <!-- O eco pendente some da tela enquanto o hint substitui a MessageList — sem esta linha
             o 1o envio parecia engolido nos ~5s ate o poll da lista reportar o flip tracked. -->
        <p class="chat-error-hint">{m.chat_mensagem_enviada_kimi()}</p>
      {/if}
    </div>
  {:else if error}
    <div class="chat-error">
      {#if errorInfo.notFound}
        <p class="chat-error-title">{m.chat_nao_achei_transcript()}</p>
        <p class="chat-error-hint">{m.chat_transcript_trocado_1()}<code>/clear</code>{m.chat_transcript_trocado_2()}</p>
      {:else}
        <p class="chat-error-title">{m.chat_nao_carregou_historico()}</p>
        <p class="chat-error-hint">{error}</p>
      {/if}
      <div class="chat-error-actions">
        <button class="retry-btn" onclick={loadHistory}>{m.lista_tentar_novamente()}</button>
        <button class="back-btn-inline" onclick={onBack}>{m.chat_voltar_sessoes()}</button>
      </div>
    </div>
  {:else}
    <MessageList
      {events}
      {stateEvent}
      {pending}
      {sessionName}
      {dockH}
      {swapIds}
      preview={previewText}
      previewMd={previewMd}
      previewFull={previewFull}
      onSelectOption={handleSelect}
      onSubmitSelected={handleSubmitSelected}
      onCancel={handleInterrupt}
      askOpen={isWide && askOpen}
      askPayload={askPayload}
      askActive={askOpen && askPayload != null}
      onAnswer={handleAnswer}
      onAskClose={closeAsk}
      onForward={(t) => (forwardText = t)}
      onOpenSession={onNavigateToChat}
      onOpenOrq={() => (orqOpen = true)}
    />
  {/if}

  {#if histGap && !loading && !error}
    <!-- A cauda carregou, o histórico antigo não. O chat segue utilizável; a falha aparece aqui
         (em vez de rolar pra cima e achar que a conversa começa no meio). Falha de rede convida a
         tocar; transcript trocado NÃO — buscar de novo daria o mesmo, então é só o aviso.
         --cp-tts-h (publicada no App.svelte): soma a altura da barra/pill de TTS quando ela está na
         tela, senão as três pills daqui ficam por baixo dela. -->
    {#if histGap === 'failed'}
      <button class="hist-pill" style:bottom={`calc(${dockH}px + 10px + var(--cp-tts-h, 0px))`} onclick={() => loadOlderInBackground(histGen)}>
        {m.chat_historico_antigo()}
      </button>
    {:else}
      <div class="hist-pill" style:bottom={`calc(${dockH}px + 10px + var(--cp-tts-h, 0px))`}>
        {m.chat_sem_historico_anterior()}
      </div>
    {/if}
  {/if}

  {#if tuiOverlay && !mirrorOpen && !xtermOpen && !terminalPanelOpen}
    <!-- Aviso DESTACADO: ha um painel que SO da pra interagir pela TUI. Pulsa pra chamar atencao;
         tocar abre o espelho. Nao toma a tela (so um banner acima do dock). -->
    <button class="tui-pill" style:bottom={`calc(${dockH}px + 10px + var(--cp-tts-h, 0px))`} onclick={abrirTerminalReal} aria-label={needsLogin ? m.chat_abrir_terminal_login() : m.chat_abrir_terminal_interagir()}>
      <span class="tui-pill-dot"></span>
      <span class="tui-pill-text">{needsLogin ? m.chat_precisa_login() : m.chat_interacao_tui()}</span>
    </button>
  {/if}

  {#if avisoErr}
    <!-- Recusa ao responder — opção do picker ou pergunta do stepper (409 do painel de terminal,
         sessão morta, tmux travado). No centro, acima do dock — é sobre o toque que acabou de
         acontecer, tem que estar no olho. -->
    <button class="aviso-err" style:bottom={`calc(${dockH}px + 10px + var(--cp-tts-h, 0px))`} onclick={() => { clearTimeout(avisoErrTimer); avisoErr = ''; }}>
      {avisoErr}
    </button>
  {/if}

  {#if !desktop && awaitingCount > 0}
    <!-- Triage mobile (feature #4): pula pra proxima sessao aguardando resposta (wrap-around).
         Canto inferior direito (alcance do polegar) pra nao brigar com o tui-pill (centralizado)
         nem cobrir o composer/navbar. Some sozinha quando o contador zera (derived, sem cache). -->
    <button class="awaiting-pill" style:bottom={`calc(${dockH}px + 10px + var(--cp-tts-h, 0px))`} onclick={goNextAwaiting} aria-label={awaitingCount > 1 ? m.chat_aguardando_proxima({ n: awaitingCount }) : m.chat_aguardando_proxima_1({ n: awaitingCount })}>
      {m.chat_aguardando_pill({ n: awaitingCount })}
    </button>
  {/if}

  <div class="bottom-dock" bind:this={dockEl}>
    {#if currentState === 'dead'}
      <div class="dead-footer">
        <p class="dead-text">{m.chat_sessao_encerrada()}</p>
        <button class="back-btn" onclick={onBack}>{'← '}{m.comum_voltar()}</button>
      </div>
    {:else}
      <!-- Composer SEMPRE visivel (exceto sessao morta). Antes ele sumia em awaiting_input e,
           se as opcoes nao fossem parseadas, o usuario ficava sem input E sem botoes = preso.
           Os OptionButtons continuam aparecendo na lista; o composer fica como saida garantida. -->
      <Composer
        bind:this={composerRef}
        {sessionName}
        bind:inputText={composerText}
        sessionState={currentState}
        status={status}
        {lastCache}
        stats={statsEvent}
        onSend={handleSend}
        onSteer={sessionProvider === 'kimi' ? steerAgora : undefined}
        {filaCount}
        onCommand={handleCommand}
        onInterrupt={handleInterrupt}
        onOpenGit={() => (gitOpen = true)}
        onOpenPreview={() => (previewOpen = true)}
        provider={sessionProvider}
        engine={sessionEngine}
        {pairPeers}
        {pairedState}
        onOpenPair={() => (pairOpen = true)}
        onOpenOrq={() => (orqOpen = true)}
        {sendToPair}
        onToggleSendToPair={() => (sendToPair = !sendToPair)}
        shellsRodando={activity.runningShells}
        onOpenActivity={() => (activityOpen = true)}
      />
    {/if}
  </div>
  </div>

  <SessionSwitcherSheet
    open={switcherOpen}
    sessions={allSessions}
    currentName={sessionName}
    onPick={pickSession}
    onNew={startNew}
    onClose={() => (switcherOpen = false)}
  />

  <CreateSessionSheet
    open={createOpen}
    servers={listServers()}
    onClose={() => (createOpen = false)}
    onCreate={handleCreate}
    onOpenSession={onNavigateToChat}
    bastao={bastaoAlvo}
  />

  <ForwardSheet
    open={forwardText != null}
    text={forwardText ?? ''}
    fromSession={sessionName}
    onClose={() => (forwardText = null)}
  />

  <OrquestracaoSheet
    open={orqOpen}
    {sessionName}
    sessoes={allSessions}
    onClose={() => (orqOpen = false)}
  />

  <PairSheet
    open={pairOpen}
    {sessionName}
    {pairPeers}
    onClose={() => (pairOpen = false)}
    onChanged={loadSessionsForNav}
    onOpenSplit={onOpenSplit
      ? (peer) => { pairOpen = false; onOpenSplit?.(peer); }
      : undefined}
    onOpenPeerChat={nested ? undefined : (peer) => { pairOpen = false; peerChat = peer; }}
  />

  <!-- Sessão do par por cima desta, inteira (transcript + composer). Mora aqui, no Chat, porque é o
       arquivo que as DUAS views usam — sai de graça no celular e no desktop. -->
  {#if !nested}
    <PairChatModal name={peerChat} {desktop} onClose={() => (peerChat = null)}
                   onNavigateToChat={onNavigateToChat} />
  {/if}

  <UsageSheet open={usageOpen} {status} onClose={() => (usageOpen = false)} />

  <Git open={gitOpen} {sessionName} {desktop} {filesInContext} onClose={() => (gitOpen = false)}
       {events} {histGap} cwd={planSession?.cwd ?? null} />

  <RunSheet open={runOpen} {sessionName} onClose={() => (runOpen = false)} onRunningChange={(r) => (runRunning = r)} />
  <MoreSheet open={moreOpen} onClose={() => (moreOpen = false)}
             onRun={() => (runOpen = true)} {runRunning}
             onActivity={(hasActivity || !!planName) ? () => (activityOpen = true) : undefined}
             onAttachments={() => (anexosOpen = true)}
             onBastao={passarBastaoDaqui}
             {activityRunning} {activityBadge} />
  <AttachmentsSheet open={anexosOpen} {sessionName} onClose={() => (anexosOpen = false)}
                    onUsarNoDitado={usarAnexoNoDitado} />

  <CodexLimitsSheet open={limitsOpen} {sessionName} onClose={() => (limitsOpen = false)} />

  <PreviewSheet open={previewOpen} onClose={() => (previewOpen = false)} />

  <ActivitySheet open={activityOpen} {activity} {sessionName} onClose={() => (activityOpen = false)}
    showPlan={!desktop} session={planSession} {planDetail} {planLoading} {planError} />

  <TerminalMirror open={mirrorOpen} {sessionName} onClose={closeMirror} />
  <TerminalMobile open={xtermOpen} {sessionName} onClose={() => (xtermOpen = false)} />

  {#if !isWide}
    <AskQuestionSheet
      open={askOpen}
      payload={askPayload}
      onSubmit={handleAnswer}
      onClose={closeAsk}
      onFallback={openMirror}
    />
  {/if}
</div>

<style>
  /* Underlay da conversa (B5): envolve MessageList + pills + dock pra ficar `inert` com o
     visor aberto. Repete o flex column do pai — sem isto o skeleton/MessageList (flex:1)
     perderia a altura e a coluna colapsaria. Nenhum estilo proprio: so o percurso de teclado. */
  .chat-underlay {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .chat-screen {
    display: flex;
    flex-direction: column;
    height: 100vh;          /* fallback; o JS (fit) sobrescreve com visualViewport.height no teclado */
    overflow: hidden;
    position: relative;
    top: 0;
    background: var(--bg-base);  /* backing solido: a layer nunca renderiza preto no glitch do iOS */
    /* Higiene de stacking; reforço de baixo risco. isolation NAO cria containing block pros sheets
       position:fixed (filhos da .chat-screen) — ao contrário de transform/contain/will-change/filter,
       que clipariam os sheets. NÃO reintroduzir transform aqui (top relativo = sem layer, sem preto). */
    isolation: isolate;
  }

  /* Desktop: a tela acompanha a PANE (.pane/.board-overlay, ambos height:100% do .desktop-main
     ou de um wrapper que encolhe com o TerminalPanel aberto no rodape), nao a viewport inteira -
     100vh vazava por baixo do painel de terminal e escondia o composer atras dele. Sem teclado
     virtual em desktop, o fit acima (nested || desktop) nem roda pra sobrescrever isto. */
  .chat-screen.desktop { height: 100%; }

  /* Pane do split é um card de vidro (DesktopShell pinta --glass-panel): o backing sólido aqui
     taparia a foto. O glitch do iOS que pede o sólido é mobile — este modo é desktop-only. */
  .chat-screen.split-pane { background: transparent; }

  /* Navbar overlay colado no topo (nao descola): a lista rola POR BAIXO via --nav-h. pointer-events
     deixa o fade transparente passar o toque pro conteudo; a navbar reativa. */
  .navbar-mount {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    z-index: 20;
    pointer-events: none;
  }
  .navbar-mount > :global(.navbar) {
    pointer-events: auto;
  }

  /* Aba fina do split (substitui a NavBar no pane estreito). No fluxo do .chat-screen, acima do
     underlay — sem fundo próprio: quem pinta o vidro é o card do pane (DesktopShell). */
  .split-tab {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    flex-shrink: 0;
    border-bottom: 1px solid var(--border-subtle);
  }
  .split-tab-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; }
  .split-tab-nome {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .split-tab-fechar {
    margin-left: auto;
    width: 22px;
    height: 22px;
    /* vence o piso global de 44px de button (app.css) — senão a aba "fina" estufa pra 56px */
    min-width: 0;
    min-height: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-secondary);
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
  }
  .split-tab-fechar:hover { color: var(--text-primary); background: var(--bg-hover); }

  /* Desktop LARGO com painel de contexto: a NavBar some — info (estado/repo/modelo/limites) e
     acoes (terminal/rodar/anexos/atividade) ja migram pro DesktopSessionContext. Mesma breakpoint
     do painel (1280px): ele so existe la, entao nunca fica um sem o outro. A altura medida cai
     pra 0 via ResizeObserver e a lista sobe junto (--nav-h). */
  @media (min-width: 1280px) {
    .chat-screen.with-context .navbar-mount { display: none; }
  }

  .chat-error {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-4);
    padding-top: var(--nav-h, 56px);
  }

  /* Skeleton de boot (no lugar do splash): linhas shimmer ocupando a area do chat. */
  .chat-skeleton {
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: var(--space-3);
    padding: var(--space-6) var(--space-5);
    max-width: 600px;
    width: 100%;
    margin: 0 auto;
    overflow: hidden;
  }
  .sk-line {
    height: 16px;
    border-radius: 8px;
    align-self: flex-start;
    background: linear-gradient(90deg, var(--bg-elevated) 0%, var(--bg-hover) 40%, var(--accent-dim) 50%, var(--bg-hover) 60%, var(--bg-elevated) 100%);
    background-size: 220% 100%;
    animation: sk-shim 1.6s linear infinite;
  }
  .sk-line.sk-r { align-self: flex-end; }   /* algumas linhas "do usuario" a direita */
  /* Aviso da 2a tentativa: transparente de proposito — quem carrega o material e a tela (ver a
     regra de transparencia no CLAUDE.md); aqui e so texto por cima. */
  .sk-aviso {
    align-self: center;
    margin: var(--space-3) 0 0;
    font-size: 13px;
    color: var(--text-dim);
  }
  @keyframes sk-shim {
    0%   { background-position: 140% 0; }
    100% { background-position: -140% 0; }
  }

  .chat-error {
    max-width: 380px;
    margin: 0 auto;
    padding-left: var(--space-5);
    padding-right: var(--space-5);
  }
  .chat-error-title {
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    text-align: center;
  }
  .chat-error-hint {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    text-align: center;
    line-height: 1.5;
  }
  .chat-error-hint code {
    font-family: var(--font-mono);
    font-size: 0.85em;
    background: var(--bg-elevated);
    padding: 1px 5px;
    border-radius: var(--radius-sm);
  }
  .chat-error-actions {
    display: flex;
    gap: var(--space-3);
    flex-wrap: wrap;
    justify-content: center;
  }

  .retry-btn {
    height: 44px;
    padding: 0 var(--space-5);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    background: var(--accent);
    color: #fff;
    font-size: var(--text-sm);
    font-weight: 500;
  }
  .back-btn-inline {
    height: 44px;
    padding: 0 var(--space-5);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-secondary);
    font-size: var(--text-sm);
  }

  /* Bottom dock: statusline bar + composer (or dead footer). Flex child normal. */
  .bottom-dock {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    z-index: 20;
  }

  /* O painel contextual é um card FLUTUANTE (position:absolute, DesktopSessionContext.svelte:279),
     então quem abre espaço pra ele é aqui — e a reserva tem que valer na MESMA faixa em que ele
     existe: 1280px (abaixo disso ele é display:none, DesktopSessionContext.svelte:656). Enquanto a
     reserva morava dentro do `@media (min-width: 1900px)`, todo monitor entre 1280 e 1899 montava o
     painel sem reservar nada: o dock ia de ponta a ponta e, com z 20 contra os 17 do painel, passava
     POR CIMA dele, comendo o rodapé (LIMITES/REPOSITÓRIO). */
  @media (min-width: 1280px) {
    /* O painel lateral ocupa espaço real de leitura. Reservamos essa faixa no próprio scroller,
       então a coluna continua centrada no espaço restante quando a sidebar abre/fecha, em vez de
       ficar presa a uma margem direita fixa que desloca o chat em larguras intermediárias. */
    /* a largura vem do estado do painel (ctxPanel), via style inline no elemento */
    .chat-screen.with-context { --ctx-w: var(--cp-ctx-w, 264px); }

    /* A coluna vive no ESPAÇO LIVRE entre a sidebar e o painel de contexto — começa onde a
       sidebar termina (o .chat-screen JÁ começa depois dela, quem reserva a faixa é o
       DesktopShell) e termina onde o painel começa. O centro dela é o centro desse espaço, e
       ELE SE MOVE quando o painel redimensiona: o recuo-esq é zero de propósito, o texto
       acompanha a divisória em vez de flutuar num vão (decisão do usuário, 16/08/2026 — a
       Task 17 tornou o painel arrastável e o centro-fixo-na-janela gastava ctx-nav px de vão
       à esquerda).
       O CENTRO FIXO NA JANELA morreu aqui. Ele existia porque, com o painel FIXO, o texto
       pulava de lugar a cada toggle de sidebar/painel (medido 10/08/2026: coluna em 0→1016
       numa janela de 1280, texto colado na borda esquerda com a faixa do painel vazia à
       direita). A regra nova aceita o deslocamento do texto quando a faixa muda — o preço é
       o usuário escolheu — mas NÃO pode trazer aquele caso de volta: com a sidebar em trilho
       e recuo-esq zero, a coluna enche a faixa e encosta na borda (folga 0) quando a faixa é
       menor que o max-width — é a MESMA forma do incidente. O max-width + margin auto dão a
       simetria quando há folga; sem folga, a coluna ocupa o que tem (simetria cede antes da
       legibilidade, decisão do usuário). */
    .chat-screen.desktop {
      --recuo-esq: 0px;
      --recuo-dir: var(--ctx-w, 0px);
    }
    .chat-screen.desktop :global(.message-list) {
      box-sizing: border-box;
      padding-left: var(--recuo-esq);
      padding-right: var(--recuo-dir);
    }
    .chat-screen.desktop .bottom-dock {
      left: var(--recuo-esq);
      right: var(--recuo-dir);
    }
    .chat-screen.with-context :global(.messages-inner) {
      /* A escala de largura (Aparencia -> Texto da conversa) entra aqui tambem: sem ela, abrir o
         painel de contexto ignorava a escolha do usuario e a coluna voltava ao teto cheio — o
         slider parecia nao funcionar justamente na tela mais larga, que e onde ele mais importa. */
      max-width: min(calc(min(1200px, 100%) * var(--cp-width-scale, 1)), 100%);
      margin-inline: auto;
    }
    /* (o `right` do dock agora vem do bloco de centro fixo acima, junto com o `left`) */
    /* MESMA fórmula da .messages-inner (teto × escala), não um número próprio: com teto fixo
       (1220px), bastava a coluna escalada passar dele pra bordas de texto e composer descolarem —
       texto colado na esquerda com o composer centrado (relatado 18/08/2026, modo abas + painel). */
    .chat-screen.with-context .bottom-dock :global(.composer-card) {
      max-width: min(calc(min(1200px, 100%) * var(--cp-width-scale, 1)), 100%);
    }
    .chat-screen.with-context .chat-skeleton,
    .chat-screen.with-context .chat-error { transform: translateX(calc(var(--ctx-w) / -2)); }
  }

  /* A largura do painel agora é arrastável e guardada (ctxPanel.largura, Task 17): os degraus
     fixos que existiam aqui (300px/340px em telas grandes) foram movidos pro default do store
     (larguraDefault) — o usuário arrasta por cima e a escolha dele manda. */
  /* Com o painel aberto o teto do texto também sobe por degraus: numa Full HD o 1200 fixo deixava
     ~225px de vazio de cada lado. Medido em 1920: 1440 usa o espaço e ainda sobra respiro. */
  /* Os dois degraus abaixo NASCERAM sem a escala de largura, e sao mais especificos que a regra que a
     aplica — entao acima de 1600px o slider "Largura da coluna" nao fazia nada: medido em 2200px de
     janela, 60 e 150 davam os mesmos 1440px. Mesma forma das outras: a escala multiplica o que a
     coluna teria (o menor entre o degrau e o espaco), limitada ao espaco real. */
  @media (min-width: 1600px) {
    .chat-screen.with-context :global(.messages-inner),
    .chat-screen.with-context .bottom-dock :global(.composer-card) { max-width: min(calc(min(1320px, 100%) * var(--cp-width-scale, 1)), 100%); }
  }
  @media (min-width: 1900px) {
    .chat-screen.with-context :global(.messages-inner),
    .chat-screen.with-context .bottom-dock :global(.composer-card) { max-width: min(calc(min(1440px, 100%) * var(--cp-width-scale, 1)), 100%); }
  }

  /* Navegador embutido: o recuo dele SOMA no do painel de contexto (--ctx-w), e vale em TODA a
     largura de desktop — as regras de --recuo-dir acima só existem em 1280+ porque só o contexto
     as usava; com o navegador aberto o conteúdo precisa recuar em qualquer largura, senão o view
     nativo (que flutua POR CIMA do DOM) cobriria o texto e o composer. O NavegadorPane se posiciona
     pela mesma var (--cp-nav-w), então bounds do view e faixa reservada nunca divergem. */
  @media (min-width: 820px) {
    .chat-screen.desktop.with-nav { --cp-nav-w: clamp(420px, 44vw, 920px); }
    .chat-screen.desktop.with-nav { --recuo-dir: calc(var(--ctx-w, 0px) + var(--cp-nav-w, 0px)); }
    .chat-screen.desktop.with-nav :global(.message-list) { box-sizing: border-box; padding-right: var(--recuo-dir); }
    .chat-screen.desktop.with-nav .bottom-dock { right: var(--recuo-dir); }
  }

  /* Aviso flutuante "interação só pela TUI": acima do dock (bottom = altura do dock + gap, via JS).
     Pulsa pra chamar atenção; centralizado. z acima do dock. */
  .tui-pill {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    z-index: 21;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    max-width: calc(100% - var(--space-6));
    padding: var(--space-2) var(--space-4);
    border: 1px solid var(--accent);
    border-radius: var(--radius-full, 999px);
    background: var(--bg-elevated, var(--bg-base));
    color: var(--text-primary);
    font-size: var(--text-sm);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
    animation: tui-pulse 1.6s ease-in-out infinite;
    -webkit-tap-highlight-color: transparent;
  }
  .tui-pill:active { background: var(--bg-hover); }
  .tui-pill-dot {
    width: 8px; height: 8px; flex-shrink: 0;
    border-radius: 50%;
    background: var(--accent);
  }
  .tui-pill-text { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  @keyframes tui-pulse {
    0%, 100% { box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35), 0 0 0 0 var(--accent); }
    50%      { box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35), 0 0 0 4px transparent; }
  }
  @media (prefers-reduced-motion: reduce) {
    .tui-pill { animation: none; }
  }

  /* Pilula de triage "N aguardando" (mobile, feature #4): FAB no canto inferior direito, acima do
     dock, alcance de polegar. Sem pulso (nao e alerta de bloqueio como o tui-pill, e uma acao
     disponivel) — cor de destaque so pra chamar atencao sem ansiedade visual. */
  .awaiting-pill {
    position: absolute;
    right: var(--space-4);
    z-index: 21;
    padding: var(--space-2) var(--space-4);
    border: none;
    border-radius: var(--radius-full, 999px);
    background: var(--accent);
    color: #fff;
    font-size: var(--text-sm);
    font-weight: 600;
    white-space: nowrap;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
    -webkit-tap-highlight-color: transparent;
  }
  .awaiting-pill:active { opacity: 0.85; }

  /* Aviso de carga de fundo falhada: mesma familia do awaiting-pill (acima do dock), a ESQUERDA
     pra nao brigar com ele (direita) nem com o tui-pill (centro). Discreto de proposito — nao e
     bloqueio, e informacao: falta historico antigo. */
  .hist-pill {
    position: absolute;
    left: var(--space-4);
    z-index: 21;
    max-width: calc(100% - var(--space-8));
    padding: var(--space-2) var(--space-4);
    border: 1px solid var(--border);
    border-radius: var(--radius-full, 999px);
    background: var(--bg-elevated, var(--bg-base));
    color: var(--text-secondary);
    font-size: var(--text-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
    -webkit-tap-highlight-color: transparent;
  }
  .hist-pill:active { background: var(--bg-hover); }

  /* O arquivo aberto (Task 11): cobre SÓ a área da conversa — da navbar ao rodapé, do começo
     do chat até o painel de contexto (--ctx-w). A árvore do painel continua viva e clicável ao
     lado (mock 2, sem véu). left: 0 de propósito: o .chat-screen JÁ começa depois da sidebar
     (quem reserva a faixa dela é o DesktopShell), então reservar de novo a faixa da esquerda
     empurraria o visor 282px para dentro e espremeria o diff (medido ao vivo: 612px em vez de
     ~894px). --surface-card:
     caixa de leitura (mesmo token do board/plano), que entra no véu do papel de parede em vez
     de virar retângulo chapado — o visor é superfície grande sobre a foto. z 30: acima do dock
     (20) e das pills (21-22), abaixo dos sheets (100). Sem border-left: a sidebar já tem
     border-right (mesma régua do DesktopShell). */
  .arq-visor {
    position: absolute;
    top: var(--nav-h, 0px);
    bottom: 0;
    left: 0;
    right: var(--ctx-w, 264px);
    z-index: 30;
    display: flex;
    flex-direction: column;
    background: var(--surface-card);
  }
  /* O painel de contexto é display:none abaixo de 1280px — o visor some junto (mesma régua),
     senao o arquivo ficaria cobrindo a navbar num desktop estreito. */
  @media (max-width: 1279px) {
    .arq-visor { display: none; }
  }

  /* Recusa ao responder (opção ou pergunta): mesma família das pílulas acima, centrada como o tui-pill (é
     sobre o toque que acabou de acontecer) e em tom de aviso. `--surface-raised` e não
     `--bg-elevated` cru: com papel de parede ligado, superfície dentro do app acompanha o véu de
     transparência em vez de virar retângulo chapado (regra de vidro do CLAUDE.md). */
  .aviso-err {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    z-index: 22;
    max-width: calc(100% - var(--space-6));
    padding: var(--space-2) var(--space-4);
    border: 1px solid var(--warning);
    border-radius: var(--radius-full, 999px);
    background: var(--surface-raised);
    color: var(--text-primary);
    font-size: var(--text-sm);
    text-align: left;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.35);
    -webkit-tap-highlight-color: transparent;
  }
  .aviso-err:active { background: var(--bg-hover); }

  /* Dead state footer */
  .dead-footer {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-5) var(--space-6);
    background: var(--bg-base);
  }

  .dead-text {
    font-size: var(--text-sm);
    color: var(--text-muted);
    text-align: center;
  }

  .back-btn {
    height: 44px;
    padding: 0 var(--space-6);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    transition: background 180ms ease-out;
  }

  .back-btn:active {
    background: var(--bg-hover);
  }
</style>
