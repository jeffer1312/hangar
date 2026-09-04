<script lang="ts">
  import { onMount } from 'svelte';
import * as m from '../paraglide/messages';
  import HangarMark from './icons/HangarMark.svelte';
  import HangarWorking from './icons/HangarWorking.svelte';
  import { createSession, gitAction, checkoutBranch, getHistoryTailForServer } from '../lib/api';
  import { getActiveId, serverColor, withServer } from '../lib/auth';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { abrirConfig } from '../lib/configNav';
  import CreateSessionSheet from './CreateSessionSheet.svelte';
  import SessionContextMenu from './SessionContextMenu.svelte';
  import Git from './Git.svelte';
import ConfirmDialog from './ConfirmDialog.svelte';
  import SessionSwitcherSheet from './SessionSwitcherSheet.svelte';
  import HoverPreview from './HoverPreview.svelte';
  import StateChip from './StateChip.svelte';
  import ProviderGlyph from './icons/ProviderGlyph.svelte';
  import type { SessionInfo, AggSession, Provider } from '../lib/types';
  import { cwdParts, rotuloEstado, stateColors, countAwaiting, initials, fmtWhen, latestAssistantEvent, clusterByPair, untrackedReason, providerTag } from '../lib/format';
  import { updateBadge } from '../lib/badge';
  import { loopBadge, LOOP_TONE_COLOR } from '../lib/loop';
  import { planBadge } from '../lib/plan';
  import PlanBar from './PlanBar.svelte';
  import type { WorkspaceAction, WorkspaceView } from '../lib/workspaceCommands';
  import WorkspaceNav from './WorkspaceNav.svelte';
  import { sidebarPrefs } from '../lib/sidebarPrefs.svelte';
  import { sidebarBridge } from '../lib/sidebarBridge';
  import { sidebarPin } from '../lib/sidebarPin.svelte';
  import { navMode } from '../lib/navMode.svelte';
  import { ctxPanel } from '../lib/ctxPanel.svelte';
  import { createSessionListModel } from '../lib/sessionListModel.svelte';

  const DEFAULT_BRANCHES = new Set(['main', 'master']);

  function showBranch(branch: string | null | undefined): branch is string {
    return !!branch && !DEFAULT_BRANCHES.has(branch);
  }

  // Linha secundária da sidebar: só o sinal acionável. O detalhe longo do spinner (modelo,
  // tokens, tempo) continua no tooltip, mas não vira texto permanente na lista.
  function sidebarStatus(s: AggSession): string | null {
    if (s.state === 'awaiting_input') return s.question ?? null;
    if (s.state !== 'working') return null;
    const label = (s.label ?? '').trim();
    if (!label) return null;
    return label.replace(/\s+\([^)]*\)\s*$/, '');
  }

  // Sidebar do DESKTOP (so monta >=820px). Reusa as MESMAS APIs/componentes do mobile, sem tocar
  // no fluxo mobile (SessionList continua intacto). Recolhe pra um trilho de ícones.
  interface Props {
    currentSession: string | null;
    onSelect: (name: string) => void;
    onCompare: (ids: { serverId: string; name: string }[]) => void;
    boardActive: boolean;      // quadro aberto -> destaca o toggle e recolhe a sidebar pro rail
    canvasActive: boolean;     // canvas aberto -> mesmo tratamento do quadro (destaca + recolhe)
    orqActive: boolean;        // orquestração aberta -> idem (a tela é larga, quer o rail)
    onWorkspaceActionsChange?: (actions: WorkspaceAction[]) => void;
    // Seletor de view (Conversa/Quadro/Canvas) + ⌘K: morava flutuando no topo da .desktop-main e
    // pousava em cima do texto do chat (a conversa nao tem o padding-top que board/canvas tem).
    // Aqui dentro ele fica no chrome, junto do resto da navegacao.
    view: WorkspaceView;
    onSelectView: (view: WorkspaceView) => void;
    onOpenCommand: () => void;
    // Avisa o pai (DesktopShell) sempre que o estado EFETIVO (pin ou override do board/canvas)
    // mudar -- mesmo padrao do onMaximizar do TerminalPanel (callback opcional + $effect observando
    // o valor). O DesktopShell nao enxerga o estado interno, e precisa dele pra so esconder a
    // sidebar com o terminal maximizado quando ela esta recolhida, nao quando esta fixada aberta.
    onCollapsedChange?: (v: boolean) => void;
    // Painel de contexto montado (= sessão aberta sem split, ou overlay). Sem ele o toggle do
    // rodapé do trilho desabilita — mesmo contrato que a SessionTabs (DesktopShell entrega).
    ctxDisponivel?: boolean;
    // Overlay do Quadro/Canvas aberto (rota #/board|#/canvas/<server>/<nome>): o Chat do overlay
    // recebe showContextPanel=true, então ele HOSPEDA o visor de arquivo — a Sidebar precisa
    // saber qual sessão está no overlay pra acertar o filesInContext do Git do trilho (Task 15).
    overlaySession: { name: string; serverId: string } | null;
  }
  let {
    currentSession, onSelect, onCompare, boardActive, canvasActive, orqActive,
    onWorkspaceActionsChange, view, onSelectView, onOpenCommand, onCollapsedChange,
    ctxDisponivel = true, overlaySession,
  }: Props = $props();

  // Toda a lógica compartilhada com o celular mora no modelo; aqui fica só o chrome do desktop
  // (rail, pin, kebab, espiada, menu de contexto, rename inline) e os embrulhos de uma linha.
  const model = createSessionListModel({
    variant: 'desktop',
    onOpen: (n) => onSelect(n),
    onCompare: (ids) => onCompare(ids),
    currentSession: () => currentSession,
  });

  // Pin do trilho mora na store compartilhada (sidebarPin): a PREFERENCIA persistida (o que o
  // usuario clicou) fica separada do override TEMPORARIO do Board/Canvas — o auto-recolher nao pode
  // virar preferencia gravada.

  // Quadro OU canvas aberto -> força o recolhido (as colunas/canvas querem a largura); sair restaura
  // o pin como estava ANTES de entrar. O override mexe so no `forced` da store, nunca na preferencia:
  // o cleanup roda a cada mudanca de `overviewActive`, entao sair do board volta o valor do usuario.
  const overviewActive = $derived(boardActive || canvasActive || orqActive);
  $effect(() => {
    sidebarPin.setForced(overviewActive ? true : null);
    return () => sidebarPin.setForced(null);
  });

  // Publica o ESTADO EFETIVO pro pai (DesktopShell) a cada mudanca do pin ou do override do
  // board/canvas — o pai so precisa saber se a sidebar esta visivel agora, nao de onde veio o valor.
  $effect(() => { onCollapsedChange?.(sidebarPin.collapsed); });

  // Densidade compacta (1 linha por sessao: so lead+nome+chips), persistida — mesmo padrao do
  // groupBy acima. Puramente visual: nao muda a lista, so esconde as linhas secundarias via CSS.
  const DENSITY_KEY = 'cp_density';
  let compact = $state(localStorage.getItem(DENSITY_KEY) === 'compact');
  function toggleCompact() {
    compact = !compact;
    try { localStorage.setItem(DENSITY_KEY, compact ? 'compact' : 'normal'); } catch { /* storage cheio/off */ }
  }

  // ── Largura redimensionavel (drag na borda direita), persistida ─────────────
  const WMIN = 200, WMAX = 520;
  const clampW = (w: number) => Math.max(WMIN, Math.min(WMAX, w));
  let width = $state(clampW(Number(localStorage.getItem('cp_sidebar_w')) || 270));
  let resizing = $state(false);
  function resizeStart(e: PointerEvent) {
    resizing = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function resizeMove(e: PointerEvent) {
    if (resizing) width = clampW(e.clientX);   // sidebar cola na esquerda -> clientX = largura
  }
  function resizeEnd() {
    if (!resizing) return;
    resizing = false;
    try { localStorage.setItem('cp_sidebar_w', String(width)); } catch { /* storage cheio/off */ }
  }
  const servers = $derived(sessionsStore.servers);
  // Agrupando por projeto, o cwd já está no header do grupo; mostrar o caminho em cada row é
  // redundância. Ele volta a aparecer quando o agrupamento é por servidor.
  const showCwd = $derived(model.groupMode === 'server');
  let activeId = $state(getActiveId());
  let showCreate = $state(false);
  // Passagem de bastão: mesma folha de criar, aberta pra CONTINUAR a sessão apontada. Fica ao lado
  // do `showCreate` e é sempre zerado por `abrirCriar()` — a folha só lê o alvo na transição de
  // abertura, então limpar no fechamento faria a tela piscar em modo normal enquanto ela sai.
  let bastaoAlvo = $state<{ name: string; cwd: string; serverId: string } | null>(null);
  function abrirCriar() { bastaoAlvo = null; showCreate = true; }
  // Fallback de foco dos diálogos: a engrenagem é o controle que SEMPRE sobra acessível.
  let acctBtnEl = $state<HTMLElement | null>(null);
  let searchOpen = $state(false);     // Buscar conversas (switcher em modo so-busca)

  // Kebab "⋯" do header: popover com a nav secundária (Buscar/Arquivo/Custos) + o toggle de
  // agrupamento — o que antes empilhava no topo da sidebar. Ancorado ao botão, abre pra baixo.
  let kebabOpen = $state(false);
  let kebabPos = $state({ left: 0, top: 0 });
  function openKebab(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    kebabPos = { left: Math.max(8, r.right - 220), top: r.bottom + 6 }; // alinha o popover à direita do botão
    kebabOpen = true;
  }
  function closeKebab() { kebabOpen = false; }

  // Agregação SSE multi-servidor vive no store compartilhado (1 EventSource por servidor pros 3
  // consumidores — Sidebar/SessionList/Board). Aqui só seguramos 1 retain e derivamos os grupos.
  // Breakpoint do painel de contexto (Task 14): o mesmo matchMedia do Chat — abaixo de 1280px o
  // DesktopSessionContext e display:none, e o Git da Sidebar volta a ser a unica casa de Arquivos.
  let isDesktopLargo = $state(
    typeof window !== 'undefined' && window.matchMedia('(min-width: 1280px)').matches,
  );
  onMount(() => {
    const off = model.mount();
    const mqLargo = window.matchMedia('(min-width: 1280px)');
    const onLargo = () => (isDesktopLargo = mqLargo.matches);
    mqLargo.addEventListener('change', onLargo);
    // Bridge pro SessionTabs (Task 6): as abas vivem FORA da <aside> recolhida, mas os workflows
    // continuam aqui — criar sessao, menu de contexto e kebab sao delegados por ela.
    const unregisterBridge = sidebarBridge.register({
      openCreate: abrirCriar,
      openSessionMenu: (event, session, serverId) => openMenu(event, session, serverId),
      openKebab,
    });
    return () => {
      unregisterBridge();
      mqLargo.removeEventListener('change', onLargo);
      off();
      clearTimeout(hpTimer);   // espiada pendente nao sobrevive ao unmount (fetch orfao + setState)
    };
  });

  // Web push + horas silenciosas vivem na tela Servidores da Configuração (ServidoresSettings,
  // extraído na Task 4a/4b) — não existem mais aqui.

  // ── Retomar sessao "sem id" (paridade com o SessionCard do mobile): relança o pane com
  // `claude --resume <uuid>` -> passa a rastrear. Caso seguro resolve direto; caso ambiguo o backend
  // devolve candidatos e abrimos o modal pra confirmar QUAL conversa retomar. Reusa o MESMO
  // resumeSession. withServer mira o dono e restaura (igual ao resto das ops). ──────────────────────
  async function handleResume(name: string, serverId: string, sessionId?: string, e?: MouseEvent) {
    e?.stopPropagation();
    const r = await model.resume(name, serverId, sessionId);
    if (!r.ok && !model.resumeCandidates) flash(m.sessao_flash_retomar({ n: r.erro }));   // erro do botão da linha -> toast
  }

  async function handleCreate(name: string, cwd?: string, configDir?: string | null, provider?: Provider,
                              engine?: string | null, model?: string | null, effort?: string | null,
                              permissionMode?: string | null) {
    // O CreateSessionSheet já posicionou o servidor-alvo como ativo (selectServer).
    const info = await createSession(name, cwd, configDir, provider, engine, model, effort, permissionMode);
    abrirSessaoDoSheet(name);
    // Aviso da reconciliação da conta (plugin ligado sem instalação etc): antes só ia pro log do
    // backend e a sessão abria "normal" sem o plugin. Texto vem pronto do backend.
    if (info?.avisos?.length) flash(m.sessao_flash_avisos_conta({ n: info.avisos.join(' · ') }));
    // SSE stream emitirá a sessão nova automaticamente
  }
  // TODA saída do CreateSessionSheet passa por aqui — o create normal, o "continuar conversa" e a
  // passagem de bastão. O sheet já trocou o servidor ativo (selectServer no pickTarget), e este
  // `activeId` é a cópia LOCAL que a Sidebar usa pro ponto do servidor e pra marcar a linha
  // `.active`. Sem ressincronizar, passar bastão de uma sessão de OUTRO servidor deixava o badge
  // no servidor antigo e a linha da sessão nova sem destaque (o "I2" original, por outra porta —
  // o ramo do bastão não passa pelo handleCreate).
  function abrirSessaoDoSheet(name: string) {
    activeId = getActiveId();
    onSelect(name);
  }
  // Excluir sessao pede confirmacao (com o nome) — clique unico no × era facil de errar. O delete real
  // so acontece no doDelete.
  async function doDelete() {
    const r = await model.doDelete();
    if (r.erro !== '') flash(m.sessao_flash_excluir({ n: r.erro }));
  }

  // ── Renomear sessão do tmux: TOQUE LONGO no nome -> edita inline ──────────────
  let editing = $state<string | null>(null);   // nome da sessão em edição
  let editValue = $state('');
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  let longPressed = false;

  function pressStart(key: string) {
    longPressed = false;
    clearTimeout(pressTimer);
    pressTimer = setTimeout(() => { longPressed = true; editing = key; editValue = key.split('::').slice(1).join('::'); }, 500);
  }
  function pressEnd() {
    clearTimeout(pressTimer);
  }
  function onMainClick(name: string, serverId: string, tracked: boolean | undefined, provider?: SessionInfo['provider']) {
    if (longPressed) { longPressed = false; return; } // foi toque longo (renomear)
    if (model.open({ name, serverId, tracked, provider })) activeId = serverId; // I2: badge local em dia
  }

  // ── Espiada no hover (desktop): ultima resposta da sessao sob o mouse, sem trocar de chat ───────
  // Dois freios contra rajada de fetch: (1) so busca depois de 400ms PARADO — passar o mouse por
  // cima de N linhas cancela cada timer no mouseleave e nao chama nada; (2) cache de 30s por sessao
  // — voltar na mesma linha nao refaz o fetch. So com ponteiro de verdade: em touch o "hover" e o
  // 1o toque, e o popover apareceria no caminho do clique.
  const canHover = window.matchMedia('(hover: hover)').matches;
  const HP_DELAY = 400;
  const HP_TTL = 30_000;
  // Cauda curta so pra achar o ultimo assistant_msg. 8 e nao 3: corridas de tool_use/tool_result
  // empurram a resposta pra tras, e uma cauda de 3 costuma vir so com ferramentas -> o popover nunca
  // abriria numa sessao que acabou de usar tools (medido: a cauda 3 desta sessao era
  // tool_use/tool_result/tool_use). O board pede 15, mas ele desenha o card inteiro.
  const HP_TAIL = 8;
  const hpCache = new Map<string, { text: string; at: number }>();
  let hp = $state<{ text: string; x: number; y: number } | null>(null);
  let hpTimer: ReturnType<typeof setTimeout> | undefined;
  let hpKey = '';   // linha sob o mouse AGORA — descarta resposta que chegou depois de o mouse sair

  function hpEnter(e: MouseEvent, name: string, serverId: string) {
    if (!canHover) return;
    const key = `${serverId}::${name}`;
    hpKey = key;
    // Guarda o ELEMENTO agora (no timer o currentTarget do evento ja e null) mas mede so na hora de
    // exibir: entrar no rail recolhido expande a sidebar (56 -> 520px) por baixo do mouse, e a medida
    // do enter apontaria pro rail — o popover nasceria EM CIMA da lista. Medir no fim tambem
    // acompanha scroll durante os 400ms.
    const rowEl = e.currentTarget as HTMLElement;
    clearTimeout(hpTimer);
    hpTimer = setTimeout(async () => {
      const hit = hpCache.get(key);
      let text: string;
      if (hit && Date.now() - hit.at < HP_TTL) {
        text = hit.text;
      } else {
        const srv = servers.find((s) => s.id === serverId);
        if (!srv) return;
        try {
          text = latestAssistantEvent(await getHistoryTailForServer(srv, name, HP_TAIL))?.text ?? '';
        } catch { return; }   // offline/timeout: espiada e opcional, nao vira erro na tela
        hpCache.set(key, { text, at: Date.now() });   // cacheia ate o vazio (senao refetch a cada passada)
      }
      if (hpKey !== key) return;   // mouse ja saiu (ou foi pra outra linha) enquanto buscava
      if (!text) return;
      const anchor = rowEl.getBoundingClientRect();
      hp = { text, x: anchor.right + 8, y: Math.min(anchor.top, window.innerHeight - 240) };
    }, HP_DELAY);
  }
  function hpLeave() {
    hpKey = '';
    clearTimeout(hpTimer);
    hp = null;
  }
  // Rede de segurança do popover PRESO: mouseleave se perde quando a linha re-ordena/some por baixo
  // do ponteiro parado (o browser não dispara enter/leave em movimento de ELEMENTO, só de ponteiro)
  // — o popover ficava aberto pra sempre (pointer-events: none, nem clicável). Qualquer movimento
  // de ponteiro fora de uma linha de sessão fecha. Só roda com hp aberto: custo zero no resto.
  function hpGuard(e: PointerEvent) {
    if (!hp) return;
    const el = e.target as Element | null;
    if (!el?.closest?.('.sess-row')) hpLeave();
  }

  function saveEdit(old: string, serverId: string) {
    const nv = editValue.trim();
    editing = null;
    if (!nv || nv === old) return;
    // Inline preserva o comportamento (fecha o input no blur); falha aparece no toast, não some.
    void model.rename(nv, old, serverId).then((r) => {
      if (!r.ok) flash(m.sessao_flash_renomear({ n: r.erro }));
    });
  }
  function onEditKey(e: KeyboardEvent, old: string) {
    if (e.key === 'Enter') { e.preventDefault(); (e.target as HTMLInputElement).blur(); }
    else if (e.key === 'Escape') { editValue = old; editing = null; }   // cancela (blur vira no-op)
  }
  function autofocus(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  // ── Menu de contexto (botao direito) na linha da sessao — so desktop ──────────
  let menu = $state<{ x: number; y: number; name: string; serverId: string; cwd: string; thenTarget: string | null } | null>(null);
  let menuOrigem: HTMLElement | null = null;
  let menuMsg = $state('');   // banner efemero pro resultado do git pull / erro do editor
  let flashTimer: ReturnType<typeof setTimeout> | undefined;

  // Recolhida, a <aside> inteira sai do DOM (gate no template) — trilho de iniciais e hover
  // expansion nao existem mais. O que sobra decide a altura: dock flutuante so quando o usuario
  // escolheu 'content' em Aparencia (sem pin recolhido pra forcar dock, 'content' e a regra).
  const floating = $derived(sidebarPrefs.height === 'content');
  // Trilho quando o pin está recolhido. SEM os termos de hover/menu/editing do original: a
  // hover-expansion foi removida de propósito na Task 5 e não volta.
  const expanded = $derived(!sidebarPin.collapsed);

  function openMenu(e: MouseEvent, s: SessionInfo, serverId: string) {
    e.preventDefault();
    clearTimeout(pressTimer);   // cancela o long-press (senao dispararia rename junto)
    hpLeave();   // botao direito nao move o mouse: fecha a espiada pra nao ficar atras do menu
    menuOrigem = (e.currentTarget as HTMLElement | null)?.closest('.sess-row')?.querySelector('.sess-main') as HTMLElement | null;
    menu = { x: e.clientX, y: e.clientY, name: s.name, serverId, cwd: s.cwd ?? '', thenTarget: s.then_target ?? null };
    // O SessionContextMenu carrega o estado de silenciar/branches/encadeamento na propria montagem.
  }
  function closeMenu() { menu = null; menuOrigem?.focus(); menuOrigem = null; }

  // Confirmacao de troca com working tree suja (switch carrega mudancas nao-conflitantes pra outra branch).
  let confirmBranch = $state<{ name: string; serverId: string; branch: string } | null>(null);
  // Gerenciador git (GitSheet) aberto pelo menu de contexto, no repo da sessao, SEM abrir o chat.
  function menuGit() {
    if (!menu) return;
    model.openGit(menu.name, menu.serverId);
    closeMenu();
  }
  // filesInContext de VERDADE (Task 14, achado do revisor na Task 12; Task 15, item 1): o Git
  // aberto pelo menu da Sidebar e da sessao X — se um Chat da sessao X esta HOSPEDANDO o visor
  // (painel de contexto visivel: desktop largo + ctxDisponivel + nao recolhido), o clique num
  // arquivo pelo Git desenharia o visor fantasma por tras do modal e deixaria a conversa inert.
  // O host pode ser o Chat normal (sem board/canvas e X = currentSession) OU o Chat do overlay
  // do Quadro/Canvas na MESMA sessao (o overlay recebe showContextPanel=true sempre). Sem o
  // overlaySession, boardActive zerava a expressao inteira e o overlay nao era reconhecido como
  // host — o mesmo defeito que a Task 14 impediu, entrando pelo Quadro.
  const filesInContext = $derived(
    isDesktopLargo && ctxDisponivel && !ctxPanel.recolhido && model.gitSheet !== null
      && ((overlaySession !== null && overlaySession.name === model.gitSheet.name)
        || (!boardActive && !canvasActive && !orqActive && model.gitSheet.name === currentSession)),
  );

  // Passar o bastão: abre a folha de criar já sabendo quem é a origem. Ao contrário do Git/Loop, o
  // servidor NÃO é apontado aqui — quem faz isso é a própria folha (o `pickTarget` da abertura), e
  // travado no da origem: o dossiê é arquivo no disco daquela máquina.
  function menuBastao() {
    if (!menu) return;
    bastaoAlvo = { name: menu.name, cwd: menu.cwd, serverId: menu.serverId };
    showCreate = true;
    closeMenu();
  }
  async function doCheckout(name: string, serverId: string, branch: string) {
    flash(`checkout ${branch}…`);
    try {
      await withServer(serverId, () => checkoutBranch(name, branch));
      flash(`branch: ${branch}`);
    } catch (e) {
      flash(`checkout: ${errMsg(e)}`);
    }
  }
  // Tree suja: guarda tudo no stash (deixa a tree limpa) e ENTAO troca -> resolve o "would be
  // overwritten by checkout". As mudancas ficam recuperaveis com "pop" na aba Git.
  async function stashAndCheckout(name: string, serverId: string, branch: string) {
    flash(m.sessao_guardando());
    try {
      await withServer(serverId, async () => {
        const r = await gitAction(name, 'stash');
        if (!r.ok) throw new Error(r.output || m.sessao_stash_falhou());
        await checkoutBranch(name, branch);
      });
      flash(m.sessao_flash_branch({ n: branch }));
    } catch (e) {
      flash(m.sessao_flash_checkout({ n: errMsg(e) }));
    }
  }
  function flash(msg: string) {
    menuMsg = msg;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => { menuMsg = ''; }, 4000);
  }

  // Candidatas ao encadeamento: sessoes do MESMO servidor da fonte (o vinculo e resolvido pelo backend
  // dessa sessao — nao ha como encadear pra uma sessao de OUTRO servidor), exceto ela mesma. Passado
  // como prop pro SessionContextMenu (que precisa de `groups`, so o Sidebar tem).
  function chainCandidates(serverId: string, exclude: string) {
    return model.allGroups.flatMap((g) => g.sessions).filter((s) => s.serverId === serverId && s.name !== exclude);
  }

  const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

  // Sidebar recolhida (pin OU override do board/canvas): a <aside> sai do DOM e a edição inline
  // não existe — Rename do menu de contexto abre um diálogo acessível (ConfirmDialog + input,
  // mesmo padrão do modal de Adicionar servidor em ServidoresSettings). Com a sidebar visível o
  // inline continua como sempre foi.
  let renameDialog = $state<{ old: string; serverId: string } | null>(null);
  let renameValue = $state('');
  let renameInputEl = $state<HTMLInputElement | null>(null);
  let renameBusy = $state(false);
  let renameError = $state('');
  function menuRename() {
    if (!menu) return;
    if (sidebarPin.collapsed) {
      renameValue = menu.name;
      renameError = '';
      renameDialog = { old: menu.name, serverId: menu.serverId };
    } else {
      editing = `${menu.serverId}::${menu.name}`;
      editValue = menu.name;
    }
    closeMenu();
  }
  function submitRenameDialog() {
    if (!renameDialog || renameBusy) return;
    const { old, serverId } = renameDialog;
    const nv = renameValue.trim();
    // Guard duplo do disabled (que também trima): nunca fecha em no-op.
    if (!nv || nv === old) return;
    renameBusy = true;
    renameError = '';
    void model.rename(nv, old, serverId).then((r) => {
      renameBusy = false;
      if (r.ok) {
        renameDialog = null;
        // A aba antiga foi destruída (keyed por nome): o foco vai pra aba recriada quando o
        // modelo refletir o novo nome (round 2).
        sidebarBridge.focusTab(`${serverId}::${r.name}`);
      } else {
        renameError = r.erro;   // diálogo fica aberto; erro visível ligado ao campo (role=alert)
      }
    });
  }
  function menuDelete() {
    if (!menu) return;
    model.requestDelete(menu.name, menu.serverId);
    closeMenu();
  }

  const activeServer = $derived(servers.find((s) => s.id === activeId) ?? servers[0] ?? null);

  // Conta (avatar do rodapé): nome = servidor ativo; subtítulo = contagem de servidores. Iniciais
  // reusam o helper compartilhado (format).
  const accountName = $derived(activeServer?.label ?? 'conta');
  const accountSub = $derived(`${servers.length} servidor${servers.length === 1 ? '' : 'es'}`);
  // Cor do servidor ATIVO: o mesmo mapa que já pinta os grupos por servidor na lista. Vira o ponto
  // no canto do botão — é o que diz "estou conectado NESTA máquina" sem gastar linha de texto.
  const accountColor = $derived(activeServer ? serverColor(activeServer.id) : 'var(--text-muted)');

  // Badge do ícone do app (feature #13): mesmo agregado do mobile (SessionList).
  $effect(() => { updateBadge(model.awaitingTotal); });

  // Servidores offline SAEM da lista (pedido do usuário: 4 headers "offline" só enchiam) e viram
  // UMA linha-resumo no fim, expansível — sumir total esconderia a queda e a chance de reconectar.
  let showOffline = $state(false);
  // Servidor ONLINE e sem sessão nenhuma também sai: o cabeçalho sozinho gasta uma linha da barra
  // pra dizer que não há nada. Não esconde nada acionável — o "+ Nova" recebe a lista COMPLETA de
  // servidores (`servers`, não `renderGroups`) e deixa escolher o alvo, então dá pra criar sessão
  // num servidor que não aparece aqui.
  const onlineGroups = $derived(model.groups.filter((g) => !g.error && g.sessions.length > 0));
  const offlineGroups = $derived(model.groups.filter((g) => g.error));
  const renderGroups = $derived(showOffline ? [...onlineGroups, ...offlineGroups] : onlineGroups);



  $effect(() => {
    const publish = onWorkspaceActionsChange;
    if (!publish) return;
    publish([
      {
        id: 'new',
        title: m.sessao_nova(),
        detail: m.sessao_criar_nova(),
        keywords: [m.lista_nova_curto(), m.sessao_criar_nova(), m.sessao_singular()],
        group: m.sessao_grupo(),
        run: abrirCriar,
      },
      {
        id: 'search',
        title: m.lista_buscar_historico(),
        detail: m.lista_buscar_conteudo(),
        keywords: ['buscar', m.lista_historico(), 'conversas'],
        group: m.sessao_grupo(),
        run: () => (searchOpen = true),
      },
      {
        id: 'archive',
        title: m.nav_arquivo(),
        detail: m.lista_conversas_encerradas(),
        keywords: ['arquivo', 'arquivadas', m.lista_historico()],
        group: m.sessao_grupo(),
        run: () => (window.location.hash = '#/archive'),
      },
      {
        id: 'costs',
        title: m.nav_custos(),
        detail: m.lista_uso_custos(),
        keywords: ['custos', 'uso', 'tokens'],
        group: m.lista_ferramentas(),
        run: () => (window.location.hash = '#/costs'),
      },
      {
        id: 'compare',
        title: m.lista_comparar_sessoes(),
        detail: m.lista_selecione_2(),
        keywords: ['comparar', m.lista_sessoes_plural(), 'lado a lado'],
        group: m.lista_colaboracao(),
        run: model.openSelectMode,
      },
      {
        id: 'broadcast',
        title: m.lista_broadcast(),
        detail: m.lista_selecione_enviar(),
        keywords: ['broadcast', m.lista_enviar(), 'mensagem', m.lista_sessoes_plural()],
        group: m.lista_colaboracao(),
        run: model.openSelectMode,
      },
    ]);
    return () => publish([]);
  });
</script>

<!-- `floating` segue a escolha de altura em Aparencia ('content' = dock flutuante permanente).
     O gate abaixo envolve SOMENTE a <aside>: sheets, menus, Git/Loop, confirmacoes e kebab — tudo
     que vem depois do </aside> — continuam montados e acessiveis. Recolhida: no modo RAIL (o
     padrão) a aside vira o trilho vertical de iniciais (classe .collapsed, desenho original);
     no modo 'tabs' ela sai do DOM e a SessionTabs assume (Task 6). -->
{#if !sidebarPin.collapsed || navMode.mode === 'rail'}
<aside class="sidebar" class:collapsed={!expanded} class:floating class:resizing style:width={sidebarPin.collapsed ? undefined : width + 'px'}>
  <div class="side-top">
    <!-- Este botao E o liga/desliga da barra: alterna entre trilho de iniciais e aberta. Chegou a
         existir uma preferencia "manter aberta" em Aparencia que fazia a mesma coisa e deixava este
         icone como clique morto — foi apagada; sobrou so a escolha de ALTURA, que e o que faltava. -->
    <!-- A marca fica nas DUAS formas do sidebar: expandido acompanha o nome, recolhido (rail) ela
         é o que sobra. Com 2 arcos, porque no rail ela desenha em ~20px. -->
    <span class="side-mark" aria-label="Hangar"><HangarMark size={20} arcs={2} /></span>
    {#if expanded}<span class="side-brand">Hangar</span>{/if}
    {#if expanded}
    <!-- Broadcast (feature #9): entra/sai do modo seleção multipla. -->
    <button
      class="select-toggle-btn"
      class:active={model.selectMode}
      onclick={model.toggleSelectMode}
      aria-label={model.selectMode ? m.lista_cancelar_selecao() : m.sessao_selecionar()}
      title={model.selectMode ? m.lista_cancelar_selecao() : m.lista_selecionar_broadcast()}
    >
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M9 11l3 3L22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/>
      </svg>
    </button>
    {/if}
    <!-- Kebab "⋯": nav secundária (Buscar/Arquivo/Custos) + agrupamento, docado no header — o twin
         desktop do hamburger do mobile. Abre um popover ancorado (renderizado fora do <aside>). -->
  </div>

  {#if expanded}
  <div class="side-views">
    <WorkspaceNav {view} onSelect={onSelectView} {onOpenCommand} />
  </div>
  {/if}

  <nav class="sess-list" class:compact aria-label={m.lista_titulo()}>
    <!-- A fila "Precisa de você" migrou para o chrome global do DesktopShell: continua visível
         em Conversa/Quadro/Canvas e deixa de duplicar conteúdo no topo da lista. -->

    {#if expanded}
    <!-- Filtro (paridade com o mobile): so aparece quando a lista fica longa. -->
    {#if model.showFilter}
      <input
        type="text"
        class="filter-input"
        bind:value={model.filterText}
        placeholder={m.lista_filtrar()}
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck={false}
        aria-label={m.lista_filtrar()}
      />
    {/if}
    {#if model.filterEmpty}
      <p class="filter-empty">{m.lista_vazia_filtro()}</p>
    {/if}
    {/if}
    <!-- Servidor online e vazio nao aparece mais; com TODOS vazios a lista ficaria em branco, sem
         dizer o porque. (Nao vale quando o filtro esta ativo — ai quem fala e o filter-empty.) -->
    {#if expanded && !model.filterText.trim() && renderGroups.length === 0 && offlineGroups.length === 0}
      <p class="filter-empty">{m.lista_vazia_aberta()} <strong>+ {m.lista_nova_curto()}</strong>.</p>
    {/if}
    {#each renderGroups as g (g.id)}
      {@const awaiting = countAwaiting(g.sessions)}
      {#if expanded && g.label}
        <!-- Sem label = modo "Nenhum": lista lisa, sem cabecalho nem chevron. -->
        <div class="grp-head-row">
          <!-- Header colapsavel (paridade com o mobile): chevron + label + contagem + aguardando. -->
          <button
            class="grp-head"
            onclick={() => model.toggleGroup(g.id)}
            aria-expanded={!model.collapsed.has(g.id)}
            title={g.error ? `${g.label}: ${g.error}` : g.label}
          >
            <span class="grp-chevron" class:collapsed={model.collapsed.has(g.id)} aria-hidden="true">▾</span>
            {#if g.color}<span class="grp-dot" style="background: {g.color};" aria-hidden="true"></span>{/if}
            <span class="grp-label">{g.label}</span>
            {#if g.sessions.length > 0}<span class="grp-count">{g.sessions.length}</span>{/if}
            {#if awaiting > 0}<span class="grp-await" title={`${awaiting} ${m.estado_aguardando()}`}>{awaiting}</span>{/if}
            {#if g.error}<span class="grp-off">{g.error}</span>{/if}
          </button>
          <!-- "enviar p/ todas" (feature #9): entra em modo seleção com o grupo inteiro marcado. -->
          <button
            class="grp-broadcast"
            onclick={() => model.selectGroupForBroadcast(g)}
            aria-label={`${m.lista_enviar_msg_todas()} ${g.label}`}
            title={m.lista_enviar_todas()}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <line x1="22" y1="2" x2="11" y2="13"/>
              <polygon points="22 2 15 22 11 13 2 9 22 2"/>
            </svg>
          </button>
        </div>
      {/if}
      {#if !expanded || !model.collapsed.has(g.id)}
      {#each clusterByPair(g.sessions) as item (item.kind === 'header' ? `ph:${item.gid}` : `${item.session.serverId}::${item.session.name}`)}
        {#if item.kind === 'header'}
          {#if expanded}
          <!-- Cluster de pareamento (Opção C): sub-header colapsável do grupo, dentro do servidor. -->
          <button class="pair-head" onclick={() => model.toggleGroup(`pair:${item.gid}`)}
                  aria-expanded={!model.collapsed.has(`pair:${item.gid}`)} title={m.sessao_grupo_pareado({ label: item.label })}>
            <span class="grp-chevron" class:collapsed={model.collapsed.has(`pair:${item.gid}`)} aria-hidden="true">▾</span>
            <span class="pair-head-label">🤝&nbsp;{item.label}</span>
            <span class="grp-count">{item.count}</span>
          </button>
          {/if}
        {:else if !item.gid || !model.collapsed.has(`pair:${item.gid}`)}
        {@const s = item.session}
        {@const rowKey = `${s.serverId}::${s.name}`}
        {@const selKey = `${s.serverId}:${s.name}`}
        {@const provTag = model.showProviderTags ? providerTag(s.provider) : null}
        {@const sub = sidebarStatus(s)}
        {@const srvLabel = servers.find((sv) => sv.id === s.serverId)?.label ?? s.serverId}
        {@const estadoTxt = s.stalled ? m.sessao_pode_travada() : rotuloEstado(s.state)}
        <!-- role=presentation: a row e so o wrapper flex — a semantica toda vive no .sess-main
             (button) e nos botoes irmaos. O hover aqui e decoracao redundante (a resposta ja esta no
             chat), entao nao pede equivalente de teclado. -->
        <div class="sess-row" class:active={s.serverId === activeId && s.name === currentSession}
             class:pair-member={!!item.gid}
             class:awaiting={s.state === 'awaiting_input'} role="presentation"
             onmouseenter={(e) => hpEnter(e, s.name, s.serverId)} onmouseleave={hpLeave}>

          {#if editing === rowKey}
            <input
              class="sess-edit"
              bind:value={editValue}
              use:autofocus
              onkeydown={(e) => onEditKey(e, s.name)}
              onblur={() => saveEdit(s.name, s.serverId)}
              aria-label={m.sessao_renomear()}
            />
          {:else}
            <button
              class="sess-main"
              class:untracked={s.tracked === false}
              class:untracked-open={s.tracked === false && s.provider === 'kimi'}
              aria-pressed={model.selectMode ? model.selected.has(selKey) : undefined}
              aria-label={!expanded ? `${s.name} · ${srvLabel} · ${estadoTxt}` : undefined}
              title={!expanded
                ? `${s.name} · ${srvLabel} · ${estadoTxt}${provTag ? ` · ${m.sessao_singular()} ${provTag}` : ''}`
                : (s.tracked === false ? untrackedReason(s.provider) : m.sessao_toque_renomear())}
              onpointerdown={() => { if (!model.selectMode && !sidebarPin.collapsed) pressStart(rowKey); }}
              onpointerup={pressEnd}
              onpointerleave={pressEnd}
              onpointercancel={pressEnd}
              oncontextmenu={(e) => { if (!model.selectMode) openMenu(e, s, s.serverId); }}
              onclick={() => {
                hpLeave();   // clique nao move o mouse -> sem mouseleave; fecha a espiada na mao
                if (model.selectMode) { if (s.tracked !== false) model.toggleSelected(selKey); return; }
                onMainClick(s.name, s.serverId, s.tracked, s.provider);
              }}
            >
              <span class="lead" aria-hidden="true">
                {#if model.selectMode}
                  <input type="checkbox" class="select-check" checked={model.selected.has(selKey)} tabindex="-1" aria-hidden="true" />
                {:else if !expanded}
                  <!-- Rail recolhido: SEMPRE iniciais — é o único texto que identifica a sessão sem
                       o nome. O ESTADO saiu do anel em volta delas e virou o ponto do canto: o anel
                       usava --accent pra "trabalhando", a mesma cor que o app usa pra destaque em
                       todo lugar, e "ociosa" era a ausência de anel — indistinguível de sessão que
                       ainda não reportou estado. Com ponto próprio, todo estado tem desenho e a
                       ausência não significa mais duas coisas. -->
                  <span class="initials" title={`${s.name} — ${estadoTxt}`}>{initials(s.name)}</span>
                  <!-- Trabalhando é a MESMA marca animada da lista aberta, só que no canto: um
                       ponto azul parado ao lado de um verde parado não dizia "está ocupada", e
                       inventar outra animação aqui seria um segundo vocabulário pro mesmo estado.
                       As iniciais ficam — trocá-las pelo indicador é o que fazia a coluna virar
                       bolinhas anônimas justo nas sessões ocupadas. O pulso do halo segue reservado
                       a quem espera resposta. -->
                  {#if s.state === 'working' && !s.stalled}
                    <span class="estado-marca" title={estadoTxt} style="color: {stateColors[s.state]};">
                      <HangarWorking size={14} />
                    </span>
                  {:else}
                    <span
                      class="estado-ponto"
                      class:aguardando={s.state === 'awaiting_input' && !s.stalled}
                      title={estadoTxt}
                      style="--cor: {s.stalled ? 'var(--warning)' : stateColors[s.state]};"
                    ></span>
                  {/if}
                {:else if s.state === 'working'}
                  <span class="row-mark" style="color: {stateColors[s.state]};"><HangarWorking size={18} /></span>
                {:else}
                  <span class="row-mark" style="color: {stateColors[s.state]};"><HangarMark size={18} /></span>
                {/if}
                {#if !expanded && !model.selectMode && model.showProviderTags}
                  <!-- Mesma regra da lista aberta: quando a lista MISTURA agentes, todo mundo leva o
                       glifo, Claude incluído — marcar só a exceção não diz nada num trilho onde os
                       nomes já sumiram. Vai no canto de cima do avatar (o de baixo é da barra de
                       plano, o outro de cima é do ponto de estado), absoluto pra não empurrar as
                       iniciais nem mudar a altura da linha. -->
                  <span class="prov-rail" title={provTag ?? 'Claude'}><ProviderGlyph provider={s.provider} size={10} /></span>
                {/if}
              </span>
              {#if !expanded && !model.selectMode}
                <!-- Rail recolhido: barra única na base da row, irmã de .lead (não dentro dele —
                     .lead é a coluna das iniciais). .sess-main precisa de position:relative pra
                     ancorar o position:absolute do compact (ver CSS). A barra e o glifo do harness
                     disputavam esta mesma faixa e eram mutuamente exclusivos; o glifo subiu pro
                     canto de cima do avatar e a base voltou a ser só da barra. -->
                <PlanBar session={s} compact />
              {/if}
              {#if expanded}
              <span class="row-info">
                  <span class="name-row">
                    <span class="sess-name">{s.name}</span>
                    {#if s.tracked === false}<span class="sess-badge" title={untrackedReason(s.provider)}>{m.sessao_sem_id()}</span>{/if}
                  </span>
                  {#if sub}
                    <span
                      class="status-sub"
                      class:asking={s.state === 'awaiting_input'}
                      class:working={s.state === 'working'}
                      title={s.state === 'awaiting_input' ? s.question : s.label}
                    >{sub}</span>
                  {/if}
                  <!-- ⧉ = worktree ligada. Fora do bloco da branch de propósito: worktree com HEAD
                       destacado (ou em main) não tem chip de branch e ainda assim precisa se
                       distinguir do checkout principal. -->
                  {#if s.worktree}
                    <span class="wt" title={m.sessao_worktree()}>worktree</span>
                  {/if}
                  {#if showCwd && s.cwd}
                    {@const cp = cwdParts(s.cwd)}
                    <span class="cwd" title={showBranch(s.branch) ? `${s.cwd} · branch ${s.branch}` : s.cwd}>
                      <span class="cwd-prefix">{cp.prefix}</span><span class="cwd-base">{cp.base}</span>
                      {#if showBranch(s.branch)}<span class="branch-inline">⎇ {s.branch}</span>{/if}
                    </span>
                  {:else if showBranch(s.branch)}
                    <span class="branch" title={m.sessao_branch_git_atual()}>⎇ {s.branch}</span>
                  {/if}
                  <!-- "+128 −24" do working tree, colado à branch/cwd (paridade com o SessionCard
                       mobile; referência: cards do super.engineering). -->
                  {#if s.git_added || s.git_removed}
                    <span class="diff-stats" aria-hidden="true">{#if s.git_added}<span class="diff-add">+{s.git_added}</span>{/if}{#if s.git_removed}<span class="diff-del">−{s.git_removed}</span>{/if}</span>
                  {/if}
                  {#if model.showProviderTags || provTag || s.limited || s.then_target || s.pair_peers?.length || s.loop_status || s.engine || s.plan_name}
                    <!-- Chips informativos (⏳/🔗/🤝/↻/⚙) na COLUNA DE TEXTO, nao ao lado do state-chip:
                         inline eles cobriam o cwd em sidebar estreita (mesmo fix do SessionCard mobile). -->
                    <span class="badges-line">
                      {#if model.showProviderTags}
                        <!-- Glifo pra TODOS quando a lista mistura providers (pedido do usuário);
                             o TEXTO continua só nas não-Claude — o default se reconhece pela marca.
                             provider ausente = Claude (o campo só viaja quando não é Claude). -->
                        <span class="prov-chip" class:prov-chip--so-icone={!provTag} title={`${m.sessao_grupo()} ${provTag ?? 'Claude'}`}><span class="sr-only">{m.sessao_grupo()}&nbsp;</span><ProviderGlyph provider={s.provider} size={12} />{#if provTag}{provTag}{/if}</span>
                      {/if}
                      {#if s.limited}
                        <span
                          class="limited-chip"
                          title={s.limit_reset ? m.sessao_limite_volta({ n: s.limit_reset }) : m.sessao_limite()}
                        >⏳{#if s.limit_reset}&nbsp;{s.limit_reset}{/if}</span>
                      {/if}
                      {#if s.then_target}
                        <span class="chain-chip" title={m.sessao_chain_envia({ n: s.then_target })}>🔗&nbsp;{s.then_target}</span>
                      {/if}
                      {#if s.pair_peers?.length}
                        <span class="chain-chip" title={m.sessao_grupo_chip({ n: s.pair_peers.join(', ') })}>🤝&nbsp;{s.pair_peers.length === 1 ? s.pair_peers[0] : s.pair_peers.length + 1}</span>
                      {/if}
                      {#if s.loop_status}
                        {@const lb = loopBadge(s.loop_status, s.loop_iter, s.loop_max)}
                        {#if lb}
                          <span class="chain-chip" style="color: {LOOP_TONE_COLOR[lb.tone]}; background: color-mix(in srgb, {LOOP_TONE_COLOR[lb.tone]} 14%, transparent);" title={m.sessao_loop_runner()}>{lb.label}</span>
                        {/if}
                      {/if}
                      {#if s.plan_name}
                        {@const pb = planBadge(s)}
                        {#if pb}
                          <span class="plan-chip" class:plan-chip--done={pb.complete} title={pb.title}>{pb.label}</span>
                        {/if}
                      {/if}
                      {#if s.engine}
                        <!-- Sem isto nada na lista distingue uma sessão de motor de uma da conta Anthropic.
                             NÃO mostramos custo aqui: o preço que o Claude Code calcula é tabela Anthropic
                             e mentiria pra um motor de outro provedor. -->
                        <span class="engine-chip" title={m.sessao_motor({ n: s.engine })}>⚙&nbsp;{s.engine}</span>
                      {/if}
                    </span>
                  {/if}
                  <PlanBar session={s} />
                </span>
                <!-- O envelope .state-chip existe pelo anel de travada e pelas regras que ja
                     miravam essa classe (hover da linha, papel de parede no app.css); a pilula em
                     si e o StateChip. -->
                <span class="state-chip" class:stalled={s.stalled === true}>
                  <!-- "pronto" (idle) e o estado COMUM da lista: pilula com texto em toda linha e
                       ruido, nao informacao — e a largura dela truncava o nome da sessao. Vira o
                       ponto (a cor segue dizendo "pronto") e as pilulas de verdade (em execucao,
                       aguardando) sobressaem. O estado segue no aria-label da linha. -->
                  <StateChip state={s.state} dot={s.state === 'idle'} title={s.stalled ? m.sessao_travada() : undefined} />
                </span>
              {/if}
            </button>
            {#if expanded && !model.selectMode}
              <!-- Retomar (paridade com o SessionCard do mobile): unica acao possivel numa linha "sem
                   id" -> visivel sempre (nao escondida no hover), tingida de accent. Reusa resumeSession.
                   Fora do Pi/Kimi/OMP: o resume varre ~/.claude/projects e relanca `claude --resume` DEPOIS
                   de matar o pane -> num pane Pi/Kimi/OMP ofereceria a conversa do agente errado e mataria
                   a sessao viva. Ali o title da linha ja diz o que fazer (untrackedReason). -->
              {#if s.tracked === false && s.provider !== 'pi' && s.provider !== 'kimi' && s.provider !== 'omp'}
                <button
                  class="sess-resume"
                  onclick={(e) => handleResume(s.name, s.serverId, undefined, e)}
                  disabled={model.resumeBusy === s.name}
                  aria-label={`${m.sessao_retomar()} ${s.name}`}
                  title={m.sessao_retomar()}
                >
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <polyline points="1 4 1 10 7 10"/>
                    <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/>
                  </svg>
                </button>
              {/if}
              <!-- Linha enxuta: só o kebab ⋯ (hover) e Retomar (quando sem id). Git/Loop/Excluir agora vivem só no menu de contexto. -->
              <button class="sess-kebab" onclick={(e) => { e.stopPropagation(); openMenu(e, s, s.serverId); }} aria-label={m.sessao_aria_opcoes({ n: s.name })} title={m.sessao_opcoes()}>⋯</button>
            {/if}
          {/if}
        </div>
        {/if}
      {/each}
      {/if}
    {/each}
    {#if expanded && offlineGroups.length > 0}
      <!-- Resumo dos offline (uma linha em vez de N headers): expande pra ver/gerenciar. -->
      <button class="grp-offline-sum" onclick={() => (showOffline = !showOffline)} aria-expanded={showOffline}>
        <span class="grp-chevron" class:collapsed={!showOffline} aria-hidden="true">▾</span>
        ⚠ {offlineGroups.length === 1 ? m.sessao_offline_1() : m.sessao_offline({ n: offlineGroups.length })}
        {#if !showOffline}<span class="grp-offline-names">({offlineGroups.map((g) => g.label).join(', ')})</span>{/if}
      </button>
    {/if}
  </nav>

  {#if expanded && model.selectMode}
    <!-- Composer compacto do broadcast (feature #9): so texto + enviar, sem anexos/slash-UI (isso
         fica no Composer normal, por sessão). Slash-command desabilita o envio (rota por sessão). -->
    <div class="broadcast-bar">
      <div class="broadcast-row">
        <span class="broadcast-count">{model.selected.size === 1 ? m.lista_selecionada_1() : m.lista_selecionadas({ n: model.selected.size })}</span>
        <button class="broadcast-compare" onclick={model.openCompare} disabled={model.compareDisabled} aria-label={m.lista_comparar_selecionadas()} title={m.lista_comparar()}>{m.lista_comparar()}</button>
        <button class="broadcast-cancel" onclick={model.toggleSelectMode} aria-label={m.lista_cancelar_selecao()}>×</button>
      </div>
      {#if model.broadcastMsg}<p class="broadcast-msg">{model.broadcastMsg}</p>{/if}
      <div class="broadcast-input-row">
        <input
          type="text"
          class="broadcast-input"
          bind:value={model.broadcastText}
          placeholder={m.lista_msg_selecionadas()}
          disabled={model.broadcastBusy}
          onkeydown={(e) => { if (e.key === 'Enter' && !model.broadcastDisabled) model.sendBroadcast(); }}
          aria-label={m.lista_broadcast_msg()}
        />
        <button class="broadcast-send" onclick={model.sendBroadcast} disabled={model.broadcastDisabled} aria-label={m.lista_enviar()}>
          {#if model.broadcastBusy}
            …
          {:else}
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <line x1="22" y1="2" x2="11" y2="13"/>
              <polygon points="22 2 15 22 11 13 2 9 22 2"/>
            </svg>
          {/if}
        </button>
      </div>
      {#if model.broadcastIsSlash}
        <p class="broadcast-hint">{m.lista_broadcast_aviso()}</p>
      {/if}
    </div>
  {/if}

  <!-- Rodapé (estilo Claude): botão da conta (avatar -> menu de conta) + CTA "Nova sessão". Tudo que
       era config/conta (servidores, notificações, horas silenciosas, reconectar, sair) vive no menu. -->
  <div class="side-foot" class:rail={!expanded}>
    <!-- A engrenagem e o kebab MUDARAM pra barra do topo (10/08/2026, decisão do usuário):
         a barra é permanente, então os comandos do app moram nela, num lugar só. O ponto do
         servidor ativo foi junto com a engrenagem (SessionTabs). Aqui fica só o que é do
         trilho: criar sessão e recolher/expandir. -->
    <button class="cta-new" onclick={abrirCriar} aria-label={m.sessao_nova()} title={m.sessao_nova()}>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
      {#if expanded}<span>{m.lista_nova_curto()}</span>{/if}
    </button>
    <!-- Recolher e a ULTIMA linha da barra: identidade no topo, chrome do app no rodape.
         Nao ficou no cabecalho porque os tres controles a direita espremiam o nome ("Han…"),
         e nao no meio do rodape porque entre as sessoes e o avatar ele lia como item solto. -->
    <button class="fold-btn" disabled={sidebarPin.forcedOverride === true}
      onclick={() => { if (sidebarPin.forcedOverride === true) return; sidebarPin.toggleUser(); }}
      aria-label={sidebarPin.forcedOverride === true
        ? m.sessao_barra_recolhida_quadro()
        : (sidebarPin.collapsed ? m.sessao_expandir_barra() : m.sessao_recolher_barra())}
      title={sidebarPin.forcedOverride === true
        ? m.sessao_quadro_recolhe()
        : (sidebarPin.collapsed ? m.sessao_expandir() : m.sessao_recolher())}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
        <rect x="3" y="4" width="18" height="16" rx="2"/>
        <line x1="9" y1="4" x2="9" y2="20"/>
      </svg>
      {#if expanded}<span class="fold-label">{m.sessao_recolher()}</span>{/if}
    </button>
  </div>

  <!-- Drag na borda direita pra redimensionar: so expandida (no trilho de 56px o handle de 6px
       flutuaria sobre a borda — era `{#if !collapsed}` no original). -->
  {#if expanded}
  <div class="resize-handle" onpointerdown={resizeStart} onpointermove={resizeMove}
    onpointerup={resizeEnd} onpointercancel={resizeEnd}
    role="separator" aria-label={m.sessao_redimensionar()} aria-orientation="vertical"></div>
  {/if}
</aside>
{/if}

<!-- Espiada no hover. FORA do <aside> pelo mesmo motivo do menu de contexto: a sidebar tem
     overflow:hidden (e backdrop-filter no modo liquid, que vira containing block) e clipa o popover. -->
{#if hp}<HoverPreview text={hp.text} x={hp.x} y={hp.y} />{/if}

<CreateSessionSheet open={showCreate} {servers} onClose={() => (showCreate = false)} onCreate={handleCreate} onOpenSession={abrirSessaoDoSheet} bastao={bastaoAlvo} />

<!-- "Buscar conversas" (nav): switcher em modo só-busca (busca de conteúdo cross-servidor, feature #10). -->
<SessionSwitcherSheet
  open={searchOpen}
  searchOnly
  sessions={[]}
  currentName=""
  onPick={(name) => { searchOpen = false; onSelect(name); }}
  onNew={() => { searchOpen = false; abrirCriar(); }}
  onClose={() => (searchOpen = false)}
/>

<!-- Popover do kebab "⋯" (header): nav secundária + agrupamento. Renderizado FORA do <aside> pra
     escapar o backdrop-filter da sidebar (mesmo motivo do menu de contexto). Ancorado via kebabPos. -->
{#if kebabOpen}
  <div class="menu-backdrop" onclick={closeKebab} role="presentation"></div>
  <div class="kebab-menu" style="left: {kebabPos.left}px; top: {kebabPos.top}px;" role="menu">
    <button type="button" role="menuitem" class="kebab-item" onclick={() => { closeKebab(); searchOpen = true; }}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>
      {m.lista_buscar()}
    </button>
    <button type="button" role="menuitem" class="kebab-item" onclick={() => { closeKebab(); window.location.hash = '#/archive'; }}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="4" rx="1"/><path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8"/><path d="M10 12h4"/></svg>
      {m.nav_arquivo()}
    </button>
    <button type="button" role="menuitem" class="kebab-item" onclick={() => { closeKebab(); window.location.hash = '#/costs'; }}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 3v18h18"/><path d="M7 14l4-4 3 3 5-6"/></svg>
      {m.nav_custos()}
    </button>
    <button type="button" role="menuitem" class="kebab-item" onclick={() => { toggleCompact(); closeKebab(); }}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M3 6h18"/><path d="M3 12h18"/><path d="M3 18h18"/></svg>
      {compact ? m.sessao_densidade_normal() : m.sessao_densidade_compacta()}
    </button>
    <div class="ctx-sep"></div>
    <div class="kebab-group-label">{m.lista_agrupar()}</div>
    <!-- Sempre visivel: agrupar era imposto ("projeto" fixo com 1 servidor) e nao havia como pedir a
         lista lisa. "Servidor" so entra com 2+ servidores, onde separa alguma coisa. -->
    <div class="group-toggle" role="radiogroup" aria-label={m.lista_agrupar()}>
      <button type="button" class:active={model.groupBy === 'none'} role="radio" aria-checked={model.groupBy === 'none'} onclick={() => model.setGroupBy('none')}>{m.lista_agrupar_nenhum()}</button>
      {#if servers.length >= 2}
        <button type="button" class:active={model.groupBy === 'server'} role="radio" aria-checked={model.groupBy === 'server'} onclick={() => model.setGroupBy('server')}>{m.lista_agrupar_servidor()}</button>
      {/if}
      <button type="button" class:active={model.groupBy === 'project'} role="radio" aria-checked={model.groupBy === 'project'} onclick={() => model.setGroupBy('project')}>{m.lista_agrupar_projeto()}</button>
    </div>
  </div>
{/if}

<svelte:window onpointermove={hpGuard} onkeydown={(e) => { if (e.key === 'Escape') { if (kebabOpen) closeKebab(); else if (menu) closeMenu(); else if (model.resumeCandidates) model.resumeCandidates = null; else if (model.confirmDel) model.confirmDel = null; else if (confirmBranch) confirmBranch = null; } }} />

<!-- Menu de contexto (botao direito na sessao). Backdrop + itens vivem no componente; o Sidebar so
     guarda posicao/alvo em `menu` e decide o que dirty->confirm / checkout / GitSheet fazem. -->
{#if menu}
  {@const m = menu}
  <SessionContextMenu x={m.x} y={m.y} name={m.name} serverId={m.serverId} cwd={m.cwd} thenTarget={m.thenTarget}
    chainCandidates={chainCandidates(m.serverId, m.name)}
    onClose={closeMenu}
    onRename={menuRename} onDelete={menuDelete} onGit={menuGit} onBastao={menuBastao}
    onPickBranch={(branch, dirty) => {
      if (dirty) confirmBranch = { name: m.name, serverId: m.serverId, branch };
      else doCheckout(m.name, m.serverId, branch);
    }}
    onFlash={flash} />
{/if}
{#if menuMsg}<div class="menu-toast" role="status">{menuMsg}</div>{/if}

<!-- Renomear com a sidebar RECOLHIDA: a <aside> está fora do DOM (sem edição inline) — diálogo
     acessível com input, mesmo padrão do modal de Adicionar servidor. Enter confirma, Esc
     cancela (ModalDialog). -->
{#if renameDialog}
  {@const rd = renameDialog}
  <ConfirmDialog title={m.sessao_renomear()} aria={m.sessao_renomear()} role="dialog"
    fallbackFocus={acctBtnEl}
    initialFocus={renameInputEl}
    onClose={() => (renameDialog = null)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (renameDialog = null) },
      { label: m.ctx_renomear(), kind: 'primary', disabled: renameBusy || !renameValue.trim() || renameValue.trim() === rd.old, onClick: submitRenameDialog },
    ]}>
    <input
      class="rename-dialog-input"
      bind:this={renameInputEl}
      bind:value={renameValue}
      use:autofocus
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      aria-label={m.sessao_novo_nome()}
      aria-invalid={renameError ? true : undefined}
      aria-describedby={renameError ? 'rename-dialog-err' : undefined}
      onkeydown={(e) => {
        if (e.key === 'Enter' && !renameBusy && renameValue.trim() && renameValue.trim() !== rd.old) {
          submitRenameDialog();
        }
      }}
    />
    {#if renameError}<p id="rename-dialog-err" class="rename-dialog-err" role="alert">{renameError}</p>{/if}
  </ConfirmDialog>
{/if}

<!-- Gerenciador git aberto pelo menu de contexto (repo da sessao, sem abrir o chat). -->
{#if model.gitSheet}
  <Git open={true} sessionName={model.gitSheet.name} desktop={true} {filesInContext} onClose={model.closeGit} />
{/if}


<!-- Confirmar exclusao (com o nome) — modal centrado, so desktop (sidebar e desktop-only). -->
{#if model.confirmDel}
  <ConfirmDialog title={m.sessao_excluir()} aria={m.sessao_excluir()}
    fallbackFocus={acctBtnEl}
    onClose={() => (model.confirmDel = null)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (model.confirmDel = null) },
      { label: m.sessao_excluir_curto(), kind: 'danger', onClick: doDelete },
    ]}>
    <p class="confirm-name">{model.confirmDel.name}</p>
    <!-- Um mesmo nome pode existir em varios servidores: mostra o dono pra a exclusao nao ser ambigua. -->
    {#if servers.length > 1}
      {@const srv = servers.find((s) => s.id === model.confirmDel?.serverId)}
      {#if srv}
        <p class="confirm-srv"><span class="confirm-srv-dot" style="background: {serverColor(srv.id)};" aria-hidden="true"></span>{srv.label}</p>
      {/if}
    {/if}
  </ConfirmDialog>
{/if}

<!-- Confirmar troca de branch com working tree suja (switch carrega mudancas nao-commitadas). -->
{#if confirmBranch}
  <ConfirmDialog title={m.sessao_trocar_branch_sujo()} aria={m.sessao_trocar_branch_sujo()}
    fallbackFocus={acctBtnEl}
    onClose={() => (confirmBranch = null)}
    actions={[
      { label: m.comum_cancelar(), onClick: () => (confirmBranch = null) },
      { label: m.sessao_trocar_assim(), onClick: () => { const c = confirmBranch; confirmBranch = null; if (c) doCheckout(c.name, c.serverId, c.branch); } },
      { label: m.sessao_guardar_trocar(), kind: 'primary', onClick: () => { const c = confirmBranch; confirmBranch = null; if (c) stashAndCheckout(c.name, c.serverId, c.branch); } },
    ]}>
    <p class="confirm-name">→ {confirmBranch.branch}</p>
    <p class="confirm-hint">{m.sessao_mudancas_nao_commitadas()} <strong>{m.sessao_guardar_trocar()}</strong> {m.sessao_poe_stash()} <strong>{m.sessao_trocar_assim()}</strong> {m.sessao_git_pode_recusar()}</p>
  </ConfirmDialog>
{/if}

<!-- Retomar conversa — caso AMBIGUO (varias sessoes no mesmo cwd): escolher qual transcript retomar.
     Paridade com o BottomSheet de resume do mobile (SessionList), no estilo dos modais do desktop. -->
{#if model.resumeCandidates}
  {@const rm = model.resumeCandidates}
  <ConfirmDialog title={m.sessao_retomar_qual()} aria={m.sessao_retomar()} role="dialog" wide
    fallbackFocus={acctBtnEl}
    onClose={() => (model.resumeCandidates = null)}
    actions={[{ label: m.sessao_fechar(), onClick: () => (model.resumeCandidates = null) }]}>
    <p class="confirm-hint">{m.sessao_multiplas_pasta()} <strong>{rm.name}</strong>.</p>
    {#if model.resumeError}<p class="resume-err">{model.resumeError}</p>{/if}
    <ul class="resume-list">
      {#each rm.candidates as c (c.session_id)}
        <li>
          <button
            type="button"
            class="resume-item"
            disabled={c.in_use || model.resumeBusy === rm.name}
            onclick={() => handleResume(rm.name, rm.serverId, c.session_id)}
          >
            <span class="resume-item-preview">{c.preview || m.sessao_sem_previa()}</span>
            <span class="resume-item-meta">
              {fmtWhen(c.mtime)}{#if c.in_use} · {m.sessao_em_uso()}{/if}
            </span>
          </button>
        </li>
      {/each}
    </ul>
  </ConfirmDialog>
{/if}

<style>
  .sidebar {
    position: relative;   /* ancora o resize-handle */
    /* ACIMA da coluna do chat, pra a SOMBRA desta barra desenhar por cima dela. Sem isto a coluna do
       chat, que vem depois no DOM e pinta fundo opaco, cobria a metade direita da sombra e sobrava
       só a esquerda — degradê suave de um lado, corte reto exatamente na fronteira das duas colunas.
       Lido como "o trilho tem outro fundo", e não era: medido na tela do usuário em 10/08/2026,
       .shell-linha, .message-list, .chat-screen e body davam TODOS rgb(248,246,242). Cor idêntica,
       sombra pela metade. A sombra é grande de propósito no modo caixa solta
       (0 18px 44px rgba(0,0,0,.34)), então ela invade bastante a coluna vizinha. */
    z-index: 1;
    width: 270px;
    flex-shrink: 0;
    height: 100%;
    display: flex;
    flex-direction: column;
    /* Glass desktop: fundo quase opaco SEM blur (mesma linha do composer/navbar — consistência +
       zero custo de backdrop-filter). Sheen no topo + brilho de borda mantêm a cara de vidro. */
    background: transparent;   /* o fundo mora no leaf ::before (vidro) */
    /* Costura com o chat em 1px sutil. O glass tinha border-right + um highlight interno de 1px;
       em crop/zoom aquilo virava uma "barra" de 3px na borda da sidebar. */
    border-right: 1px solid var(--border-subtle);
    box-shadow: inset 0 1px 1px var(--glass-specular);   /* rim no topo */
    padding: var(--space-3);
    gap: var(--space-2);
    transition: width 160ms var(--ease-out);
    overflow: hidden;
  }
  /* Enquanto arrasta: sem transicao (segue o ponteiro sem lag). */
  .sidebar.resizing { transition: none; }
  .resize-handle {
    position: absolute; top: 0; right: 0; width: 6px; height: 100%;
    cursor: col-resize; z-index: 6; touch-action: none;
  }
  @media (hover: hover) {
    .resize-handle:hover { background: var(--accent-dim); }
  }
  /* Chromium (data-liquid): refracao SVG real (liquid). No desktop a sidebar fica AO LADO do chat
     (nada atras pra refratar) -> efeito sutil; mais visivel quando ha conteudo atras. */
  :global(html[data-liquid]) .sidebar {
    /* Fundo NÃO vai aqui: ele já mora no ::before (regra abaixo). Com os dois, o mesmo 0.46 pintava
       duas vezes (~0.71 efetivo) e a sidebar ficava parede ao lado do composer, que só tem o leaf. */
    background: transparent;
    backdrop-filter: url(#liquid-glass) blur(20px) saturate(180%);
  }
  /* Vidro da sidebar: mesmo leaf do composer. O ::before carrega a COR; o `backdrop-filter` do
     liquid fica no ELEMENTO (regra acima) — medido no browser: `getComputedStyle(.sidebar)` traz
     `url(#liquid-glass) blur(20px)` e o ::before vem `none`. Consequência: no Chromium com
     data-liquid a sidebar É containing block de `position: fixed`, então tudo que ela abre e
     precisa cobrir a janela sai daqui por portal — BottomSheet.svelte e ModalDialog.svelte já
     fazem isso. Sem o portal, a sheet de Aparência nascia com 309px (a largura da sidebar) e
     encolhia junto quando o mouse saía. */
  .sidebar::before {
    content: "";
    position: absolute;
    inset: 0;
    z-index: -1;
    border-radius: inherit;
    pointer-events: none;
    background: var(--glass-bg-solid);
  }
  /* Com liquid (Chromium) o vidro é o MESMO do composer: `--glass-bg`, não `--glass-panel`. O 0.86/
     0.70 do panel é rede de segurança pra quando o engine descarta o backdrop-filter (app.css:52) —
     aqui o filtro está no elemento e o Chromium não descarta, então a rede só empilhava um segundo
     fundo por cima do primeiro e virava parede. Fora do liquid (WebKit, sem filtro) o panel fica.
     SÓ NO ESCURO: no claro o vidro é branco a 0.52 sobre texto ESCURO, e 0.52 de branco não apaga
     tinta escura — o chat atravessava a sidebar inteira (medido). No escuro é tinta clara sobre
     vidro escuro, que some. Por isso o claro fica no --glass-panel (0.90/0.695 com foto de fundo). */
  :global(html[data-liquid]) .sidebar::before {
    background: var(--glass-panel);
  }
  :global(html[data-liquid][data-theme='dark']) .sidebar::before {
    background: var(--glass-bg);
  }

  .sidebar.collapsed { width: 56px; padding: var(--space-3) var(--space-2); }

  /* Aparência → Painéis → "Soltos" (o padrão): a sidebar deixa de ser parede colada e vira card,
     mesmo tratamento do painel de contexto (DesktopSessionContext.svelte:287) e do dock recolhido
     logo abaixo. `height: auto` + as margens: o flex do shell estica o item pra altura do container
     MENOS as margens — com `height: 100%` ele vazaria 2×space-3 pra fora da tela.
     `:not(.floating)` porque o dock recolhido já é card por conta própria, com altura de conteúdo.
     Em "Colados" a regra não casa e volta a parede de ponta a ponta, como era antes. */
  :global(html:not([data-panels='edge'])) .sidebar:not(.floating) {
    height: auto;
    margin: var(--space-3) 0 var(--space-3) var(--space-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-xl);
    box-shadow: 0 18px 44px rgba(0, 0, 0, 0.34), inset 0 1px 1px var(--glass-specular);
  }

  /* Dock flutuante: com o pin recolhido a sidebar deixa de ser uma parede de ponta a ponta e vira
     uma peca com a ALTURA DO CONTEUDO, centralizada na esquerda. Segue no fluxo do flex do shell
     (o chat nao passa por baixo dela); quem faz a altura encolher e o `align-self: center` — sem
     ele o item de flex estica pra altura toda. Muitas sessoes: o teto de max-height entra e a
     lista rola por dentro, entao o dock nunca encosta nas bordas. */
  .sidebar.floating {
    align-self: center;
    height: auto;
    max-height: calc(100% - var(--space-8));
    margin-left: var(--space-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-xl);
    box-shadow: var(--elev-3);
  }
  /* Aparência → Painéis → "Colados" alcança o dock TAMBÉM (18/08). Antes nenhuma regra mirava o
     `.floating`: quem escolhia "Colados" via a sidebar expandida virar parede e o dock continuar
     boiando com canto e sombra — a opção valia pra dois dos três painéis. Encostado na borda ele
     também não tem margem. */
  :global(html[data-panels='edge']) .sidebar.floating {
    align-self: stretch;
    height: 100%;
    max-height: none;
    margin-left: 0;
    border: 0;
    border-right: 1px solid var(--border-subtle);
    border-radius: 0;
    box-shadow: none;
  }
  /* A lista so rola quando bate no teto: com 2 sessoes o dock e curto e sem barra. */
  .sidebar.floating .sess-list { flex: 0 1 auto; }

  /* ── Vocabulario do trilho (desenho ORIGINAL, restaurado de 0d9ffc5) ─────
     Um item, uma medida: 36px de lado pra TUDO que e acao (recolher, ⋯, sessao, nova sessao) e o
     mesmo raio. Antes conviviam 36px de icone, 48px de linha de sessao e 36px de botao redondo, com
     tres raios — a coluna parecia desalinhada mesmo com todos os itens centralizados. O circulo
     fica reservado ao avatar da conta: circulo = pessoa, quadrado arredondado = acao. */
  .sidebar.collapsed .sess-main { min-height: 36px; }
  /* Sessao ABERTA no trilho: anel accent na moldura da linha. O realce era um fundo accent a 10%
     atras de um quadrado que cobre a linha toda — invisivel. O anel vai por dentro (inset) pra nao
     estourar os 40px uteis do dock, e nao disputa com o box-shadow das iniciais (travada/ocupada). */
  /* Selecionado: BARRA na borda, não anel. O anel agora é vocabulário de estado; usar anel de
     accent aqui fazia "selecionado" e "trabalhando" competirem pelo mesmo desenho e pela mesma cor. */
  .sidebar.collapsed .sess-row.active {
    background: var(--accent-dim);
    box-shadow: inset 3px 0 0 0 var(--accent);
  }
  .sidebar.collapsed .sess-list { gap: var(--space-1); }
  /* 36px em TODAS as caixas visiveis do trilho. Com iniciais/avatar a 32 e os botoes a 36, a coluna
     lia como desalinhada: os itens estavam centrados, mas com larguras diferentes. */
  .sidebar.collapsed .initials { width: 36px; height: 36px; border-radius: var(--radius-md); }
  .sidebar.collapsed .side-foot.rail .cta-new {
    height: 36px;
    border-radius: var(--radius-md);
  }
  /* Rodape do dock: mesma distancia entre itens que o resto do trilho, e a regua encostada nas
     bordas internas do dock em vez de uma linha curta boiando no meio. */
  .sidebar.floating .side-foot.rail {
    gap: var(--space-1);
    margin: var(--space-1) calc(var(--space-2) * -1) 0;
    padding: var(--space-2) var(--space-2) 0;
  }

  /* ── Polish: foco de teclado + transições de estado ──────────────────────
     Foco visível (dev-tool: anel accent), inset pra nao ser cortado pelo overflow da sidebar/lista.
     Cobre todo controle do componente (sidebar + menus/modais). Inputs ja sinalizam foco pela borda. */
  button:focus-visible,
  [role="separator"]:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  /* Transições de hover/estado consistentes (150–300ms): cor/fundo/opacidade suaves, mantendo o
     press-scale (transform) do reset global — este seletor mais específico o sobrescreveria. Só nos
     controles da sidebar; menus de contexto/modais têm hover próprio. */
  .sidebar button,
  .sidebar .sess-row {
    transition: background-color 180ms var(--ease-out),
                color 180ms var(--ease-out),
                opacity 180ms var(--ease-out),
                transform 160ms var(--ease-out);
  }

  .side-top { display: flex; align-items: center; gap: var(--space-2); min-height: 36px; }
  /* Linha propria sob o header: o seletor de view separa "onde estou" da lista de sessoes. */
  .side-views { display: flex; margin-top: var(--space-2); }
  /* Rail recolhido: o header empilha (recolher em cima, kebab embaixo) — a nav secundária segue
     acessível num toque sem precisar expandir. */
  .sidebar.collapsed .side-top { flex-direction: column; gap: var(--space-2); }
  /* O rodape e LINHA quando aberto e COLUNA no rail (.side-foot.rail), entao o botao muda de forma:
     aberto ele e um icone compacto ao lado do CTA; no rail ocupa a largura do trilho. */
  .fold-btn {
    display: inline-flex; align-items: center; justify-content: center; gap: var(--space-2);
    flex: 0 0 auto; width: 36px; height: 36px; padding: 0;
    background: transparent; border: 0; border-radius: var(--radius-md);
    color: var(--text-muted); cursor: pointer;
  }
  .sidebar.collapsed .fold-btn { width: 100%; }
  @media (hover: hover) { .fold-btn:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); } }
  .fold-btn:disabled { color: var(--text-muted); opacity: 0.55; cursor: default; }
  /* o rotulo so aparece se sobrar espaco: no rodape em linha o icone ja basta */
  .fold-label { display: none; }
  .row-mark { display: inline-flex; }
  .side-mark { display: flex; align-items: center; color: var(--accent); flex: 0 0 auto; }
  .side-brand { flex: 1; min-width: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  /* Toggle do modo de seleção: mesma caixa de 36px dos outros controles do header. */
  .select-toggle-btn {
    flex-shrink: 0; width: 36px; height: 36px;
    border-radius: var(--radius-md); color: var(--text-secondary);
  }
  .select-toggle-btn:hover { background: var(--bg-hover); }
  .select-toggle-btn.active { color: var(--accent); background: var(--accent-dim); }

  /* ── Kebab "⋯" do header + seu popover (nav secundária + agrupamento) ── */
  /* Popover do kebab: mesmo visual do menu de contexto (bg/borda/sombra), aberto pra baixo. */
  .kebab-menu {
    position: fixed; z-index: 41; min-width: 220px; padding: 4px;
    display: flex; flex-direction: column;
    background: var(--surface-raised); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); box-shadow: var(--elev-2);
  }
  .kebab-item {
    display: flex; align-items: center; gap: var(--space-3);
    width: 100%; height: 40px; padding: 0 10px;
    justify-content: flex-start; text-align: left;
    color: var(--text-primary); font-size: var(--text-sm); border-radius: var(--radius-sm);
  }
  .kebab-item svg { flex-shrink: 0; color: var(--text-secondary); }
  .kebab-item:hover { background: var(--bg-hover); }
  .kebab-group-label {
    /* Receita unificada de rotulo de secao (tokens de app.css) — era 12px/600/0.04em. */
    font-size: var(--label-size); font-weight: var(--label-weight); letter-spacing: var(--label-tracking); text-transform: uppercase;
    color: var(--text-muted); padding: var(--space-2) 10px var(--space-1);
  }
  /* Toggle dentro do popover: zera a margem que tinha quando ficava na lista. */
  .kebab-menu .group-toggle { margin: 0 6px 4px; }

  .sess-list { flex: 1; overflow-y: auto; display: flex; flex-direction: column; gap: 2px; margin-top: var(--space-2); }
  /* Toggle Servidor|Projeto (feature #3): controle segmentado COMPACTO e subordinado — inline (nao
     ocupa a largura toda), baixo e discreto, um agrupador silencioso e nao um CTA. min-height:0
     derruba o piso global de 44px dos botoes. So aparece com >=2 servidores. */
  .group-toggle {
    display: inline-flex; align-self: flex-start; gap: 2px; padding: 2px; margin: 0 0 var(--space-2);
    background: var(--surface-inset); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
  }
  .group-toggle button {
    min-height: 0; height: 24px; min-width: 0; padding: 0 var(--space-2);
    border-radius: var(--radius-sm); font-size: var(--text-xs); font-weight: 500; color: var(--text-muted);
  }
  @media (hover: hover) { .group-toggle button:hover { color: var(--text-secondary); } }
  .group-toggle button.active { background: var(--surface-raised); color: var(--text-primary); font-weight: 600; }
  /* Filtro (paridade com o mobile) — compacto, alinhado ao conteudo da sidebar. */
  .filter-input {
    width: 100%; height: 32px; margin: 0 0 var(--space-2); padding: 0 var(--space-2);
    background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
    color: var(--text-primary); font-family: var(--font-ui); font-size: var(--text-sm); outline: none;
    transition: border-color 160ms var(--ease-out);
  }
  .filter-input::placeholder { color: var(--text-muted); }
  .filter-input:focus { border-color: var(--accent); }
  .filter-empty { font-size: var(--text-xs); color: var(--text-muted); text-align: center; padding: var(--space-4) var(--space-2); }
  /* Header do grupo virou uma row (label + "enviar p/ todas", feature #9). */
  .grp-head-row { display: flex; align-items: center; }
  .grp-head-row:not(:first-child) { margin-top: var(--space-2); }
  .grp-head-row .grp-head { flex: 1; min-width: 0; }
  .grp-head {
    display: flex; align-items: center; gap: var(--space-2);
    width: 100%; text-align: left;
    padding: var(--space-2) var(--space-2) 4px;
    /* Receita unificada de rotulo de secao (tokens de app.css) — era 12px/600/0.04em. */
    font-size: var(--label-size); font-weight: var(--label-weight); color: var(--text-muted);
    text-transform: uppercase; letter-spacing: var(--label-tracking); border-radius: var(--radius-sm);
  }
  @media (hover: hover) { .grp-head:hover { color: var(--text-secondary); } }

  /* Sub-header do cluster de pareamento (Opção C): recuado sob o servidor, cor accent, colapsável.
     padding-left: var(--space-4) → chevron na coluna x=16px, mesmo eixo da borda-accent dos
     .pair-member logo abaixo (8px de margin + 3px de border + padding da .sess-main = label em
     ~19px; o texto do pair-head cai na mesma coluna porque o gap+16px de padding fecham a conta). */
  .pair-head {
    display: flex; align-items: center; gap: var(--space-2);
    width: 100%; text-align: left;
    padding: 4px var(--space-2) 4px calc(var(--space-4) + 3px);
    background: none; border: none; cursor: pointer;
    font-size: var(--text-xs); font-weight: 600; color: var(--accent);
    border-radius: var(--radius-sm);
  }
  .pair-head-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  @media (hover: hover) { .pair-head:hover { background: var(--bg-hover); } }
  /* Linha de MEMBRO do cluster: o recuo liga as sessões do grupo. Escopo `:not(.collapsed)` porque
     no trilho de 56px o `margin-left` empurrava as iniciais 10px pra direita — as sessões pareadas
     saíam do eixo dos outros itens do dock, e o agrupamento nem se lê lá (não há sub-header). */
  /* A borda accent do pair-member herda --space-2 de margin, mas a .sess-row recebe também
     border-left 3px no escopo :not(.collapsed) (awaiting). Somando: borda começa em 8px e label em
     ~19px — sobrando 3px a menos que o pair-head, que começa a escrever em 19px+ (ver .pair-head).
     Muover a borda para dentro da .sess-row alinha a faixa com o cabeçalho. */
  .sidebar:not(.collapsed) .sess-row.pair-member { margin-left: calc(var(--space-2) - 3px); }
  .grp-chevron {
    flex-shrink: 0; font-size: 9px; color: var(--text-muted);
    transition: transform 160ms var(--ease-out);
  }
  .grp-chevron.collapsed { transform: rotate(-90deg); }
  .grp-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; }
  .grp-label { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  /* Contagem de sessoes do grupo (paridade com o mobile). */
  .grp-count {
    flex-shrink: 0; text-transform: none; letter-spacing: 0; font-weight: 600;
    color: var(--text-muted); background: var(--surface-inset);
    border-radius: var(--radius-full); padding: 0 6px; min-width: 18px; text-align: center;
    font-variant-numeric: tabular-nums;
  }
  /* Badge de aguardando (âmbar) — numero + title "N aguardando". */
  .grp-await {
    flex-shrink: 0; text-transform: none; letter-spacing: 0; font-weight: 700;
    color: var(--warning); background: rgba(255, 159, 10, 0.14);
    border-radius: var(--radius-full); padding: 0 6px; min-width: 18px; text-align: center;
    font-variant-numeric: tabular-nums;
  }
  .grp-off { color: var(--warning); font-weight: 600; text-transform: none; letter-spacing: 0; }
  /* Linha-resumo dos servidores offline (colapsados fora da lista). */
  .grp-offline-sum {
    display: flex; align-items: center; gap: var(--space-2);
    width: 100%; min-height: 34px; padding: 0 var(--space-2); margin-top: var(--space-2);
    border-radius: var(--radius-md); font-size: var(--text-xs); font-weight: 600;
    color: var(--warning); text-align: left;
  }
  .grp-offline-sum:hover { background: var(--bg-hover); }
  .grp-offline-names {
    color: var(--text-muted); font-weight: 500; overflow: hidden;
    text-overflow: ellipsis; white-space: nowrap; min-width: 0;
  }
  .grp-broadcast {
    flex-shrink: 0; width: 24px; height: 24px; margin-right: var(--space-1);
    color: var(--text-muted); font-size: var(--text-xs); border-radius: var(--radius-sm);
  }
  .grp-broadcast:hover { color: var(--accent); background: var(--bg-hover); }
  /* Broadcast por grupo é ação secundária: some até o hover/foco pra não repetir um aviãozinho em
     todo header. Continua alcançável por teclado via focus-within. */
  @media (hover: hover) {
    .grp-broadcast { opacity: 0; }
    .grp-head-row:hover .grp-broadcast,
    .grp-head-row:focus-within .grp-broadcast { opacity: 1; }
  }
  /* Checkbox do modo seleção: so decorativo (o toque na row inteira alterna). */
  .select-check { width: 16px; height: 16px; accent-color: var(--accent); pointer-events: none; }
  .sess-row { display: flex; align-items: center; border-radius: var(--radius-md); }
  /* hover SÓ em dispositivo com mouse. No touch (tablet), o :hover fazia o 1º toque virar "hover" e
     o 2º o clique -> precisava de 2 toques pra abrir a sessão. hover:hover isola isso. */
  @media (hover: hover) { .sess-row:hover { background: var(--bg-hover); } }
  .sess-row.active {
    background: color-mix(in srgb, var(--accent) 10%, transparent);
  }
  .sess-row.active .sess-name { color: var(--text-primary); font-weight: 650; }
  .sess-row.active .branch,
  .sess-row.active .branch-inline { color: var(--accent); }
  /* Sessão aguardando resposta: realce âmbar — o olho acha sem ler chip por chip. Realce é EXCEÇÃO
     (só awaiting): a barra carrega o sinal e o tint fica no mínimo, pra não virar fundo colorido.
     Só na sidebar EXPANDIDA: no rail de 56px a barra descentralizaria as iniciais (que já vêm
     tingidas pelo estado, tornando o realce redundante lá). A borda transparente na base — e não só
     na .awaiting — é o que impede a row de pular 3px ao entrar/sair de awaiting. */
  .sidebar:not(.collapsed) .sess-row { border-left: 3px solid transparent; }
  /* Faixa esquerda disputada: o cluster de pareamento (accent) e o awaiting (âmbar) querem a MESMA
     borda. A base transparente acima é mais específica que o `.sess-row.pair-member` solto lá em cima
     e apagava a faixa do cluster justo na sidebar expandida — que é onde o cluster aparece. Reafirma a
     cor no mesmo escopo; awaiting vem DEPOIS e ganha no desempate por ordem: urgência > agrupamento
     (o membro ainda fica identificado pelo recuo e pelo 🤝 do sub-header). */
  .sidebar:not(.collapsed) .sess-row.pair-member { border-left-color: var(--accent-dim); }
  .sidebar:not(.collapsed) .sess-row.awaiting {
    border-left-color: var(--warning);
    background: color-mix(in srgb, var(--warning) 7%, transparent);
  }
  @media (hover: hover) {
    .sidebar:not(.collapsed) .sess-row.awaiting:hover { background: color-mix(in srgb, var(--warning) 12%, transparent); }
  }
  .sess-main {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2); min-height: 48px;
    padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--text-secondary);
    border-radius: var(--radius-md);
  }
  /* Modo compacto: só lead+nome+chips — cabe o dobro de sessões. Saem as 3 linhas secundárias
     (subtítulo de estado, cwd e branch). A altura mora na .sess-main (a .sess-row não tem
     min-height), então é ela que encolhe; os chips de estado/🤝 ficam, são o sinal da linha. */
  .sess-list.compact .sess-main { min-height: 36px; }
  .sess-list.compact .status-sub,
  .sess-list.compact .cwd,
  .sess-list.compact .branch { display: none; }
  .row-info { display: flex; flex-direction: column; gap: 1px; flex: 1; min-width: 0; }
  .name-row { display: flex; align-items: center; gap: var(--space-2); min-width: 0; }
  /* Subtítulo de estado vivo: a pergunta (awaiting) ou o texto do spinner (working), truncado —
     linha acionável sem abrir a sessão (feature #1). */
  .status-sub {
    min-width: 0;
    font-size: var(--text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status-sub.asking { color: var(--warning); font-weight: 600; }
  .status-sub.working { color: var(--text-secondary); font-style: italic; }
  .cwd { display: flex; min-width: 0; font-family: var(--font-mono); font-size: 10px; }
  .cwd-prefix { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-muted); }
  .cwd-base {
    /* encolhe COM ellipsis (era 0 0 auto e vazava por baixo dos chips em sidebar estreita) */
    flex: 0 1 auto; min-width: 3ch; overflow: hidden; text-overflow: ellipsis;
    white-space: nowrap; color: var(--text-secondary);
  }
  .badges-line {
    display: flex; align-items: center; gap: var(--space-1); flex-wrap: nowrap;
    min-width: 0; margin-top: 2px; overflow: hidden;
  }
  /* Branch git da linha: agora inline no cwd pra nao gastar uma linha inteira de metadado. */
  .branch,
  .branch-inline {
    min-width: 0; font-family: var(--font-mono); font-size: 10px; color: var(--text-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .branch-inline { flex-shrink: 0; margin-left: var(--space-1); }
  /* Marcador de worktree: chip com a palavra inteira, nunca trunca (quem cede largura é o cwd).
     Era um glifo ⧉ de 10px e não dava pra ver — dizer o nome custa 8 caracteres. */
  .wt {
    flex-shrink: 0;
    padding: 0 5px;
    border-radius: var(--radius-full);
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
    font-size: 9px;
    font-weight: 700;
    letter-spacing: 0.03em;
  }
  /* "+128 −24" do working tree (paridade com o SessionCard): mono, cores semânticas de diff. */
  .diff-stats {
    flex-shrink: 0;
    display: inline-flex;
    gap: 4px;
    font-family: var(--font-mono);
    font-size: 10px;
  }
  .diff-add { color: var(--success); }
  .diff-del { color: var(--error); }
  /* Envelope da pilula (o desenho dela vive no StateChip.svelte). */
  .state-chip { display: inline-flex; flex-shrink: 0; border-radius: var(--radius-full); }
  /* Travada (feature #7): anel âmbar sutil no chip — avisa sem gritar. Outline, e nao box-shadow
     inset: a sombra do envelope ficaria por baixo do fundo da pilula. */
  .state-chip.stalled {
    outline: 1px solid var(--warning); outline-offset: -1px;
  }
  /* Rate-limit radar (feature #8): chip proprio, mesma familia visual do stalled (âmbar, calmo). */
  .limited-chip {
    flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis;
    font-size: 10px; font-weight: 600; letter-spacing: 0.02em;
    padding: 2px 7px; border-radius: var(--radius-full); white-space: nowrap;
    color: var(--warning); background: rgba(255, 159, 10, 0.12);
    font-variant-numeric: tabular-nums;
  }
  /* Feature #12: indicador do vinculo 'then' — mesmo formato do limited-chip, cor neutra (accent). */
  .chain-chip {
    flex: 0 1 auto; min-width: 0; font-size: 10px; font-weight: 600; letter-spacing: 0.02em;
    padding: 2px 7px; border-radius: var(--radius-full); white-space: nowrap;
    max-width: 96px; overflow: hidden; text-overflow: ellipsis;
    color: var(--accent); background: var(--accent-dim);
  }
  /* Progresso do plano do superpowers (Task 3). */
  .plan-chip {
    padding: 1px 6px;
    border-radius: var(--radius-full);
    background: var(--accent-dim);
    color: var(--accent);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    /* O rotulo agora carrega o NOME do plano, que e longo e variavel: sem teto ele empurrava o resto
       da linha de chips pra fora. Corta o nome com reticencias e mantem a linha inteira. */
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 22ch;
  }
  .plan-chip--done {
    background: color-mix(in srgb, var(--success) 14%, transparent);
    color: var(--success);
  }
  /* Motor de modelo (Task 5): sessao rodando fora da conta Anthropic. */
  /* Provider da sessão (Codex/Pi) — só o que NÃO é Claude ganha chip. Tinta neutra de propósito:
     é rótulo de identidade, não estado; accent já é "motor" e âmbar já é "sem id". */
  .prov-chip {
    flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis;
    font-size: 10px; font-weight: 700; letter-spacing: 0.02em;
    color: var(--text-muted); background: var(--surface-raised);
    border: 1px solid var(--border-subtle);
    padding: 1px 6px; border-radius: var(--radius-full); white-space: nowrap;
    /* Glifo colorido na frente do texto (mesmo chip do SessionCard): sem flex o ícone quebrava a linha de base. */
    display: inline-flex; align-items: center; gap: 4px;
  }
  /* Claude (só a marca, sem texto): chip só-ícone, mais compacto que os com rótulo. */
  .prov-chip--so-icone { padding: 1px 3px; }
  /* Mesma etiqueta no rail de 56px: colada na base do avatar, absoluta (não mexe na altura da row).
     bottom: -3px e não -7px: com densidade compacta a row cai pra 34px e o avatar de 30px quase a
     preenche — a -7px a etiqueta passava da row e encostava no avatar de baixo. */
  .prov-rail {
    position: absolute; left: -4px; top: -4px;
    color: var(--text-secondary); background: var(--surface-raised);
    border: 1px solid var(--border-subtle);
    padding: 1px 3px; border-radius: var(--radius-full); white-space: nowrap;
    display: inline-flex; align-items: center;
  }
  .engine-chip {
    flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis;
    font-size: 10px; font-weight: 700; letter-spacing: 0.02em;
    color: var(--accent); background: var(--accent-dim);
    padding: 1px 6px; border-radius: var(--radius-full);
  }
  .lead { width: 18px; flex-shrink: 0; display: inline-flex; align-items: center; justify-content: center; }
  /* Rail recolhido: iniciais precisam de mais espaco que o icone de 18px. */
  .sidebar.collapsed .lead { width: auto; position: relative; }
  /* Rail recolhido — as INICIAIS dizem quem é; quem diz como está é o ponto do canto. Antes a cor
     do estado pintava a letra E o fundo do chip, então o olho lia "chip verde / chip roxo" em vez
     de ler a sessão. Superfície em --surface-raised (e não uma cor sólida) porque era o único
     elemento do trilho que não deixava o papel de parede passar. */
  .initials {
    width: 30px; height: 30px; border-radius: 8px;
    display: flex; align-items: center; justify-content: center;
    font-size: 11px; font-weight: 700; letter-spacing: 0.02em;
    color: var(--text-primary);
    background: var(--surface-raised);
    border: 1px solid var(--border);
  }
  /* O estado tem desenho PRÓPRIO agora — este ponto no canto de cima do avatar. Ele existe pros
     quatro estados, então "sem marca" deixou de ser um deles: antes ociosa era a ausência de anel,
     que é o mesmo desenho de uma sessão que ainda não reportou nada. A borda na cor do painel
     recorta o ponto do avatar em qualquer papel de parede. */
  .estado-ponto {
    position: absolute; right: -3px; top: -3px;
    width: 9px; height: 9px; border-radius: var(--radius-full);
    background: var(--cor, var(--text-secondary));
    border: 2px solid var(--bg-elevated);
    box-sizing: content-box;
  }
  /* Trabalhando: a marca do hangar com a animação que ela já tem na lista aberta. Fundo próprio
     (não transparente) porque são traços finos sobre a quina do avatar — sem ele os arcos se
     misturam com a borda e com o papel de parede. */
  .estado-marca {
    position: absolute; right: -6px; top: -6px;
    display: inline-flex; align-items: center; justify-content: center;
    padding: 1px; border-radius: var(--radius-full);
    background: var(--bg-elevated);
  }
  /* ÊNFASE POR URGÊNCIA — quem pulsa é quem precisa DE TI. Quem espera resposta ganha um halo largo
     e lento. O halo NUNCA vai a zero de alfa: sobre papel de parede movimentado ele sumia entre um
     pulso e outro. */
  .estado-ponto.aguardando {
    animation: rail-chama 2.2s var(--ease-out) infinite;
  }
  @keyframes rail-chama {
    0%, 100% { box-shadow: 0 0 0 2px color-mix(in srgb, var(--cor, transparent) 55%, transparent); }
    55%      { box-shadow: 0 0 0 7px color-mix(in srgb, var(--cor, transparent) 0%, transparent); }
  }
  .sidebar.collapsed .sess-row { justify-content: center; }
  /* position:relative pra ancorar a PlanBar compact (position:absolute) — sem isto ela flutua em
     relação ao body inteiro em vez de ficar na base desta linha. */
  .sidebar.collapsed .sess-main { justify-content: center; padding: 0; position: relative; }
  .sess-row.active .sess-main { color: var(--text-primary); }
  .sess-name { flex: 1; min-width: 0; font-size: var(--text-sm); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sess-main.untracked { opacity: 0.45; cursor: default; }
  /* Kimi "sem id" (pré-1º-prompt) ABRE o chat: cursor normal pra não mentir que a linha é inerte. */
  .sess-main.untracked.untracked-open { cursor: pointer; }
  .sess-badge {
    flex-shrink: 0; font-size: 10px; padding: 1px 5px; border-radius: var(--radius-sm);
    background: var(--surface-raised); border: 1px solid var(--border-subtle); color: var(--warning); white-space: nowrap;
  }
  .sess-edit {
    flex: 1; min-width: 0; height: 38px; padding: 0 var(--space-2);
    background: var(--surface-inset); border: 1px solid var(--accent); border-radius: var(--radius-md);
    color: var(--text-primary); font-size: var(--text-sm); outline: none;
  }
  /* Input do diálogo de rename (sidebar recolhida) — mesma família visual do .sess-edit. */
  .rename-dialog-input {
    width: 100%; height: 44px; padding: 0 var(--space-3);
    background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-md);
    color: var(--text-primary); font-family: var(--font-ui); font-size: var(--text-sm); outline: none;
  }
  .rename-dialog-input::placeholder { color: var(--text-muted); }
  .rename-dialog-input:focus { border-color: var(--accent); }
  .rename-dialog-input[aria-invalid='true'] { border-color: var(--error); }
  .rename-dialog-err { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--error); }
  @media (hover: hover) {
    .sess-kebab { display: none; }
    .sess-row:hover .sess-kebab, .sess-row:focus-within .sess-kebab { display: inline-flex; }
  }
  /* TOUCH (tablet/celular na sidebar): 3 botoes inline esmagavam o nome -> some git/loop/x,
     fica SO o kebab (abre o menu de contexto completo). Desktop com mouse: kebab nao existe,
     hover revela os botoes como antes. */
  .sess-kebab {
    display: none; width: 22px; height: 22px; min-height: 0; flex-shrink: 0;
    border-radius: var(--radius-sm); color: var(--text-muted);
    font-size: var(--text-base); line-height: 1;
    align-items: center; justify-content: center;
  }
  @media (hover: none) {
    .sess-kebab { display: inline-flex; opacity: 0.7; }
  }
  /* Retomar da linha "sem id": unica acao possivel -> SEMPRE visivel (nao hover-revealed), tingida de
     accent pra puxar o olho (a row inteira fica apagada com opacity 0.45). */
  .sess-resume {
    width: 22px; height: 22px; min-height: 0; flex-shrink: 0; border-radius: var(--radius-sm);
    display: inline-flex; align-items: center; justify-content: center;
    color: var(--accent); background: var(--accent-dim); margin-right: 2px;
  }
  .sess-resume:hover { background: var(--accent); color: #fff; }
  .sess-resume:disabled { opacity: 0.5; }

  /* Composer compacto do broadcast (feature #9): so texto + enviar, sem anexos/slash-UI. */
  .broadcast-bar {
    display: flex; flex-direction: column; gap: var(--space-2);
    padding-top: var(--space-2); margin-top: var(--space-2);
    border-top: 1px solid var(--border-subtle);
  }
  .broadcast-row { display: flex; align-items: center; justify-content: space-between; }
  .broadcast-count { font-size: var(--text-xs); font-weight: 600; color: var(--text-primary); }
  .broadcast-cancel { width: 24px; height: 24px; color: var(--text-secondary); font-size: var(--text-base); line-height: 1; border-radius: var(--radius-sm); }
  .broadcast-cancel:hover { background: var(--bg-hover); }
  .broadcast-compare {
    font-size: var(--text-xs); font-weight: 600; color: var(--accent);
    padding: 4px 10px; border: 1px solid var(--accent); border-radius: var(--radius-full);
    background: transparent;
  }
  .broadcast-compare:disabled { color: var(--text-muted); border-color: var(--border-default); }
  @media (hover: hover) { .broadcast-compare:not(:disabled):hover { background: var(--accent-dim); } }
  .broadcast-msg { font-size: var(--text-xs); color: var(--warning); margin: 0; }
  .broadcast-hint { font-size: var(--text-xs); color: var(--text-muted); margin: 0; }
  .broadcast-input-row { display: flex; gap: var(--space-2); }
  .broadcast-input {
    flex: 1; min-width: 0; height: 34px;
    background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
    color: var(--text-primary); font-size: var(--text-sm); padding: 0 var(--space-2); outline: none;
  }
  .broadcast-input:focus { border-color: var(--accent); }
  .broadcast-send {
    width: 34px; height: 34px; flex-shrink: 0;
    background: var(--accent); border-radius: var(--radius-sm); color: #fff; font-size: var(--text-sm);
  }
  .broadcast-send:disabled { background: var(--bg-hover); color: var(--text-muted); }

  /* ── Rodapé: engrenagem (Configurações) + CTA "Nova sessão" ── */
  .side-foot {
    display: flex; align-items: center; gap: var(--space-2);
    border-top: 1px solid var(--border-subtle); padding-top: var(--space-2); margin-top: var(--space-1);
  }
  .side-foot.rail { flex-direction: column; }
  /* Mesmo vocabulario dos chips de sessao: superficie que acompanha a transparencia + anel de
     accent pra dizer "este e voce". O gradiente anterior tinha um roxo CHUMBADO (#a06de0) que nao
     existe em token nenhum: com a paleta vinda do papel de parede, o accent mudava e ele nao. */
  /* NÃO é avatar de pessoa: é o SERVIDOR ativo (a engrenagem abre Configurações). Círculo com
     iniciais lia como gente — daí a pergunta "o que é esse NJ?". Quadrado arredondado é convenção
     de coisa, e o ponto no canto traz a cor daquele servidor, a mesma que já agrupa a lista. */
  .cta-new {
    display: flex; align-items: center; gap: var(--space-1); flex-shrink: 0;
    height: 36px; padding: 0 var(--space-3);
    background: var(--accent); color: var(--bg-base); font-size: var(--text-sm); font-weight: 600;
    border-radius: var(--radius-full); white-space: nowrap;
  }
  .cta-new svg { flex-shrink: 0; }
  .cta-new:hover { background: var(--accent-press); }
  .side-foot.rail .cta-new { width: 36px; padding: 0; justify-content: center; }

  /* ── Menu de contexto ── */
  /* .menu-backdrop e .ctx-sep ficam aqui porque o kebab do header tambem os usa; o resto do menu de
     contexto (.ctx-menu/.ctx-branch/.ctx-chain-*) migrou pro SessionContextMenu.svelte. */
  .menu-backdrop { position: fixed; inset: 0; z-index: 40; }
  .ctx-sep { height: 1px; margin: 4px 6px; background: var(--border-subtle); }

  /* ── Corpo dos modais de confirmação (o chassi vive em ConfirmDialog.svelte;
     estas classes alcançam nós do snippet, compilado no escopo deste componente). ── */
  .confirm-hint { font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.5; }
  .confirm-name {
    font-family: var(--font-mono); font-size: var(--text-sm); color: var(--text-secondary);
    padding: var(--space-2) var(--space-3); background: var(--surface-inset);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    word-break: break-all;
  }
  /* Servidor dono da sessao no confirm de exclusao (desambigua nome repetido entre servidores). */
  .confirm-srv { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-xs); color: var(--text-muted); }
  .confirm-srv-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; }

  /* Modal de resume (caso ambiguo): lista de transcripts candidatos pra escolher — o card fica mais
     largo (wide) que os confirms simples pra caber as previas. */
  .resume-err { font-size: var(--text-sm); color: var(--error); margin: 0; }
  .resume-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: var(--space-2); max-height: 50vh; overflow-y: auto; }
  .resume-item {
    width: 100%; text-align: left; display: flex; flex-direction: column; gap: 3px;
    padding: var(--space-3); background: var(--surface-inset);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md); cursor: pointer;
  }
  .resume-item:hover { background: var(--bg-hover); }
  .resume-item:disabled { opacity: 0.5; cursor: not-allowed; }
  .resume-item-preview { font-size: var(--text-sm); color: var(--text-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .resume-item-meta { font-size: var(--text-xs); color: var(--text-muted); }

  /* Banner efemero (resultado do git pull / erro do editor). */
  .menu-toast {
    position: fixed; z-index: 42; left: 50%; bottom: 20px; transform: translateX(-50%);
    max-width: min(520px, 90vw); padding: 8px 14px;
    background: var(--surface-raised); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); box-shadow: 0 6px 20px rgba(0,0,0,0.35);
    color: var(--text-primary); font-size: var(--text-sm); font-family: var(--font-mono);
    white-space: pre-wrap; word-break: break-word; max-height: 40vh; overflow-y: auto;
  }
</style>
