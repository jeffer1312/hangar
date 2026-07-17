<script lang="ts">
  import { onMount } from 'svelte';
  import { createSession, deleteSession, renameSession, gitAction, checkoutBranch, resumeSession, broadcast, getHistoryTailForServer } from '../lib/api';
  import { listServers, getActiveId, selectServer, removeServer, addServer, renameServer, serverColor, clearCredentials, parseServerPairing, withServer } from '../lib/auth';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import CreateSessionSheet from './CreateSessionSheet.svelte';
  import SessionContextMenu from './SessionContextMenu.svelte';
  import QrScanner from './QrScanner.svelte';
  import GitSheet from './GitSheet.svelte';
import ConfirmDialog from './ConfirmDialog.svelte';
  import AttentionFeed from './AttentionFeed.svelte';
  import AccountMenu from './AccountMenu.svelte';
  import SessionSwitcherSheet from './SessionSwitcherSheet.svelte';
  import HoverPreview from './HoverPreview.svelte';
  import type { SessionInfo, State, AggSession, ResumeCandidate } from '../lib/types';
  import { stateLabels, stateColors, countAwaiting, groupSelectedByServer, initials, projectKey, projectLabel, effectiveGroupBy, fmtWhen, sortSessions, latestAssistantEvent, clusterByPair, type GroupBy } from '../lib/format';
  import { updateBadge } from '../lib/badge';
  import Lottie from './Lottie.svelte';
  import pensando from '../lib/lottie/pensando.json';

  const stateChipBg: Record<State, string> = {
    working: 'var(--accent-dim)', idle: 'rgba(52,199,89,0.12)',
    awaiting_input: 'rgba(255,159,10,0.14)', dead: 'rgba(255,69,58,0.12)',
  };
  const STATIC_FRAME = 0; // f0 = anel cheio e simétrico (frames do meio ficam ralos)

  // cwd -> prefixo truncável + basename que nunca encolhe (mesma lógica do SessionCard).
  function cwdParts(cwd: string | undefined) {
    const p = (cwd ?? '').replace(/\/+$/, '');
    const i = p.lastIndexOf('/');
    return i < 0 ? { prefix: '', base: p } : { prefix: p.slice(0, i + 1), base: p.slice(i + 1) };
  }

  // Sidebar do DESKTOP (so monta >=820px). Reusa as MESMAS APIs/componentes do mobile, sem tocar
  // no fluxo mobile (SessionList continua intacto). Recolhe pra um trilho de ícones.
  interface Props {
    currentSession: string | null;
    onSelect: (name: string) => void;
    onCompare: (ids: { serverId: string; name: string }[]) => void;
    onLogout: () => void;
    boardActive: boolean;      // quadro aberto -> destaca o toggle e recolhe a sidebar pro rail
    onToggleBoard: () => void;
    canvasActive: boolean;     // canvas aberto -> mesmo tratamento do quadro (destaca + recolhe)
    onToggleCanvas: () => void;
  }
  let { currentSession, onSelect, onCompare, onLogout, boardActive, onToggleBoard, canvasActive, onToggleCanvas }: Props = $props();

  // Grupo generico: por SERVIDOR (hoje) ou por PROJETO (cwd) — mesmo shape nos dois modos. Cada
  // sessao carrega o serverId dela (no modo projeto um grupo pode juntar sessoes de servidores
  // diferentes; no modo servidor e sempre o mesmo serverId do grupo, mas mantem uniforme pro
  // template nao precisar saber em qual modo esta).
  interface SessRow extends SessionInfo { serverId: string }
  interface Group { id: string; label: string; color: string | null; error: string | null; sessions: SessRow[] }
  let collapsed = $state(false);   // pin: recolhido persistente (botao) — trilho de iniciais
  let hovering = $state(false);    // hover sobre a sidebar recolhida -> expande temporario
  let filterText = $state('');     // filtro por nome/cwd/servidor (aparece quando a lista fica longa)

  // Quadro OU canvas aberto -> recolhe pro rail (as colunas/canvas querem a largura); sair restaura o
  // pin como estava ANTES de entrar. Guarda o valor só na entrada: se o usuario re-expandir a mao
  // dentro da visão geral, nao recolhemos de novo — mas ao sair ainda voltamos ao estado pre-visão.
  const overviewActive = $derived(boardActive || canvasActive);
  let preBoardCollapsed: boolean | null = null;
  $effect(() => {
    if (overviewActive) {
      if (preBoardCollapsed === null) { preBoardCollapsed = collapsed; collapsed = true; }
    } else if (preBoardCollapsed !== null) {
      collapsed = preBoardCollapsed;
      preBoardCollapsed = null;
    }
  });

  // Grupos colapsados por id (servidor ou projeto), persistido — MESMA chave do mobile (SessionList)
  // pra o estado ser compartilhado onde faz sentido. Diferente do `collapsed` (pin do rail).
  const COLLAPSE_KEY = 'cp_collapsed_servers';
  function loadCollapsedGroups(): Set<string> {
    try { return new Set(JSON.parse(localStorage.getItem(COLLAPSE_KEY) ?? '[]')); } catch { return new Set(); }
  }
  let collapsedGroups = $state<Set<string>>(loadCollapsedGroups());
  function toggleGroup(id: string) {
    const next = new Set(collapsedGroups);
    if (next.has(id)) next.delete(id); else next.add(id);
    collapsedGroups = next;
    // Persiste só o colapso de servidor/projeto; o de CLUSTER de pareamento (chave 'pair:<gid>') é
    // efêmero — o gid é regenerado a cada pareamento, então salvar acumularia lixo morto pra sempre.
    try { localStorage.setItem(COLLAPSE_KEY, JSON.stringify([...next].filter((k) => !k.startsWith('pair:')))); } catch { /* quota/priv mode */ }
  }

  // Toggle "Servidor | Projeto" (feature #3), persistido — mesmo padrao de cp_sidebar_w. Guarda a
  // PREFERENCIA crua; o modo efetivo (effectiveGroupBy) força "projeto" quando ha <2 servidores.
  const GROUP_BY_KEY = 'cp_group_by';
  let groupBy = $state<GroupBy>(localStorage.getItem(GROUP_BY_KEY) === 'project' ? 'project' : 'server');
  function setGroupBy(mode: GroupBy) {
    groupBy = mode;
    try { localStorage.setItem(GROUP_BY_KEY, mode); } catch { /* storage cheio/off */ }
    // `groups` é derived — reage sozinho à mudança de groupBy, sem recompute manual.
  }

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
  let activeId = $state(getActiveId());
  let scanning = $state(false);
  let showCreate = $state(false);
  let accountOpen = $state(false);    // menu de conta (avatar do rodapé)
  let acctAnchorEl = $state<HTMLElement>();  // âncora do popover do menu de conta
  let searchOpen = $state(false);     // "Buscar conversas" (switcher em modo só-busca)
  let showAddServer = $state(false);  // modal de adicionar servidor (colar URL / QR)

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
  onMount(() => {
    sessionsStore.retain();
    return () => {
      sessionsStore.release();
      clearTimeout(hpTimer);   // espiada pendente nao sobrevive ao unmount (fetch orfao + setState)
    };
  });

  // Agrupamento é POLÍTICA do Sidebar (servidor|projeto); o dado agregado/deduplicado vem do store.
  const groups = $derived.by<Group[]>(() => {
    if (servers.length === 0) return [];
    // Modo efetivo: com <2 servidores, "por servidor" viraria 1 grupo gigante -> força "por projeto".
    const mode = effectiveGroupBy(groupBy, servers.length);
    if (mode === 'project') {
      // Modo projeto: junta sessoes de TODOS os servidores pela chave do cwd. Servidor offline so
      // perde as sessoes dele (sem banner por grupo — nao ha "1 servidor" pra apontar o erro).
      const byKey = new Map<string, SessRow[]>();
      for (const s of sessionsStore.rows) {
        const pk = projectKey(s.cwd);
        const arr = byKey.get(pk);
        if (arr) arr.push(s); else byKey.set(pk, [s]);
      }
      return [...byKey.entries()]
        .map(([key, rowsOf]) => ({ id: key, label: projectLabel(rowsOf[0]?.cwd), color: null, error: null, sessions: sortSessions(rowsOf) }))
        .sort((a, b) => a.label.localeCompare(b.label));
    }
    // Modo servidor (comportamento de sempre): 1 grupo por servidor, sempre presente (mesmo vazio
    // ou offline), na ORDEM de `servers`. Semântica de erro preservada: "offline" só aparece quando
    // NÃO há lista (com lista stale o grupo mostra as sessões e não o banner).
    return sessionsStore.byServer.map((b) => ({
      id: b.server.id,
      label: b.server.label,
      color: serverColor(b.server.id),
      error: b.loaded ? null : b.error,
      sessions: sortSessions(b.sessions),
    }));
  });

  // Web push + horas silenciosas migraram pro AccountMenu (menu de conta do rodapé), que reusa os
  // mesmos enablePush/getPushSettings/setQuietHours — não reimplementa nada aqui.

  // ── Retomar sessao "sem id" (paridade com o SessionCard do mobile): relança o pane com
  // `claude --resume <uuid>` -> passa a rastrear. Caso seguro resolve direto; caso ambiguo o backend
  // devolve candidatos e abrimos o modal pra confirmar QUAL conversa retomar. Reusa o MESMO
  // resumeSession. withServer mira o dono e restaura (igual ao resto das ops). ──────────────────────
  let resumeModal = $state<{ name: string; serverId: string; candidates: ResumeCandidate[] } | null>(null);
  let resumeBusy = $state('');   // nome da sessao em processamento (desabilita o botao/itens)
  let resumeError = $state('');
  async function handleResume(name: string, serverId: string, sessionId?: string, e?: MouseEvent) {
    e?.stopPropagation();
    resumeError = '';
    resumeBusy = name;
    try {
      const r = await withServer(serverId, () => resumeSession(name, sessionId));
      if (r && 'ambiguous' in r && r.ambiguous) {
        resumeModal = { name, serverId, candidates: r.candidates };   // ambiguo: escolher no modal
      } else {
        resumeModal = null;   // religada (caso seguro ou escolha confirmada); o SSE atualiza a linha
      }
    } catch (err) {
      resumeError = err instanceof Error ? err.message : 'falha ao retomar';
      if (!resumeModal) flash(`retomar: ${resumeError}`);   // erro do botao da linha -> toast
    } finally {
      resumeBusy = '';
    }
  }

  async function handleCreate(name: string, cwd?: string, configDir?: string | null, provider?: 'claude' | 'codex') {
    // O CreateSessionSheet já posicionou o servidor-alvo como ativo (selectServer).
    await createSession(name, cwd, configDir, provider);
    activeId = getActiveId(); // I2: sync local state after sheet's selectServer
    onSelect(name);
    // SSE stream emitirá a sessão nova automaticamente
  }
  // Excluir sessao pede confirmacao (com o nome) — clique unico no × era facil de errar. O delete real
  // so acontece no doDelete.
  let confirmDel = $state<{ name: string; serverId: string } | null>(null);
  function handleDelete(name: string, serverId: string, e: MouseEvent) {
    e.stopPropagation();
    confirmDel = { name, serverId };
  }
  async function doDelete() {
    if (!confirmDel) return;
    const { name, serverId } = confirmDel;
    confirmDel = null;
    const prev = getActiveId(); // C1: save before pointing at target server
    selectServer(serverId); // api.ts mira o server ativo -> aponta pro dono da sessão
    try { await deleteSession(name); } catch (e) { flash(`excluir: ${errMsg(e)}`); }
    if (prev && prev !== serverId) selectServer(prev); // C1: restore so open chat stays on its server
    // SSE stream emitirá a lista atualizada automaticamente
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
  function onMainClick(name: string, serverId: string, tracked: boolean | undefined) {
    if (longPressed) { longPressed = false; return; } // foi toque longo (renomear)
    if (tracked === false) return; // sem id confiável -> não abre
    selectServer(serverId); // o Chat usa o server ativo
    activeId = serverId; // I2: keep local badge in sync immediately
    onSelect(name);
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

  async function saveEdit(old: string, serverId: string) {
    const nv = editValue.trim();
    editing = null;
    if (!nv || nv === old) return;
    const prev = getActiveId(); // C1: save before pointing at target server
    selectServer(serverId);
    try {
      const r = await renameSession(old, nv);
      if (old === currentSession) onSelect(r.name);
    } catch { /* load corrige */ }
    finally {
      if (prev && prev !== serverId) selectServer(prev); // C1: restore so open chat stays on its server
      // SSE stream emitirá a sessão renomeada automaticamente
    }
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
  let menuMsg = $state('');   // banner efemero pro resultado do git pull / erro do editor
  let flashTimer: ReturnType<typeof setTimeout> | undefined;

  // Largura efetiva: pin aberto, OU hover, OU overlay preso a uma linha (menu/rename) ativo -> nao
  // recolhe por baixo dele. Rail de iniciais so quando nada disso vale. (Apos menu/editing existirem.)
  const expanded = $derived(!collapsed || hovering || menu !== null || editing !== null);

  function openMenu(e: MouseEvent, s: SessionInfo, serverId: string) {
    e.preventDefault();
    clearTimeout(pressTimer);   // cancela o long-press (senao dispararia rename junto)
    hpLeave();   // botao direito nao move o mouse: fecha a espiada pra nao ficar atras do menu
    menu = { x: e.clientX, y: e.clientY, name: s.name, serverId, cwd: s.cwd ?? '', thenTarget: s.then_target ?? null };
    // O SessionContextMenu carrega o estado de silenciar/branches/encadeamento na propria montagem.
  }
  function closeMenu() { menu = null; }

  // Confirmacao de troca com working tree suja (switch carrega mudancas nao-conflitantes pra outra branch).
  let confirmBranch = $state<{ name: string; serverId: string; branch: string } | null>(null);
  // Gerenciador git (GitSheet) aberto pelo menu de contexto, no repo da sessao, SEM abrir o chat.
  // A GitSheet mira o server ATIVO -> aponto pro dono da sessao enquanto aberta e restauro no fechar.
  let gitSheet = $state<{ name: string } | null>(null);
  let gitSheetPrevServer: string | null = null;
  function menuGit() {
    if (!menu) return;
    const { name, serverId } = menu;
    gitSheetPrevServer = getActiveId();
    selectServer(serverId);
    gitSheet = { name };
    closeMenu();
  }
  function closeGitSheet() {
    gitSheet = null;
    if (gitSheetPrevServer) { selectServer(gitSheetPrevServer); gitSheetPrevServer = null; }
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
    flash(`guardando mudanças…`);
    try {
      await withServer(serverId, async () => {
        const r = await gitAction(name, 'stash');
        if (!r.ok) throw new Error(r.output || 'stash falhou');
        await checkoutBranch(name, branch);
      });
      flash(`branch: ${branch} — mudanças guardadas (use "pop" na aba Git pra recuperar)`);
    } catch (e) {
      flash(`checkout: ${errMsg(e)}`);
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
    return groups.flatMap((g) => g.sessions).filter((s) => s.serverId === serverId && s.name !== exclude);
  }

  const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

  function menuRename() {
    if (!menu) return;
    editing = `${menu.serverId}::${menu.name}`;
    editValue = menu.name;
    closeMenu();
  }
  function menuDelete() {
    if (!menu) return;
    confirmDel = { name: menu.name, serverId: menu.serverId };
    closeMenu();
  }

  // Renomear servidor: o AccountMenu cuida da UI inline; aqui só persistimos e reagregamos pra os
  // headers de grupo pegarem o nome novo (sem esperar o próximo SSE).
  function onRenameServer(id: string, label: string) {
    renameServer(id, label);
    sessionsStore.refreshServers();   // reagrega pra os headers de grupo pegarem o nome novo já
  }

  // Abre o menu de conta recarregando a lista de servidores (pode ter mudado desde a última abertura).
  function openAccount() {
    sessionsStore.refreshServers();
    accountOpen = !accountOpen;
  }

  // Trocar de servidor ativo (menu de conta): reload pra o app inteiro apontar pro novo backend.
  function pickServer(id: string) {
    if (id === getActiveId()) { accountOpen = false; return; }
    selectServer(id);
    window.location.reload();
  }
  // Remover servidor pede confirmacao (com o nome) — o × de um toque removia na hora e, se fosse o
  // unico servidor, deslogava junto. O remove real so acontece no doDropServer.
  let confirmSrv = $state<{ id: string; label: string } | null>(null);
  function dropServer(id: string) {
    const s = servers.find((x) => x.id === id);
    confirmSrv = { id, label: s?.label ?? id };
  }
  function doDropServer() {
    if (!confirmSrv) return;
    const id = confirmSrv.id;
    confirmSrv = null;
    const was = id === getActiveId();
    removeServer(id);   // auth notifica o store (onServersChanged) -> ele reconcilia os streams sozinho
    activeId = getActiveId();
    if (listServers().length === 0) { clearCredentials(); onLogout(); return; }
    if (was) window.location.reload();
  }
  function handleScan(text: string) {
    scanning = false;
    const parsed = parseServerPairing(text);
    if (!parsed) return;
    addServer(parsed.base, parsed.token);
    window.location.reload();
  }
  // Colar servidor manual (desktop nao tem camera): cola a URL de pareamento (com token) e adiciona
  // pela MESMA rota de parse do QR. Aberto pelo item "Adicionar servidor" do menu de conta.
  let addUrlText = $state('');
  let addError = $state('');
  function openAddServer() {
    addUrlText = '';
    addError = '';
    showAddServer = true;
  }
  function submitPasteServer() {
    const parsed = parseServerPairing(addUrlText.trim());
    if (!parsed) { addError = 'Cole a URL de pareamento (com o token).'; return; }
    addServer(parsed.base, parsed.token);
    window.location.reload();
  }
  function logout() {
    clearCredentials();
    onLogout();
  }
  // Sair pede confirmacao (recuperacao exige o token/QR de novo).
  let confirmLogout = $state(false);

  const activeServer = $derived(servers.find((s) => s.id === activeId) ?? servers[0] ?? null);

  // Conta (avatar do rodapé): nome = servidor ativo; subtítulo = contagem de servidores. Iniciais
  // reusam o helper compartilhado (format).
  const accountName = $derived(activeServer?.label ?? 'conta');
  const accountSub = $derived(`${servers.length} servidor${servers.length === 1 ? '' : 'es'}`);
  const accountInitials = $derived(initials(accountName));

  // Badge do ícone do app (feature #13): mesmo agregado do mobile (SessionList), so que a partir de
  // `groups` (por servidor) — flatten pra contar aguardando em TODOS os servidores.
  const awaitingTotal = $derived(groups.reduce((n, g) => n + countAwaiting(g.sessions), 0));
  $effect(() => { updateBadge(awaitingTotal); });

  // Fila cross-server pra a AttentionFeed (feature #6): as rows do store já vêm deduplicadas e
  // enriquecidas com label/cor do servidor (AggSession). Independe do modo de agrupamento; o
  // attentionFeed reordena por urgência internamente.
  const attnSessions = $derived<AggSession[]>(sessionsStore.rows);

  // Filtro (paridade com o mobile): so aparece quando ha muitas sessoes; casa nome/cwd/rotulo do
  // grupo. Aplicado sobre `groups` num derived (reativo ao texto) — nao mexe no fluxo do SSE. Grupo
  // que zera apos o filtro some; sem filtro, `groups` passa intacto (mantem grupo offline/vazio).
  const totalSessions = $derived(groups.reduce((n, g) => n + g.sessions.length, 0));
  const showFilter = $derived(totalSessions > 6);
  const filteredGroups = $derived.by(() => {
    const q = filterText.trim().toLowerCase();
    if (!q) return groups;
    return groups
      .map((g) => ({
        ...g,
        sessions: g.sessions.filter(
          (s) =>
            s.name.toLowerCase().includes(q) ||
            (s.cwd ?? '').toLowerCase().includes(q) ||
            g.label.toLowerCase().includes(q),
        ),
      }))
      .filter((g) => g.sessions.length > 0);
  });
  const filterEmpty = $derived(filterText.trim() !== '' && filteredGroups.length === 0);

  // Gerenciador git (GitSheet) aberto pelo botao git DA LINHA (repo da sessao, sem abrir o chat) —
  // mesma mecânica do menuGit (mira o server dono, restaura no fechar via closeGitSheet).
  function rowGit(name: string, serverId: string, e: MouseEvent) {
    e.stopPropagation();
    gitSheetPrevServer = getActiveId();
    selectServer(serverId);
    gitSheet = { name };
  }

  // ── Broadcast (feature #9): selecionar N sessoes e mandar 1 prompt pra todas ──────────────────
  // Mesmo padrao do mobile (SessionList): selecao = "<serverId>:<name>", groupSelectedByServer
  // particiona por servidor-dono -> 1 chamada a broadcast() por servidor (withServer restaura o ativo).
  let selectMode = $state(false);
  let selected = $state<Set<string>>(new Set());
  let broadcastText = $state('');
  let broadcastBusy = $state(false);
  let broadcastMsg = $state('');

  function toggleSelectMode() {
    selectMode = !selectMode;
    selected = new Set();
    broadcastText = '';
    broadcastMsg = '';
  }
  function toggleSelected(key: string) {
    const next = new Set(selected);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    selected = next;
  }
  // "enviar p/ todas" no header do grupo: entra em selecao com o grupo inteiro marcado (exceto "sem id").
  function selectGroupForBroadcast(g: Group) {
    selectMode = true;
    selected = new Set(
      g.sessions.filter((s) => s.tracked !== false).map((s) => `${s.serverId}:${s.name}`),
    );
  }
  // Slash-command manda por sessao (correcao de rota) -> desabilita o broadcast em vez de replicar
  // "/comando" pra N sessoes de uma vez (ex: /clear em todas sem querer).
  const broadcastIsSlash = $derived(broadcastText.trim().startsWith('/'));
  const broadcastDisabled = $derived(broadcastBusy || selected.size === 0 || !broadcastText.trim() || broadcastIsSlash);

  // "Comparar" (feature #11): reusa a MESMA seleção multipla do broadcast pra abrir a grade lado a
  // lado. Precisa de 2+ (comparar 1 sessão não tem propósito).
  const compareDisabled = $derived(selected.size < 2);
  function openCompare() {
    const allSessions = groups.flatMap((g) => g.sessions);
    const ids = allSessions
      .filter((s) => selected.has(`${s.serverId}:${s.name}`))
      .map((s) => ({ serverId: s.serverId, name: s.name }));
    onCompare(ids);
  }

  async function sendBroadcast() {
    const text = broadcastText.trim();
    if (broadcastDisabled) return;
    broadcastBusy = true;
    broadcastMsg = '';
    const allSessions = groups.flatMap((g) => g.sessions);
    const byServer = groupSelectedByServer(allSessions, selected);
    const prev = getActiveId();
    const failed: string[] = [];
    for (const [serverId, names] of byServer) {
      selectServer(serverId);
      try {
        const results = await broadcast(names, text);
        for (const [n, r] of Object.entries(results)) if (!r.ok) failed.push(n);
      } catch {
        failed.push(...names); // servidor offline/erro de rede -> conta todo o lote dele como falho
      }
    }
    if (prev) selectServer(prev);
    broadcastBusy = false;
    if (failed.length) {
      broadcastMsg = `falha: ${failed.join(', ')}`;
    } else {
      broadcastText = '';
      selected = new Set();
      selectMode = false;
    }
  }
</script>

<aside class="sidebar" class:collapsed={!expanded} class:resizing
  style:width={expanded ? width + 'px' : undefined}
  onmouseenter={() => (hovering = true)} onmouseleave={() => (hovering = false)}>
  <div class="side-top">
    <button class="icon-btn" onclick={() => (collapsed = !collapsed)} aria-label={collapsed ? 'Expandir' : 'Recolher'}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
        <rect x="3" y="4" width="18" height="16" rx="2"/>
        <line x1="9" y1="4" x2="9" y2="20"/>
      </svg>
    </button>
    {#if expanded}<span class="side-brand">claude pocket</span>{/if}
    {#if expanded}
      <!-- Broadcast (feature #9): entra/sai do modo seleção multipla. -->
      <button
        class="select-toggle-btn"
        class:active={selectMode}
        onclick={toggleSelectMode}
        aria-label={selectMode ? 'Cancelar seleção' : 'Selecionar sessões'}
        title={selectMode ? 'Cancelar seleção' : 'Selecionar / broadcast'}
      >
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M9 11l3 3L22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/>
        </svg>
      </button>
    {/if}
    <!-- Toggle do quadro (visualização irmã da lista+chat). Fora do {#if expanded} de proposito: no
         rail recolhido — que e justamente onde a sidebar fica com o quadro aberto — precisa aparecer
         pra dar a volta. -->
    <button class="board-btn" class:active={boardActive} onclick={onToggleBoard}
            title={boardActive ? 'Voltar pra lista' : 'Quadro de sessões'}
            aria-label="Alternar quadro de sessões" aria-pressed={boardActive}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" aria-hidden="true">
        <rect x="3" y="3" width="7.5" height="7.5" rx="1.5"/><rect x="13.5" y="3" width="7.5" height="7.5" rx="1.5"/>
        <rect x="3" y="13.5" width="7.5" height="7.5" rx="1.5"/><rect x="13.5" y="13.5" width="7.5" height="7.5" rx="1.5"/>
      </svg>
    </button>
    <!-- Toggle do canvas livre (visualização irmã do quadro). Fora do {#if expanded} pelo mesmo motivo
         do board: no rail recolhido — onde a sidebar fica com o canvas aberto — precisa aparecer. -->
    <button class="board-btn" class:active={canvasActive} onclick={onToggleCanvas}
            title={canvasActive ? 'Voltar pra lista' : 'Canvas de sessões'}
            aria-label="Alternar canvas de sessões" aria-pressed={canvasActive}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" aria-hidden="true">
        <rect x="3" y="4" width="9" height="7" rx="1.5"/><rect x="14.5" y="8" width="6.5" height="10" rx="1.5"/>
        <rect x="5" y="14.5" width="7" height="5.5" rx="1.5"/>
      </svg>
    </button>
    <!-- Kebab "⋯": nav secundária (Buscar/Arquivo/Custos) + agrupamento, docado no header — o twin
         desktop do hamburger do mobile. Abre um popover ancorado (renderizado fora do <aside>). -->
    <button class="kebab-btn" class:active={kebabOpen} onclick={openKebab} aria-haspopup="menu" aria-expanded={kebabOpen} aria-label="Mais opções" title="Buscar, Arquivo, Custos, Agrupar">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><circle cx="5" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="19" cy="12" r="1.6"/></svg>
    </button>
  </div>

  <nav class="sess-list" class:compact aria-label="Sessões">
    {#if expanded}
      <!-- "Precisa de você" (feature #6): fila cross-server de sessoes aguardando, fixa no topo.
           Picker inline (OptionButtons) responde sem abrir o chat; nativo AskUserQuestion abre. -->
      <AttentionFeed sessions={attnSessions} onOpenChat={(s) => onMainClick(s.name, s.serverId, s.tracked)} />
      <!-- Toggle Servidor|Projeto (feature #3) migrou pro popover do kebab (header) — não empilha
           mais no topo da lista. Continua condicional a >=2 servidores (dentro do popover). -->

      <!-- Filtro (paridade com o mobile): so aparece quando a lista fica longa. -->
      {#if showFilter}
        <input
          type="text"
          class="filter-input"
          bind:value={filterText}
          placeholder="Filtrar sessões"
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck={false}
          aria-label="Filtrar sessões"
        />
      {/if}
      {#if filterEmpty}
        <p class="filter-empty">Nenhuma sessão corresponde ao filtro.</p>
      {/if}
    {/if}
    {#each filteredGroups as g (g.id)}
      {@const awaiting = countAwaiting(g.sessions)}
      {#if expanded}
        <div class="grp-head-row">
          <!-- Header colapsavel (paridade com o mobile): chevron + label + contagem + aguardando. -->
          <button
            class="grp-head"
            onclick={() => toggleGroup(g.id)}
            aria-expanded={!collapsedGroups.has(g.id)}
            title={g.error ? `${g.label}: ${g.error}` : g.label}
          >
            <span class="grp-chevron" class:collapsed={collapsedGroups.has(g.id)} aria-hidden="true">▾</span>
            {#if g.color}<span class="grp-dot" style="background: {g.color};" aria-hidden="true"></span>{/if}
            <span class="grp-label">{g.label}</span>
            {#if g.sessions.length > 0}<span class="grp-count">{g.sessions.length}</span>{/if}
            {#if awaiting > 0}<span class="grp-await" title={`${awaiting} aguardando`}>{awaiting}</span>{/if}
            {#if g.error}<span class="grp-off">offline</span>{/if}
          </button>
          <!-- "enviar p/ todas" (feature #9): entra em modo seleção com o grupo inteiro marcado. -->
          <button
            class="grp-broadcast"
            onclick={() => selectGroupForBroadcast(g)}
            aria-label={`Enviar mensagem para todas as sessões de ${g.label}`}
            title="Enviar p/ todas"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <line x1="22" y1="2" x2="11" y2="13"/>
              <polygon points="22 2 15 22 11 13 2 9 22 2"/>
            </svg>
          </button>
        </div>
      {/if}
      {#if !expanded || !collapsedGroups.has(g.id)}
      {#each clusterByPair(g.sessions) as item (item.kind === 'header' ? `ph:${item.gid}` : `${item.session.serverId}::${item.session.name}`)}
        {#if item.kind === 'header'}
          {#if expanded}
            <!-- Cluster de pareamento (Opção C): sub-header colapsável do grupo, dentro do servidor. -->
            <button class="pair-head" onclick={() => toggleGroup(`pair:${item.gid}`)}
                    aria-expanded={!collapsedGroups.has(`pair:${item.gid}`)} title={`Grupo pareado: ${item.label}`}>
              <span class="grp-chevron" class:collapsed={collapsedGroups.has(`pair:${item.gid}`)} aria-hidden="true">▾</span>
              <span class="pair-head-label">🤝&nbsp;{item.label}</span>
              <span class="grp-count">{item.count}</span>
            </button>
          {/if}
        {:else if !item.gid || !collapsedGroups.has(`pair:${item.gid}`)}
        {@const s = item.session}
        {@const rowKey = `${s.serverId}::${s.name}`}
        {@const selKey = `${s.serverId}:${s.name}`}
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
              aria-label="Renomear sessão"
            />
          {:else}
            <button
              class="sess-main"
              class:untracked={s.tracked === false}
              aria-pressed={selectMode ? selected.has(selKey) : undefined}
              title={!expanded ? s.name : (s.tracked === false ? 'claude aberto sem --session-id: transcript nao rastreavel' : 'Toque longo pra renomear')}
              onpointerdown={() => { if (!selectMode) pressStart(rowKey); }}
              onpointerup={pressEnd}
              onpointerleave={pressEnd}
              onpointercancel={pressEnd}
              oncontextmenu={(e) => { if (!selectMode) openMenu(e, s, s.serverId); }}
              onclick={() => {
                hpLeave();   // clique nao move o mouse -> sem mouseleave; fecha a espiada na mao
                if (selectMode) { if (s.tracked !== false) toggleSelected(selKey); return; }
                onMainClick(s.name, s.serverId, s.tracked);
              }}
            >
              <span class="lead" aria-hidden="true">
                {#if selectMode}
                  <input type="checkbox" class="select-check" checked={selected.has(selKey)} tabindex="-1" aria-hidden="true" />
                {:else if !expanded && s.state !== 'working'}
                  <!-- Rail recolhido: iniciais tingidas pelo estado (identifica sem o nome). -->
                  <span class="initials" style="color: {stateColors[s.state]}; border-color: {stateColors[s.state]}; background: {stateChipBg[s.state]};">{initials(s.name)}</span>
                {:else if !expanded && s.stalled}
                  <!-- Working travada no rail recolhido: iniciais com anel âmbar (sem o spinner, que sumiria o aviso). -->
                  <span class="initials stalled" title="Pode estar travada" style="color: {stateColors[s.state]}; border-color: {stateColors[s.state]}; background: {stateChipBg[s.state]};">{initials(s.name)}</span>
                {:else if s.state === 'working'}
                  <Lottie data={pensando as any} size={18} loop autoplay />
                {:else}
                  <Lottie data={pensando as any} size={18} loop={false} autoplay={false} frame={STATIC_FRAME} />
                {/if}
              </span>
              {#if expanded}
                <span class="row-info">
                  <span class="name-row">
                    <span class="sess-name">{s.name}</span>
                    {#if s.tracked === false}<span class="sess-badge" title="sem --session-id: nao rastreavel">sem id</span>{/if}
                  </span>
                  {#if s.state === 'awaiting_input' && s.question}
                    <span class="status-sub asking" title={s.question}>{s.question}</span>
                  {:else if s.state === 'working' && s.label}
                    <span class="status-sub working" title={s.label}>{s.label}</span>
                  {/if}
                  {#if s.cwd}
                    {@const cp = cwdParts(s.cwd)}
                    <span class="cwd" title={s.cwd}><span class="cwd-prefix">{cp.prefix}</span><span class="cwd-base">{cp.base}</span></span>
                  {/if}
                  {#if s.branch}
                    <!-- Branch git atual (paridade com o mobile), linha propria pra nao competir com o cwd. -->
                    <span class="branch" title="branch git atual">⎇ {s.branch}</span>
                  {/if}
                </span>
                {#if s.limited}
                  <span
                    class="limited-chip"
                    title={s.limit_reset ? `Limite de uso atingido — volta ${s.limit_reset}` : 'Limite de uso atingido'}
                  >⏳{#if s.limit_reset}&nbsp;{s.limit_reset}{/if}</span>
                {/if}
                {#if s.then_target}
                  <!-- Feature #12: indicador do vinculo 'then' armado ("quando terminar -> enviar pra"). -->
                  <span class="chain-chip" title={`Quando terminar, envia pra "${s.then_target}"`}>🔗&nbsp;{s.then_target}</span>
                {/if}
                {#if s.pair_peers?.length}
                  <!-- Grupo de trabalho: mesmo formato do chain-chip (1 par = nome; N = contagem). -->
                  <span class="chain-chip" title={`Grupo com ${s.pair_peers.join(', ')} — trabalham juntas via cp-send`}>🤝&nbsp;{s.pair_peers.length === 1 ? s.pair_peers[0] : s.pair_peers.length + 1}</span>
                {/if}
                <span
                  class="state-chip"
                  class:stalled={s.stalled === true}
                  style="color: {stateColors[s.state]}; background: {stateChipBg[s.state]};"
                  title={s.stalled ? 'Pode estar travada — sem atividade há um tempo' : undefined}
                >{stateLabels[s.state]}</span>
              {/if}
            </button>
            {#if expanded && !selectMode}
              <!-- Retomar (paridade com o SessionCard do mobile): unica acao possivel numa linha "sem
                   id" -> visivel sempre (nao escondida no hover), tingida de accent. Reusa resumeSession. -->
              {#if s.tracked === false}
                <button
                  class="sess-resume"
                  onclick={(e) => handleResume(s.name, s.serverId, undefined, e)}
                  disabled={resumeBusy === s.name}
                  aria-label={`Retomar conversa de ${s.name}`}
                  title="Retomar conversa"
                >
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <polyline points="1 4 1 10 7 10"/>
                    <path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/>
                  </svg>
                </button>
              {/if}
              <!-- Git da linha (paridade com o mobile): abre a GitSheet no repo do cwd, sem abrir o
                   chat. So quando ha cwd. stopPropagation pra nao disparar rename/navegacao da linha. -->
              {#if s.cwd}
                <button class="sess-git" onclick={(e) => rowGit(s.name, s.serverId, e)} aria-label={`Git de ${s.name}`} title="Gerenciador git">
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <line x1="6" y1="3" x2="6" y2="15"/>
                    <circle cx="18" cy="6" r="3"/>
                    <circle cx="6" cy="18" r="3"/>
                    <path d="M18 9a9 9 0 0 1-9 9"/>
                  </svg>
                </button>
              {/if}
              <button class="sess-del" onclick={(e) => handleDelete(s.name, s.serverId, e)} aria-label={`Excluir ${s.name}`}>×</button>
            {/if}
          {/if}
        </div>
        {/if}
      {/each}
      {/if}
    {/each}
  </nav>

  {#if expanded && selectMode}
    <!-- Composer compacto do broadcast (feature #9): so texto + enviar, sem anexos/slash-UI (isso
         fica no Composer normal, por sessão). Slash-command desabilita o envio (rota por sessão). -->
    <div class="broadcast-bar">
      <div class="broadcast-row">
        <span class="broadcast-count">{selected.size} selecionada{selected.size === 1 ? '' : 's'}</span>
        <button class="broadcast-compare" onclick={openCompare} disabled={compareDisabled} aria-label="Comparar sessões selecionadas" title="Comparar">Comparar</button>
        <button class="broadcast-cancel" onclick={toggleSelectMode} aria-label="Cancelar seleção">×</button>
      </div>
      {#if broadcastMsg}<p class="broadcast-msg">{broadcastMsg}</p>{/if}
      <div class="broadcast-input-row">
        <input
          type="text"
          class="broadcast-input"
          bind:value={broadcastText}
          placeholder="Mensagem para as sessões selecionadas"
          disabled={broadcastBusy}
          onkeydown={(e) => { if (e.key === 'Enter' && !broadcastDisabled) sendBroadcast(); }}
          aria-label="Mensagem de broadcast"
        />
        <button class="broadcast-send" onclick={sendBroadcast} disabled={broadcastDisabled} aria-label="Enviar">
          {#if broadcastBusy}
            …
          {:else}
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <line x1="22" y1="2" x2="11" y2="13"/>
              <polygon points="22 2 15 22 11 13 2 9 22 2"/>
            </svg>
          {/if}
        </button>
      </div>
      {#if broadcastIsSlash}
        <p class="broadcast-hint">Slash-commands não têm broadcast — envie dentro da sessão.</p>
      {/if}
    </div>
  {/if}

  <!-- Rodapé (estilo Claude): botão da conta (avatar -> menu de conta) + CTA "Nova sessão". Tudo que
       era config/conta (servidores, notificações, horas silenciosas, reconectar, sair) vive no menu. -->
  <div class="side-foot" class:rail={!expanded}>
    <div class="account-anchor" bind:this={acctAnchorEl}>
      <button class="acct-btn" onclick={openAccount} aria-haspopup="menu" aria-expanded={accountOpen} aria-label="Menu da conta" title={expanded ? undefined : accountName}>
        <span class="acct-avatar" aria-hidden="true">{accountInitials}</span>
        {#if expanded}
          <span class="acct-who">
            <span class="acct-name">{accountName}</span>
            <span class="acct-sub">{accountSub}</span>
          </span>
        {/if}
      </button>
      <AccountMenu
        open={accountOpen}
        onClose={() => (accountOpen = false)}
        initials={accountInitials}
        {accountName}
        {accountSub}
        {servers}
        anchorEl={acctAnchorEl}
        {activeId}
        onSwitchServer={pickServer}
        {onRenameServer}
        onRemoveServer={dropServer}
        onAddServer={openAddServer}
        onReconnect={sessionsStore.reconnect}
        onLogout={() => (confirmLogout = true)}
      />
    </div>
    <button class="cta-new" onclick={() => (showCreate = true)} aria-label="Nova sessão" title="Nova sessão">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
      {#if expanded}<span>Nova</span>{/if}
    </button>
  </div>

  {#if !collapsed}
    <!-- Drag na borda direita pra redimensionar (so pin aberto). -->
    <div class="resize-handle" onpointerdown={resizeStart} onpointermove={resizeMove}
      onpointerup={resizeEnd} onpointercancel={resizeEnd}
      role="separator" aria-label="Redimensionar barra lateral" aria-orientation="vertical"></div>
  {/if}
</aside>

<!-- Espiada no hover. FORA do <aside> pelo mesmo motivo do menu de contexto: a sidebar tem
     overflow:hidden (e backdrop-filter no modo liquid, que vira containing block) e clipa o popover. -->
{#if hp}<HoverPreview text={hp.text} x={hp.x} y={hp.y} />{/if}

<CreateSessionSheet open={showCreate} {servers} onClose={() => (showCreate = false)} onCreate={handleCreate} onOpenSession={onSelect} />
{#if scanning}<QrScanner onScan={handleScan} onClose={() => (scanning = false)} />{/if}

<!-- "Buscar conversas" (nav): switcher em modo só-busca (busca de conteúdo cross-servidor, feature #10). -->
<SessionSwitcherSheet
  open={searchOpen}
  searchOnly
  sessions={[]}
  currentName=""
  onPick={(name) => { searchOpen = false; onSelect(name); }}
  onNew={() => { searchOpen = false; showCreate = true; }}
  onClose={() => (searchOpen = false)}
/>

<!-- Popover do kebab "⋯" (header): nav secundária + agrupamento. Renderizado FORA do <aside> pra
     escapar o backdrop-filter da sidebar (mesmo motivo do menu de contexto). Ancorado via kebabPos. -->
{#if kebabOpen}
  <div class="menu-backdrop" onclick={closeKebab} role="presentation"></div>
  <div class="kebab-menu" style="left: {kebabPos.left}px; top: {kebabPos.top}px;" role="menu">
    <button type="button" role="menuitem" class="kebab-item" onclick={() => { closeKebab(); accountOpen = false; searchOpen = true; }}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>
      Buscar conversas
    </button>
    <button type="button" role="menuitem" class="kebab-item" onclick={() => { closeKebab(); window.location.hash = '#/archive'; }}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="4" rx="1"/><path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8"/><path d="M10 12h4"/></svg>
      Arquivo
    </button>
    <button type="button" role="menuitem" class="kebab-item" onclick={() => { closeKebab(); window.location.hash = '#/costs'; }}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 3v18h18"/><path d="M7 14l4-4 3 3 5-6"/></svg>
      Custos
    </button>
    <button type="button" role="menuitem" class="kebab-item" onclick={() => { toggleCompact(); closeKebab(); }}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M3 6h18"/><path d="M3 12h18"/><path d="M3 18h18"/></svg>
      {compact ? 'Densidade normal' : 'Densidade compacta'}
    </button>
    {#if servers.length >= 2}
      <div class="ctx-sep"></div>
      <div class="kebab-group-label">Agrupar por</div>
      <div class="group-toggle" role="radiogroup" aria-label="Agrupar por">
        <button type="button" class:active={groupBy === 'server'} role="radio" aria-checked={groupBy === 'server'} onclick={() => setGroupBy('server')}>Servidor</button>
        <button type="button" class:active={groupBy === 'project'} role="radio" aria-checked={groupBy === 'project'} onclick={() => setGroupBy('project')}>Projeto</button>
      </div>
    {/if}
  </div>
{/if}

<!-- Adicionar servidor (do menu de conta): colar a URL de pareamento (com token) ou escanear QR.
     Mesma rota de parse do QR (parseServerPairing). Estilo dos modais do desktop (confirm-card). -->
{#if showAddServer}
  <ConfirmDialog title="Adicionar servidor" aria="Adicionar servidor" role="dialog"
    onClose={() => (showAddServer = false)}
    actions={[
      { label: 'Escanear QR', onClick: () => { showAddServer = false; scanning = true; } },
      { label: 'Adicionar', kind: 'primary', disabled: !addUrlText.trim(), onClick: submitPasteServer },
    ]}>
    <input
      type="url"
      class="add-srv-input"
      bind:value={addUrlText}
      placeholder="Colar URL do servidor (com token)"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      use:autofocus
      onkeydown={(e) => { addError = ''; if (e.key === 'Enter') submitPasteServer(); }}
      aria-label="URL de pareamento do servidor"
    />
    {#if addError}<p class="resume-err" role="alert">{addError}</p>{/if}
  </ConfirmDialog>
{/if}

<svelte:window onkeydown={(e) => { if (e.key === 'Escape') { if (kebabOpen) closeKebab(); else if (menu) closeMenu(); else if (resumeModal) resumeModal = null; else if (confirmDel) confirmDel = null; else if (confirmSrv) confirmSrv = null; else if (confirmBranch) confirmBranch = null; else if (confirmLogout) confirmLogout = false; } }} />

<!-- Menu de contexto (botao direito na sessao). Backdrop + itens vivem no componente; o Sidebar so
     guarda posicao/alvo em `menu` e decide o que dirty->confirm / checkout / GitSheet fazem. -->
{#if menu}
  {@const m = menu}
  <SessionContextMenu x={m.x} y={m.y} name={m.name} serverId={m.serverId} cwd={m.cwd} thenTarget={m.thenTarget}
    chainCandidates={chainCandidates(m.serverId, m.name)}
    onClose={closeMenu}
    onRename={menuRename} onDelete={menuDelete} onGit={menuGit}
    onPickBranch={(branch, dirty) => {
      if (dirty) confirmBranch = { name: m.name, serverId: m.serverId, branch };
      else doCheckout(m.name, m.serverId, branch);
    }}
    onFlash={flash} />
{/if}
{#if menuMsg}<div class="menu-toast" role="status">{menuMsg}</div>{/if}

<!-- Gerenciador git aberto pelo menu de contexto (repo da sessao, sem abrir o chat). -->
{#if gitSheet}
  <GitSheet open={true} sessionName={gitSheet.name} onClose={closeGitSheet} />
{/if}

<!-- Confirmar remocao de servidor (com o nome) — mesmo padrao do excluir sessao. -->
{#if confirmSrv}
  <ConfirmDialog title="Remover este servidor?" aria="Confirmar remoção de servidor"
    onClose={() => (confirmSrv = null)}
    actions={[
      { label: 'Cancelar', onClick: () => (confirmSrv = null) },
      { label: 'Remover', kind: 'danger', onClick: doDropServer },
    ]}>
    <p class="confirm-name">{confirmSrv.label}</p>
  </ConfirmDialog>
{/if}

<!-- Confirmar exclusao (com o nome) — modal centrado, so desktop (sidebar e desktop-only). -->
{#if confirmDel}
  <ConfirmDialog title="Excluir esta sessão?" aria="Confirmar exclusão"
    onClose={() => (confirmDel = null)}
    actions={[
      { label: 'Cancelar', onClick: () => (confirmDel = null) },
      { label: 'Excluir', kind: 'danger', onClick: doDelete },
    ]}>
    <p class="confirm-name">{confirmDel.name}</p>
    <!-- Um mesmo nome pode existir em varios servidores: mostra o dono pra a exclusao nao ser ambigua. -->
    {#if servers.length > 1}
      {@const srv = servers.find((s) => s.id === confirmDel?.serverId)}
      {#if srv}
        <p class="confirm-srv"><span class="confirm-srv-dot" style="background: {serverColor(srv.id)};" aria-hidden="true"></span>{srv.label}</p>
      {/if}
    {/if}
  </ConfirmDialog>
{/if}

<!-- Confirmar troca de branch com working tree suja (switch carrega mudancas nao-commitadas). -->
{#if confirmBranch}
  <ConfirmDialog title="Trocar de branch com mudanças não salvas?" aria="Confirmar troca de branch"
    onClose={() => (confirmBranch = null)}
    actions={[
      { label: 'Cancelar', onClick: () => (confirmBranch = null) },
      { label: 'Trocar assim', onClick: () => { const c = confirmBranch; confirmBranch = null; if (c) doCheckout(c.name, c.serverId, c.branch); } },
      { label: 'Guardar e trocar', kind: 'primary', onClick: () => { const c = confirmBranch; confirmBranch = null; if (c) stashAndCheckout(c.name, c.serverId, c.branch); } },
    ]}>
    <p class="confirm-name">→ {confirmBranch.branch}</p>
    <p class="confirm-hint">Há alterações não commitadas. <strong>Guardar e trocar</strong> põe elas no stash (recupera com “pop” na aba Git). <strong>Trocar assim</strong> deixa o git carregá-las — e pode recusar se conflitar.</p>
  </ConfirmDialog>
{/if}

<!-- Retomar conversa — caso AMBIGUO (varias sessoes no mesmo cwd): escolher qual transcript retomar.
     Paridade com o BottomSheet de resume do mobile (SessionList), no estilo dos modais do desktop. -->
{#if resumeModal}
  {@const rm = resumeModal}
  <ConfirmDialog title="Retomar qual conversa?" aria="Retomar conversa" role="dialog" wide
    onClose={() => (resumeModal = null)}
    actions={[{ label: 'Fechar', onClick: () => (resumeModal = null) }]}>
    <p class="confirm-hint">Há mais de uma sessão nesta pasta — escolha o transcript pra continuar em <strong>{rm.name}</strong>.</p>
    {#if resumeError}<p class="resume-err">{resumeError}</p>{/if}
    <ul class="resume-list">
      {#each rm.candidates as c (c.session_id)}
        <li>
          <button
            type="button"
            class="resume-item"
            disabled={c.in_use || resumeBusy === rm.name}
            onclick={() => handleResume(rm.name, rm.serverId, c.session_id)}
          >
            <span class="resume-item-preview">{c.preview || '(sem prévia)'}</span>
            <span class="resume-item-meta">
              {fmtWhen(c.mtime)}{#if c.in_use} · em uso por outra sessão{/if}
            </span>
          </button>
        </li>
      {/each}
    </ul>
  </ConfirmDialog>
{/if}

<!-- Confirmar saida (recuperacao exige token/QR de novo). -->
{#if confirmLogout}
  <ConfirmDialog title="Sair do app?" aria="Confirmar saída"
    onClose={() => (confirmLogout = false)}
    actions={[
      { label: 'Cancelar', onClick: () => (confirmLogout = false) },
      { label: 'Sair', kind: 'danger', onClick: () => { confirmLogout = false; logout(); } },
    ]}>
    <p class="confirm-hint">Você vai precisar do token (QR ou digitado) pra entrar de novo — e ele pode estar no PC.</p>
  </ConfirmDialog>
{/if}

<style>
  .sidebar {
    position: relative;   /* ancora o resize-handle */
    width: 270px;
    flex-shrink: 0;
    height: 100%;
    display: flex;
    flex-direction: column;
    /* Glass desktop: fundo quase opaco SEM blur (mesma linha do composer/navbar — consistência +
       zero custo de backdrop-filter). Sheen no topo + brilho de borda mantêm a cara de vidro. */
    background: var(--glass-bg-solid);
    border-right: 1px solid var(--glass-border);
    box-shadow:
      inset 0 1px 1px var(--glass-specular),   /* rim no topo */
      inset -1px 0 0 var(--glass-highlight);    /* luz na borda direita */
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
    background: var(--glass-bg);
    backdrop-filter: url(#liquid-glass) blur(20px) saturate(180%);
  }
  .sidebar.collapsed { width: 56px; padding: var(--space-3) var(--space-2); }

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
  .icon-btn {
    width: 36px; height: 36px; flex-shrink: 0; border-radius: var(--radius-md);
    color: var(--text-secondary); display: inline-flex; align-items: center; justify-content: center;
  }
  .icon-btn:active, .icon-btn:hover { background: var(--bg-hover); }
  .side-brand { flex: 1; min-width: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  /* Toggles do header (modo selecao / quadro): mesma caixa de 36px, acesa em accent quando ativa. */
  .select-toggle-btn, .board-btn {
    flex-shrink: 0; width: 36px; height: 36px;
    border-radius: var(--radius-md); color: var(--text-secondary);
  }
  .select-toggle-btn:hover, .board-btn:hover { background: var(--bg-hover); }
  .select-toggle-btn.active, .board-btn.active { color: var(--accent); background: var(--accent-dim); }

  /* ── Kebab "⋯" do header + seu popover (nav secundária + agrupamento) ── */
  .kebab-btn {
    flex-shrink: 0; width: 36px; height: 36px;
    border-radius: var(--radius-md); color: var(--text-secondary);
    display: inline-flex; align-items: center; justify-content: center;
  }
  .kebab-btn:hover, .kebab-btn.active { background: var(--bg-hover); color: var(--text-primary); }
  /* Rail recolhido: o header empilha (recolher em cima, kebab embaixo) — a nav secundária segue
     acessível num toque sem precisar expandir. */
  .sidebar.collapsed .side-top { flex-direction: column; gap: var(--space-2); }
  /* Popover do kebab: mesmo visual do menu de contexto (bg/borda/sombra), aberto pra baixo. */
  .kebab-menu {
    position: fixed; z-index: 41; min-width: 220px; padding: 4px;
    display: flex; flex-direction: column;
    background: var(--bg-elevated); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); box-shadow: 0 8px 28px rgba(0, 0, 0, 0.4);
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
    font-size: var(--text-xs); font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase;
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
    background: var(--bg-base); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
  }
  .group-toggle button {
    min-height: 0; height: 24px; min-width: 0; padding: 0 var(--space-2);
    border-radius: var(--radius-sm); font-size: var(--text-xs); font-weight: 500; color: var(--text-muted);
  }
  @media (hover: hover) { .group-toggle button:hover { color: var(--text-secondary); } }
  .group-toggle button.active { background: var(--bg-elevated); color: var(--text-primary); font-weight: 600; }
  /* Filtro (paridade com o mobile) — compacto, alinhado ao conteudo da sidebar. */
  .filter-input {
    width: 100%; height: 32px; margin: 0 0 var(--space-2); padding: 0 var(--space-2);
    background: var(--bg-base); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
    color: var(--text-primary); font-family: var(--font-ui); font-size: var(--text-sm); outline: none;
    transition: border-color 160ms var(--ease-out);
  }
  .filter-input::placeholder { color: var(--text-muted); }
  .filter-input:focus { border-color: var(--accent); }
  .filter-empty { font-size: var(--text-xs); color: var(--text-muted); text-align: center; padding: var(--space-4) var(--space-2); }
  /* Precisa de você (AttentionFeed): alinha a caixa a largura do conteudo da sidebar (margens laterais
     do componente sao pensadas pro mobile — desktop-only, nao mexe no mobile). */
  .sess-list :global(.attn) { margin-left: 0; margin-right: 0; margin-bottom: var(--space-2); border-radius: var(--radius-md); }
  /* Header do grupo virou uma row (label + "enviar p/ todas", feature #9). */
  .grp-head-row { display: flex; align-items: center; }
  .grp-head-row:not(:first-child) { margin-top: var(--space-2); }
  .grp-head-row .grp-head { flex: 1; min-width: 0; }
  .grp-head {
    display: flex; align-items: center; gap: var(--space-2);
    width: 100%; text-align: left;
    padding: var(--space-2) var(--space-2) 4px;
    font-size: var(--text-xs); font-weight: 600; color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.04em; border-radius: var(--radius-sm);
  }
  @media (hover: hover) { .grp-head:hover { color: var(--text-secondary); } }

  /* Sub-header do cluster de pareamento (Opção C): recuado sob o servidor, cor accent, colapsável. */
  .pair-head {
    display: flex; align-items: center; gap: var(--space-2);
    width: 100%; text-align: left;
    padding: 4px var(--space-2) 4px var(--space-4);
    background: none; border: none; cursor: pointer;
    font-size: var(--text-xs); font-weight: 600; color: var(--accent);
    border-radius: var(--radius-sm);
  }
  .pair-head-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  @media (hover: hover) { .pair-head:hover { background: var(--bg-hover); } }
  /* Linha de MEMBRO do cluster: faixa accent na borda esquerda liga as sessões do grupo. */
  .sess-row.pair-member { border-left: 2px solid var(--accent-dim); margin-left: var(--space-2); }
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
    color: var(--text-muted); background: var(--bg-base);
    border-radius: var(--radius-full); padding: 0 6px; min-width: 18px; text-align: center;
  }
  /* Badge de aguardando (âmbar) — numero + title "N aguardando". */
  .grp-await {
    flex-shrink: 0; text-transform: none; letter-spacing: 0; font-weight: 700;
    color: var(--warning); background: rgba(255, 159, 10, 0.14);
    border-radius: var(--radius-full); padding: 0 6px; min-width: 18px; text-align: center;
    font-variant-numeric: tabular-nums;
  }
  .grp-off { color: var(--warning); font-weight: 600; text-transform: none; letter-spacing: 0; }
  .grp-broadcast {
    flex-shrink: 0; width: 24px; height: 24px; margin-right: var(--space-1);
    color: var(--text-muted); font-size: var(--text-xs); border-radius: var(--radius-sm);
  }
  .grp-broadcast:hover { color: var(--accent); background: var(--bg-hover); }
  /* Checkbox do modo seleção: so decorativo (o toque na row inteira alterna). */
  .select-check { width: 16px; height: 16px; accent-color: var(--accent); pointer-events: none; }
  .sess-row { display: flex; align-items: center; border-radius: var(--radius-md); }
  /* hover SÓ em dispositivo com mouse. No touch (tablet), o :hover fazia o 1º toque virar "hover" e
     o 2º o clique -> precisava de 2 toques pra abrir a sessão. hover:hover isola isso. */
  @media (hover: hover) { .sess-row:hover { background: var(--bg-hover); } }
  .sess-row.active { background: var(--bg-elevated); }
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
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2); min-height: 46px;
    padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--text-secondary);
    border-radius: var(--radius-md);
  }
  /* Modo compacto: só lead+nome+chips — cabe o dobro de sessões. Saem as 3 linhas secundárias
     (subtítulo de estado, cwd e branch). A altura mora na .sess-main (a .sess-row não tem
     min-height), então é ela que encolhe; os chips de estado/🤝 ficam, são o sinal da linha. */
  .sess-list.compact .sess-main { min-height: 34px; }
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
  .cwd-base { flex: 0 0 auto; white-space: nowrap; color: var(--text-secondary); }
  /* Branch git da linha (paridade com o mobile): mono, accent, truncada. */
  .branch {
    min-width: 0; font-family: var(--font-mono); font-size: 10px; color: var(--accent);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .state-chip {
    flex-shrink: 0; font-size: 10px; font-weight: 600; letter-spacing: 0.02em;
    padding: 2px 7px; border-radius: var(--radius-full); white-space: nowrap;
  }
  /* Travada (feature #7): anel âmbar sutil no chip — avisa sem gritar. */
  .state-chip.stalled {
    box-shadow: inset 0 0 0 1px var(--warning);
  }
  /* Rate-limit radar (feature #8): chip proprio, mesma familia visual do stalled (âmbar, calmo). */
  .limited-chip {
    flex-shrink: 0; font-size: 10px; font-weight: 600; letter-spacing: 0.02em;
    padding: 2px 7px; border-radius: var(--radius-full); white-space: nowrap;
    color: var(--warning); background: rgba(255, 159, 10, 0.12);
    font-variant-numeric: tabular-nums;
  }
  /* Feature #12: indicador do vinculo 'then' — mesmo formato do limited-chip, cor neutra (accent). */
  .chain-chip {
    flex-shrink: 0; font-size: 10px; font-weight: 600; letter-spacing: 0.02em;
    padding: 2px 7px; border-radius: var(--radius-full); white-space: nowrap;
    max-width: 96px; overflow: hidden; text-overflow: ellipsis;
    color: var(--accent); background: var(--accent-dim);
  }
  .lead { width: 18px; flex-shrink: 0; display: inline-flex; align-items: center; justify-content: center; }
  /* Rail recolhido: iniciais precisam de mais espaco que o icone de 18px. */
  .sidebar.collapsed .lead { width: auto; }
  .initials {
    width: 30px; height: 30px; border-radius: 8px;
    display: flex; align-items: center; justify-content: center;
    font-size: 11px; font-weight: 700; letter-spacing: 0.02em;
    border: 1px solid;
  }
  /* Travada (feature #7) no rail recolhido: anel âmbar sutil, mesma ideia do .state-chip.stalled. */
  .initials.stalled {
    box-shadow: inset 0 0 0 1px var(--warning);
  }
  .sidebar.collapsed .sess-row { justify-content: center; }
  .sidebar.collapsed .sess-main { justify-content: center; padding: 0; }
  .sess-row.active .sess-main { color: var(--text-primary); }
  .sess-name { flex: 1; min-width: 0; font-size: var(--text-sm); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sess-main.untracked { opacity: 0.45; cursor: default; }
  .sess-badge {
    flex-shrink: 0; font-size: 10px; padding: 1px 5px; border-radius: var(--radius-sm);
    background: var(--bg-elevated); border: 1px solid var(--border-subtle); color: var(--warning); white-space: nowrap;
  }
  .sess-edit {
    flex: 1; min-width: 0; height: 38px; padding: 0 var(--space-2);
    background: var(--bg-base); border: 1px solid var(--accent); border-radius: var(--radius-md);
    color: var(--text-primary); font-size: var(--text-sm); outline: none;
  }
  .sess-del {
    width: 22px; height: 22px; min-height: 0; flex-shrink: 0; border-radius: var(--radius-sm);
    color: var(--text-muted); font-size: var(--text-base); line-height: 1; opacity: 0; margin-right: 2px;
  }
  @media (hover: hover) { .sess-row:hover .sess-del { opacity: 1; } }
  @media (hover: none) { .sess-del { opacity: 0.55; } }   /* touch: × sempre visível, sem o trap do :hover */
  .sess-del:hover { color: var(--error); background: var(--bg-base); }
  /* Git da linha: mesma mecânica hover-revealed do ×, tinge de accent ao passar. */
  .sess-git {
    width: 22px; height: 22px; min-height: 0; flex-shrink: 0; border-radius: var(--radius-sm);
    display: inline-flex; align-items: center; justify-content: center;
    color: var(--text-muted); opacity: 0;
  }
  @media (hover: hover) { .sess-row:hover .sess-git { opacity: 1; } }
  @media (hover: none) { .sess-git { opacity: 0.55; } }   /* touch: sempre visível */
  .sess-git:hover { color: var(--accent); background: var(--bg-base); }
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
    background: var(--bg-base); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
    color: var(--text-primary); font-size: var(--text-sm); padding: 0 var(--space-2); outline: none;
  }
  .broadcast-input:focus { border-color: var(--accent); }
  .broadcast-send {
    width: 34px; height: 34px; flex-shrink: 0;
    background: var(--accent); border-radius: var(--radius-sm); color: #fff; font-size: var(--text-sm);
  }
  .broadcast-send:disabled { background: var(--bg-hover); color: var(--text-muted); }

  /* ── Rodapé: conta (avatar -> menu) + CTA "Nova sessão" ── */
  .side-foot {
    display: flex; align-items: center; gap: var(--space-2);
    border-top: 1px solid var(--border-subtle); padding-top: var(--space-2); margin-top: var(--space-1);
  }
  .side-foot.rail { flex-direction: column; }
  /* Âncora do popover do menu de conta (abre pra cima). */
  .account-anchor { position: relative; flex: 1; min-width: 0; }
  .side-foot.rail .account-anchor { flex: 0 0 auto; }
  .acct-btn {
    display: flex; align-items: center; gap: var(--space-2); width: 100%; min-width: 0;
    padding: var(--space-1) var(--space-2); justify-content: flex-start; text-align: left;
    color: var(--text-primary); border-radius: var(--radius-md);
  }
  .acct-btn:hover { background: var(--bg-hover); }
  .side-foot.rail .acct-btn { justify-content: center; padding: var(--space-1); }
  .acct-avatar {
    width: 30px; height: 30px; flex-shrink: 0; border-radius: 50%;
    display: grid; place-items: center;
    background: linear-gradient(135deg, var(--accent), #a06de0);
    color: #fff; font-size: var(--text-xs); font-weight: 700;
  }
  .acct-who { min-width: 0; display: flex; flex-direction: column; }
  .acct-name { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .acct-sub { font-size: var(--text-xs); color: var(--text-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .cta-new {
    display: flex; align-items: center; gap: var(--space-1); flex-shrink: 0;
    height: 36px; padding: 0 var(--space-3);
    background: var(--accent); color: #fff; font-size: var(--text-sm); font-weight: 600;
    border-radius: var(--radius-full); white-space: nowrap;
  }
  .cta-new svg { flex-shrink: 0; }
  .cta-new:hover { background: var(--accent-press); }
  .side-foot.rail .cta-new { width: 36px; padding: 0; justify-content: center; }

  /* Input do modal "Adicionar servidor" (colar URL de pareamento). */
  .add-srv-input {
    width: 100%; height: 44px; padding: 0 var(--space-3);
    background: var(--bg-base); border: 1px solid var(--border-default); border-radius: var(--radius-md);
    color: var(--text-primary); font-family: var(--font-ui); font-size: var(--text-sm); outline: none;
  }
  .add-srv-input::placeholder { color: var(--text-muted); }
  .add-srv-input:focus { border-color: var(--accent); }

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
    padding: var(--space-2) var(--space-3); background: var(--bg-base);
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
    padding: var(--space-3); background: var(--bg-base);
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
    background: var(--bg-elevated); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); box-shadow: 0 6px 20px rgba(0,0,0,0.35);
    color: var(--text-primary); font-size: var(--text-sm); font-family: var(--font-mono);
    white-space: pre-wrap; word-break: break-word; max-height: 40vh; overflow-y: auto;
  }
</style>
