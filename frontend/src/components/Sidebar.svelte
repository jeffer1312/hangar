<script lang="ts">
  import { onMount } from 'svelte';
  import { openSessionsStream, createSession, deleteSession, renameSession, openEditor, gitAction, getBranches, checkoutBranch, getPushSettings, setSessionMute, broadcast, setThenLink, clearThenLink } from '../lib/api';
  import { listServers, getActiveId, selectServer, removeServer, addServer, renameServer, serverColor, clearCredentials } from '../lib/auth';
  import CreateSessionSheet from './CreateSessionSheet.svelte';
  import QrScanner from './QrScanner.svelte';
  import GitSheet from './GitSheet.svelte';
  import AttentionFeed from './AttentionFeed.svelte';
  import type { SessionInfo, State, AggSession } from '../lib/types';
  import { stateLabels, stateColors, countAwaiting, groupSelectedByServer, projectKey, projectLabel } from '../lib/format';
  import { updateBadge } from '../lib/badge';
  import type { Server } from '../lib/auth';
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
  }
  let { currentSession, onSelect, onCompare, onLogout }: Props = $props();

  // Grupo generico: por SERVIDOR (hoje) ou por PROJETO (cwd) — mesmo shape nos dois modos. Cada
  // sessao carrega o serverId dela (no modo projeto um grupo pode juntar sessoes de servidores
  // diferentes; no modo servidor e sempre o mesmo serverId do grupo, mas mantem uniforme pro
  // template nao precisar saber em qual modo esta).
  interface SessRow extends SessionInfo { serverId: string }
  interface Group { id: string; label: string; color: string | null; error: string | null; sessions: SessRow[] }
  let groups = $state<Group[]>([]);
  let collapsed = $state(false);   // pin: recolhido persistente (botao)
  let hovering = $state(false);    // hover sobre a sidebar recolhida -> expande temporario

  // Toggle "Servidor | Projeto" (feature #3), persistido — mesmo padrao de cp_sidebar_w.
  const GROUP_BY_KEY = 'cp_group_by';
  type GroupBy = 'server' | 'project';
  let groupBy = $state<GroupBy>(localStorage.getItem(GROUP_BY_KEY) === 'project' ? 'project' : 'server');
  function setGroupBy(mode: GroupBy) {
    groupBy = mode;
    try { localStorage.setItem(GROUP_BY_KEY, mode); } catch { /* storage cheio/off */ }
    recompute();
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
  let servers = $state(listServers());
  let activeId = $state(getActiveId());
  let scanning = $state(false);
  let showCreate = $state(false);
  let serversOpen = $state(false);

  // Iniciais pro rail recolhido (identificar sem o nome). "claude-pocket"->CP, "jeffer1312"->JE.
  function initials(name: string): string {
    const parts = name.split(/[^a-zA-Z0-9]+/).filter(Boolean);
    if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
    return (parts[0] ?? name).slice(0, 2).toUpperCase();
  }

  // Ordena DENTRO de cada grupo por nome (ordem alfabética estável — não pula quando a atividade muda).
  function sortSessions<T extends SessionInfo>(list: T[]): T[] {
    return [...list].sort((a, b) => a.name.localeCompare(b.name));
  }

  const slots = new Map<string, { sessions: SessionInfo[] | null; error: string | null }>();
  function recompute() {
    if (servers.length === 0) { groups = []; return; }
    const seen = new Set<string>(); // dedup global: backend compartilhado por 2 URLs não duplica
    if (groupBy === 'project') {
      // Modo projeto: junta sessoes de TODOS os servidores pela chave do cwd. Servidor offline so
      // perde as sessoes dele (sem banner por grupo — nao ha "1 servidor" pra apontar o erro).
      const byKey = new Map<string, SessRow[]>();
      for (const srv of servers) {
        const slot = slots.get(srv.id);
        if (!slot?.sessions) continue;
        for (const s of slot.sessions) {
          const key = `${s.jsonl ?? s.cwd ?? ''}::${s.name}`;
          if (seen.has(key)) continue;
          seen.add(key);
          const pk = projectKey(s.cwd);
          const arr = byKey.get(pk);
          const row: SessRow = { ...s, serverId: srv.id };
          if (arr) arr.push(row); else byKey.set(pk, [row]);
        }
      }
      groups = [...byKey.entries()]
        .map(([key, rows]) => ({ id: key, label: projectLabel(rows[0]?.cwd), color: null, error: null, sessions: sortSessions(rows) }))
        .sort((a, b) => a.label.localeCompare(b.label));
      return;
    }
    // Modo servidor (comportamento de sempre): 1 grupo por servidor, sempre presente (mesmo vazio
    // ou offline), na ORDEM de `servers`.
    groups = servers.map((srv) => {
      const slot = slots.get(srv.id);
      if (!slot || !slot.sessions) return { id: srv.id, label: srv.label, color: serverColor(srv.id), error: slot?.error ?? null, sessions: [] };
      const fresh = slot.sessions.filter((s) => {
        const key = `${s.jsonl ?? s.cwd ?? ''}::${s.name}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      }).map((s): SessRow => ({ ...s, serverId: srv.id }));
      return { id: srv.id, label: srv.label, color: serverColor(srv.id), error: null, sessions: sortSessions(fresh) };
    });
  }

  let streams = new Map<string, EventSource>();
  function connect(list: Server[]) {
    for (const [id, es] of streams) {
      if (!list.some((s) => s.id === id)) { es.close(); streams.delete(id); slots.delete(id); }
    }
    for (const s of list) {
      if (streams.has(s.id)) continue;
      const es = openSessionsStream(s);
      es.addEventListener('sessions', (e) => {
        slots.set(s.id, { sessions: JSON.parse((e as MessageEvent).data), error: null });
        recompute();
      });
      es.onerror = () => {
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        recompute();
      };
      streams.set(s.id, es);
    }
    recompute();
  }
  onMount(() => {
    servers = listServers();
    connect(servers);
    return () => { for (const es of streams.values()) es.close(); streams.clear(); };
  });

  async function handleCreate(name: string, cwd?: string, configDir?: string | null) {
    // O CreateSessionSheet já posicionou o servidor-alvo como ativo (selectServer).
    await createSession(name, cwd, configDir);
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
    try { await deleteSession(name); } catch { /* ignora; SSE corrige */ }
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
    menu = { x: e.clientX, y: e.clientY, name: s.name, serverId, cwd: s.cwd ?? '', thenTarget: s.then_target ?? null };
    // Silenciar (feature #5): estado carregado sob demanda (nao trava a abertura do menu). null =
    // ainda carregando/desconhecido -> o botao mostra "Silenciar" ate a resposta chegar.
    menuMuted = null;
    withServer(serverId, () => getPushSettings())
      .then((p) => { if (menu?.name === s.name) menuMuted = p.muted.includes(s.name); })
      .catch(() => { menuMuted = false; });
  }
  function closeMenu() { menu = null; branchView = null; chainView = null; }
  let menuMuted = $state<boolean | null>(null);
  async function menuToggleMute() {
    if (!menu) return;
    const { name, serverId } = menu;
    const next = !menuMuted;
    closeMenu();
    try {
      await withServer(serverId, () => setSessionMute(name, next));
      flash(next ? 'notificações silenciadas' : 'notificações religadas');
    } catch (e) { flash(`silenciar: ${errMsg(e)}`); }
  }

  // Submenu "Trocar branch" (2a pagina do menu, evita flyout). branchView != null = mostrando a lista.
  let branchView = $state<{ list: string[]; current: string | null; dirty: boolean } | null>(null);
  // Confirmacao de troca com working tree suja (switch carrega mudancas nao-conflitantes pra outra branch).
  let confirmBranch = $state<{ name: string; serverId: string; branch: string } | null>(null);
  let branchLoading = $state(false);
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
  async function menuBranches() {
    if (!menu) return;
    const { name, serverId } = menu;
    branchView = { list: [], current: null, dirty: false };
    branchLoading = true;
    try {
      const info = await withServer(serverId, () => getBranches(name));
      branchView = { list: info.branches, current: info.current, dirty: info.dirty ?? false };
    } catch (e) {
      branchView = null;
      flash(`branches: ${errMsg(e)}`);
    } finally {
      branchLoading = false;
    }
  }
  async function pickBranch(branch: string) {
    if (!menu) return;
    const { name, serverId } = menu;
    const cur = branchView?.current;
    const dirty = branchView?.dirty ?? false;
    if (branch === cur) { closeMenu(); return; }
    // Tree suja: switch levaria as mudancas nao-commitadas pra outra branch -> confirma antes.
    if (dirty) {
      confirmBranch = { name, serverId, branch };
      closeMenu();
      return;
    }
    closeMenu();
    await doCheckout(name, serverId, branch);
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

  // Submenu "Quando terminar, enviar p/…" (feature #12, 2a pagina do menu — mesma ideia do branchView):
  // picker de sessao ALVO (mesmo servidor da fonte, ThenLink/registry sao locais ao backend) + texto do
  // prompt. target=null ate escolher; pre-preenche com o vinculo ja armado (so o alvo — o texto fica no
  // backend e nao volta por GET, entao reabrir pra editar exige redigitar).
  let chainView = $state<{ target: string | null; text: string } | null>(null);
  let chainBusy = $state(false);
  function menuChain() {
    if (!menu) return;
    chainView = { target: menu.thenTarget, text: '' };
  }
  // Candidatas ao encadeamento: sessoes do MESMO servidor da fonte (o vinculo e resolvido pelo backend
  // dessa sessao — nao ha como encadear pra uma sessao de OUTRO servidor), exceto ela mesma.
  function chainCandidates(serverId: string, exclude: string) {
    return groups.flatMap((g) => g.sessions).filter((s) => s.serverId === serverId && s.name !== exclude);
  }
  async function saveChain() {
    if (!menu || !chainView?.target) return;
    const text = chainView.text.trim();
    if (!text) return;
    const { name, serverId } = menu;
    const target = chainView.target;
    chainBusy = true;
    try {
      await withServer(serverId, () => setThenLink(name, target, text));
      flash(`encadeado → ${target}`);
    } catch (e) {
      flash(`encadear: ${errMsg(e)}`);
    } finally {
      chainBusy = false;
      closeMenu();
    }
  }
  async function removeChain() {
    if (!menu) return;
    const { name, serverId } = menu;
    closeMenu();
    try {
      await withServer(serverId, () => clearThenLink(name));
      flash('vínculo removido');
    } catch (e) {
      flash(`remover vínculo: ${errMsg(e)}`);
    }
  }

  // api.ts mira SEMPRE o server ativo -> aponta pro dono da sessao e restaura (igual handleDelete).
  // Propaga o erro (o caller decide mostrar): o finally garante o restore mesmo em throw.
  async function withServer<T>(serverId: string, fn: () => Promise<T>): Promise<T> {
    const prev = getActiveId();
    selectServer(serverId);
    try { return await fn(); }
    finally { if (prev && prev !== serverId) selectServer(prev); }
  }
  const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

  async function copyToClipboard(s: string) {
    try { await navigator.clipboard.writeText(s); }
    catch {  // LAN via HTTP puro: clipboard API off -> fallback execCommand.
      const ta = document.createElement('textarea');
      ta.value = s; ta.style.position = 'fixed'; ta.style.opacity = '0';
      document.body.appendChild(ta); ta.select(); document.execCommand('copy'); ta.remove();
    }
  }

  function menuRename() {
    if (!menu) return;
    editing = `${menu.serverId}::${menu.name}`;
    editValue = menu.name;
    closeMenu();
  }
  function menuCopyCwd() {
    if (!menu) return;
    copyToClipboard(menu.cwd);
    closeMenu();
  }
  async function menuOpenEditor() {
    if (!menu) return;
    const { name, serverId } = menu;
    closeMenu();
    try { await withServer(serverId, () => openEditor(name)); }
    catch (e) { flash(`abrir editor: ${errMsg(e)}`); }   // 404 = backend desatualizado; 500 = CP_EDITOR/DISPLAY
  }
  async function menuGitPull() {
    if (!menu) return;
    const { name, serverId } = menu;
    closeMenu();
    flash('git pull…');
    try {
      const r = await withServer(serverId, () => gitAction(name, 'pull'));
      flash(r.output.trim().split('\n')[0] || 'pull ok');
    } catch (e) { flash(`git pull: ${errMsg(e)}`); }
  }
  function menuDelete() {
    if (!menu) return;
    confirmDel = { name: menu.name, serverId: menu.serverId };
    closeMenu();
  }

  // Rename inline de servidor no menu do rodape (mesma ideia do mobile). Label custom persistido.
  let editingServer = $state<string | null>(null);
  let editServerLabel = $state('');
  function startServerRename(id: string, current: string) {
    editingServer = id;
    editServerLabel = current;
  }
  function saveServerRename() {
    if (editingServer) {
      renameServer(editingServer, editServerLabel);
      servers = listServers();
      recompute(); // reagrega pra os headers de grupo pegarem o nome novo (sem esperar o próximo SSE)
    }
    editingServer = null;
  }

  function pickServer(id: string) {
    if (id === getActiveId()) { serversOpen = false; return; }
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
    removeServer(id);
    servers = listServers();
    activeId = getActiveId();
    if (servers.length === 0) { clearCredentials(); onLogout(); return; }
    connect(servers);
    if (was) window.location.reload();
  }
  function handleScan(text: string) {
    let tok = text.trim();
    let base = '';
    try {
      const u = new URL(text);
      const t = u.searchParams.get('token');
      if (t) tok = t;
      base = u.searchParams.get('api') ?? u.origin;
    } catch { base = ''; }
    scanning = false;
    if (!tok || !base) return;
    addServer(base, tok);
    window.location.reload();
  }
  function logout() {
    clearCredentials();
    onLogout();
  }
  // Sair pede confirmacao (recuperacao exige o token/QR de novo).
  let confirmLogout = $state(false);

  const activeServer = $derived(servers.find((s) => s.id === activeId) ?? servers[0] ?? null);

  // Badge do ícone do app (feature #13): mesmo agregado do mobile (SessionList), so que a partir de
  // `groups` (por servidor) — flatten pra contar aguardando em TODOS os servidores.
  const awaitingTotal = $derived(groups.reduce((n, g) => n + countAwaiting(g.sessions), 0));
  $effect(() => { updateBadge(awaitingTotal); });

  // Fila cross-server pra a AttentionFeed (feature #6): achata os grupos (ja deduplicados no
  // recompute) e enriquece cada linha com label/cor do servidor. Independe do modo de agrupamento.
  const attnSessions = $derived<AggSession[]>(
    groups.flatMap((g) => g.sessions).map((s) => ({
      ...s,
      serverLabel: servers.find((v) => v.id === s.serverId)?.label ?? s.serverId,
      serverColor: serverColor(s.serverId),
    })),
  );

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
  </div>

  <div class="new-row">
    <button class="new-btn" onclick={() => (showCreate = true)} aria-label="Nova sessão">
      <span class="new-plus" aria-hidden="true">+</span>
      {#if expanded}<span>Nova sessão</span>{/if}
    </button>
    {#if expanded}
      <!-- Broadcast (feature #9): entra/sai do modo seleção multipla. -->
      <button
        class="select-toggle-btn"
        class:active={selectMode}
        onclick={toggleSelectMode}
        aria-label={selectMode ? 'Cancelar seleção' : 'Selecionar sessões'}
        title={selectMode ? 'Cancelar seleção' : 'Selecionar sessões'}
      >☑</button>
    {/if}
  </div>

  <nav class="sess-list" aria-label="Sessões">
    {#if expanded}
      <!-- "Precisa de você" (feature #6): fila cross-server de sessoes aguardando, fixa no topo.
           Picker inline (OptionButtons) responde sem abrir o chat; nativo AskUserQuestion abre. -->
      <AttentionFeed sessions={attnSessions} onOpenChat={(s) => onMainClick(s.name, s.serverId, s.tracked)} />
      <!-- Toggle Servidor|Projeto (feature #3): agrupa por servidor (hoje) ou por cwd. -->
      <div class="group-toggle" role="radiogroup" aria-label="Agrupar por">
        <button type="button" class:active={groupBy === 'server'} role="radio" aria-checked={groupBy === 'server'} onclick={() => setGroupBy('server')}>Servidor</button>
        <button type="button" class:active={groupBy === 'project'} role="radio" aria-checked={groupBy === 'project'} onclick={() => setGroupBy('project')}>Projeto</button>
      </div>
    {/if}
    {#each groups as g (g.id)}
      {#if expanded}
        <div class="grp-head-row">
          <div class="grp-head" title={g.error ? `${g.label}: ${g.error}` : g.label}>
            {#if g.color}<span class="grp-dot" style="background: {g.color};" aria-hidden="true"></span>{/if}
            <span class="grp-label">{g.label}</span>
            {#if g.error}<span class="grp-off">offline</span>{/if}
          </div>
          <!-- "enviar p/ todas" (feature #9): entra em modo seleção com o grupo inteiro marcado. -->
          <button
            class="grp-broadcast"
            onclick={() => selectGroupForBroadcast(g)}
            aria-label={`Enviar mensagem para todas as sessões de ${g.label}`}
            title="Enviar p/ todas"
          >➤</button>
        </div>
      {/if}
      {#each g.sessions as s (s.serverId + '::' + s.name)}
        {@const rowKey = `${s.serverId}::${s.name}`}
        {@const selKey = `${s.serverId}:${s.name}`}
        <div class="sess-row" class:active={s.serverId === activeId && s.name === currentSession}>
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
                <span
                  class="state-chip"
                  class:stalled={s.stalled === true}
                  style="color: {stateColors[s.state]}; background: {stateChipBg[s.state]};"
                  title={s.stalled ? 'Pode estar travada — sem atividade há um tempo' : undefined}
                >{stateLabels[s.state]}</span>
              {/if}
            </button>
            {#if expanded && !selectMode}
              <button class="sess-del" onclick={(e) => handleDelete(s.name, s.serverId, e)} aria-label={`Excluir ${s.name}`}>×</button>
            {/if}
          {/if}
        </div>
      {/each}
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
          {broadcastBusy ? '…' : '➤'}
        </button>
      </div>
      {#if broadcastIsSlash}
        <p class="broadcast-hint">Slash-commands não têm broadcast — envie dentro da sessão.</p>
      {/if}
    </div>
  {/if}

  {#if expanded}
    <div class="side-foot">
      {#if serversOpen}
        <div class="srv-menu">
          {#each servers as s (s.id)}
            <div class="srv-row">
              {#if editingServer === s.id}
                <span class="srv-dot" class:on={s.id === activeId} aria-hidden="true"></span>
                <input
                  class="srv-edit"
                  bind:value={editServerLabel}
                  use:autofocus
                  onkeydown={(e) => { if (e.key === 'Enter') saveServerRename(); if (e.key === 'Escape') editingServer = null; }}
                  onblur={saveServerRename}
                  aria-label="Novo nome do servidor"
                />
              {:else}
                <button class="srv-pick" onclick={() => pickServer(s.id)}>
                  <span class="srv-dot" class:on={s.id === activeId} aria-hidden="true"></span>
                  <span class="srv-label">{s.label}</span>
                </button>
                <button class="srv-rename" onclick={() => startServerRename(s.id, s.label)} aria-label={`Renomear ${s.label}`} title="Renomear">✎</button>
                {#if servers.length > 1}<button class="srv-del" onclick={() => dropServer(s.id)} aria-label="Remover">×</button>{/if}
              {/if}
            </div>
          {/each}
          <button class="srv-add" onclick={() => { scanning = true; serversOpen = false; }}>+ Adicionar (QR)</button>
        </div>
      {/if}
      <button class="server-btn" onclick={() => (serversOpen = !serversOpen)}>
        <span class="srv-dot on" aria-hidden="true"></span>
        <span class="srv-label">{activeServer?.label ?? 'servidor'}</span>
        <span class="srv-caret" aria-hidden="true">⌃</span>
      </button>
      <button class="costs-btn" onclick={() => (window.location.hash = '#/costs')}>Custos</button>
      <button class="logout-btn" onclick={() => (confirmLogout = true)}>Sair</button>
    </div>
  {/if}

  {#if !collapsed}
    <!-- Drag na borda direita pra redimensionar (so pin aberto). -->
    <div class="resize-handle" onpointerdown={resizeStart} onpointermove={resizeMove}
      onpointerup={resizeEnd} onpointercancel={resizeEnd}
      role="separator" aria-label="Redimensionar barra lateral" aria-orientation="vertical"></div>
  {/if}
</aside>

<CreateSessionSheet open={showCreate} {servers} onClose={() => (showCreate = false)} onCreate={handleCreate} onOpenSession={onSelect} />
{#if scanning}<QrScanner onScan={handleScan} onClose={() => (scanning = false)} />{/if}

<svelte:window onkeydown={(e) => { if (e.key === 'Escape') { if (menu) closeMenu(); else if (confirmDel) confirmDel = null; else if (confirmSrv) confirmSrv = null; else if (confirmBranch) confirmBranch = null; else if (confirmLogout) confirmLogout = false; } }} />

<!-- Menu de contexto (botao direito na sessao). Backdrop full-screen captura o clique-fora. -->
{#if menu}
  <div class="menu-backdrop" onclick={closeMenu} oncontextmenu={(e) => { e.preventDefault(); closeMenu(); }} role="presentation"></div>
  <div class="ctx-menu" style="left: {menu.x}px; top: {menu.y}px;" role="menu">
    {#if branchView}
      <button type="button" class="ctx-back" onclick={() => (branchView = null)}>‹ Trocar branch</button>
      <div class="ctx-sep"></div>
      {#if branchLoading}
        <div class="ctx-info">carregando…</div>
      {:else if branchView.list.length}
        <div class="ctx-scroll">
          {#each branchView.list as b (b)}
            <button type="button" role="menuitem" class="ctx-branch" class:current={b === branchView.current} onclick={() => pickBranch(b)}>
              {b}{#if b === branchView.current}<span class="ctx-cur">✓</span>{/if}
            </button>
          {/each}
        </div>
      {:else}
        <div class="ctx-info">sem branches</div>
      {/if}
    {:else if chainView}
      <button type="button" class="ctx-back" onclick={() => (chainView = null)}>‹ Quando terminar, enviar p/</button>
      <div class="ctx-sep"></div>
      {#if chainCandidates(menu.serverId, menu.name).length}
        <div class="ctx-scroll">
          {#each chainCandidates(menu.serverId, menu.name) as c (c.serverId + c.name)}
            <button
              type="button" role="menuitem" class="ctx-branch"
              class:current={c.name === chainView.target}
              onclick={() => { if (chainView) chainView.target = c.name; }}
            >{c.name}{#if c.name === chainView.target}<span class="ctx-cur">✓</span>{/if}</button>
          {/each}
        </div>
      {:else}
        <div class="ctx-info">nenhuma outra sessão neste servidor</div>
      {/if}
      <div class="ctx-sep"></div>
      <div class="ctx-chain-form">
        <input
          type="text"
          class="ctx-chain-input"
          placeholder="Prompt a enviar…"
          bind:value={chainView.text}
          onkeydown={(e) => { if (e.key === 'Enter') saveChain(); }}
          aria-label="Prompt a enviar pra sessão alvo"
        />
        <button
          type="button" class="ctx-chain-save" onclick={saveChain}
          disabled={!chainView.target || !chainView.text.trim() || chainBusy}
        >Salvar</button>
      </div>
      {#if menu.thenTarget}
        <div class="ctx-sep"></div>
        <button type="button" role="menuitem" class="danger" onclick={removeChain}>Remover vínculo</button>
      {/if}
    {:else}
      <button type="button" role="menuitem" onclick={menuRename}>Renomear</button>
      <button type="button" role="menuitem" onclick={menuToggleMute}>
        {menuMuted ? 'Reativar notificações' : 'Silenciar notificações'}
      </button>
      {#if menu.cwd}
        <button type="button" role="menuitem" onclick={menuCopyCwd}>Copiar cwd</button>
        <button type="button" role="menuitem" onclick={menuOpenEditor}>Abrir no editor</button>
        <div class="ctx-sep"></div>
        <button type="button" role="menuitem" onclick={menuGit}>Git<span class="ctx-more">›</span></button>
        <button type="button" role="menuitem" onclick={menuGitPull}>Git pull</button>
        <button type="button" role="menuitem" onclick={menuBranches}>Trocar branch<span class="ctx-more">›</span></button>
      {/if}
      <div class="ctx-sep"></div>
      <button type="button" role="menuitem" onclick={menuChain}>
        {menu.thenTarget ? `Encadeado → ${menu.thenTarget}` : 'Quando terminar, enviar p/…'}<span class="ctx-more">›</span>
      </button>
      <div class="ctx-sep"></div>
      <button type="button" role="menuitem" class="danger" onclick={menuDelete}>Excluir</button>
    {/if}
  </div>
{/if}
{#if menuMsg}<div class="menu-toast" role="status">{menuMsg}</div>{/if}

<!-- Gerenciador git aberto pelo menu de contexto (repo da sessao, sem abrir o chat). -->
{#if gitSheet}
  <GitSheet open={true} sessionName={gitSheet.name} onClose={closeGitSheet} />
{/if}

<!-- Confirmar remocao de servidor (com o nome) — mesmo padrao do excluir sessao. -->
{#if confirmSrv}
  <div class="confirm-backdrop" onclick={() => (confirmSrv = null)} role="presentation"></div>
  <div class="confirm-card" role="alertdialog" aria-modal="true" aria-label="Confirmar remoção de servidor">
    <p class="confirm-title">Remover este servidor?</p>
    <p class="confirm-name">{confirmSrv.label}</p>
    <div class="confirm-actions">
      <button type="button" class="c-btn" onclick={() => (confirmSrv = null)}>Cancelar</button>
      <button type="button" class="c-btn c-danger" onclick={doDropServer}>Remover</button>
    </div>
  </div>
{/if}

<!-- Confirmar exclusao (com o nome) — modal centrado, so desktop (sidebar e desktop-only). -->
{#if confirmDel}
  <div class="confirm-backdrop" onclick={() => (confirmDel = null)} role="presentation"></div>
  <div class="confirm-card" role="alertdialog" aria-modal="true" aria-label="Confirmar exclusão">
    <p class="confirm-title">Excluir esta sessão?</p>
    <p class="confirm-name">{confirmDel.name}</p>
    <div class="confirm-actions">
      <button type="button" class="c-btn" onclick={() => (confirmDel = null)}>Cancelar</button>
      <button type="button" class="c-btn c-danger" onclick={doDelete}>Excluir</button>
    </div>
  </div>
{/if}

<!-- Confirmar troca de branch com working tree suja (switch carrega mudancas nao-commitadas). -->
{#if confirmBranch}
  <div class="confirm-backdrop" onclick={() => (confirmBranch = null)} role="presentation"></div>
  <div class="confirm-card" role="alertdialog" aria-modal="true" aria-label="Confirmar troca de branch">
    <p class="confirm-title">Trocar de branch com mudanças não salvas?</p>
    <p class="confirm-name">→ {confirmBranch.branch}</p>
    <p class="confirm-hint">Há alterações não commitadas. <strong>Guardar e trocar</strong> põe elas no stash (recupera com “pop” na aba Git). <strong>Trocar assim</strong> deixa o git carregá-las — e pode recusar se conflitar.</p>
    <div class="confirm-actions">
      <button type="button" class="c-btn" onclick={() => (confirmBranch = null)}>Cancelar</button>
      <button type="button" class="c-btn" onclick={() => { const c = confirmBranch; confirmBranch = null; if (c) doCheckout(c.name, c.serverId, c.branch); }}>Trocar assim</button>
      <button type="button" class="c-btn c-primary" onclick={() => { const c = confirmBranch; confirmBranch = null; if (c) stashAndCheckout(c.name, c.serverId, c.branch); }}>Guardar e trocar</button>
    </div>
  </div>
{/if}

<!-- Confirmar saida (recuperacao exige token/QR de novo). -->
{#if confirmLogout}
  <div class="confirm-backdrop" onclick={() => (confirmLogout = false)} role="presentation"></div>
  <div class="confirm-card" role="alertdialog" aria-modal="true" aria-label="Confirmar saída">
    <p class="confirm-title">Sair do app?</p>
    <p class="confirm-hint">Você vai precisar do token (QR ou digitado) pra entrar de novo — e ele pode estar no PC.</p>
    <div class="confirm-actions">
      <button type="button" class="c-btn" onclick={() => (confirmLogout = false)}>Cancelar</button>
      <button type="button" class="c-btn c-danger" onclick={() => { confirmLogout = false; logout(); }}>Sair</button>
    </div>
  </div>
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

  .side-top { display: flex; align-items: center; gap: var(--space-2); min-height: 36px; }
  .icon-btn {
    width: 36px; height: 36px; flex-shrink: 0; border-radius: var(--radius-md);
    color: var(--text-secondary); display: inline-flex; align-items: center; justify-content: center;
  }
  .icon-btn:active, .icon-btn:hover { background: var(--bg-hover); }
  .side-brand { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); white-space: nowrap; }

  .new-row { display: flex; gap: var(--space-2); }
  .new-row .new-btn { flex: 1; min-width: 0; }
  .new-btn {
    display: flex; align-items: center; gap: var(--space-2); height: 40px; padding: 0 var(--space-3);
    border-radius: var(--radius-md); background: var(--accent-dim); color: var(--text-primary);
    font-size: var(--text-sm); font-weight: 500; justify-content: flex-start; white-space: nowrap;
  }
  .sidebar.collapsed .new-btn { justify-content: center; padding: 0; }
  .new-btn:hover { background: var(--accent); color: #fff; }
  .new-plus { font-size: var(--text-lg); line-height: 1; flex-shrink: 0; }
  /* Toggle do modo selecao (feature #9), ao lado de "Nova sessão". */
  .select-toggle-btn {
    flex-shrink: 0; width: 40px; height: 40px;
    border-radius: var(--radius-md); color: var(--text-secondary);
  }
  .select-toggle-btn:hover { background: var(--bg-hover); }
  .select-toggle-btn.active { color: var(--accent); background: var(--accent-dim); }

  .sess-list { flex: 1; overflow-y: auto; display: flex; flex-direction: column; gap: 2px; margin-top: var(--space-2); }
  /* Toggle Servidor|Projeto (feature #3): segmentado, mesma largura dos 2 lados. */
  .group-toggle {
    display: flex; gap: 2px; padding: 2px; margin: 0 0 var(--space-2);
    background: var(--bg-base); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
  }
  .group-toggle button { flex: 1; height: 28px; border-radius: var(--radius-sm); font-size: var(--text-xs); color: var(--text-secondary); }
  .group-toggle button.active { background: var(--bg-elevated); color: var(--text-primary); font-weight: 600; }
  /* Header do grupo virou uma row (label + "enviar p/ todas", feature #9). */
  .grp-head-row { display: flex; align-items: center; }
  .grp-head-row:not(:first-child) { margin-top: var(--space-2); }
  .grp-head-row .grp-head { flex: 1; min-width: 0; }
  .grp-head {
    display: flex; align-items: center; gap: var(--space-2);
    padding: var(--space-2) var(--space-2) 4px;
    font-size: var(--text-xs); font-weight: 600; color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.04em;
  }
  .grp-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; }
  .grp-label { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
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
  .sess-main {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2); min-height: 46px;
    padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--text-secondary);
    border-radius: var(--radius-md);
  }
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

  .side-foot { display: flex; flex-direction: column; gap: var(--space-1); border-top: 1px solid var(--border-subtle); padding-top: var(--space-2); }
  .server-btn {
    display: flex; align-items: center; gap: var(--space-2); height: 36px; padding: 0 var(--space-2);
    border-radius: var(--radius-md); justify-content: flex-start; color: var(--text-secondary);
  }
  .server-btn:hover { background: var(--bg-hover); }
  .srv-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--border-default); flex-shrink: 0; }
  .srv-dot.on { background: var(--accent); }
  .srv-label { flex: 1; min-width: 0; font-size: var(--text-sm); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .srv-caret { color: var(--text-muted); font-size: var(--text-xs); }
  .srv-menu { display: flex; flex-direction: column; gap: 2px; padding: var(--space-1); background: var(--bg-elevated); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); margin-bottom: var(--space-1); }
  .srv-row { display: flex; align-items: center; }
  .srv-pick { flex: 1; display: flex; align-items: center; gap: var(--space-2); height: 32px; padding: 0 var(--space-2); justify-content: flex-start; color: var(--text-primary); font-size: var(--text-sm); border-radius: var(--radius-sm); }
  .srv-pick:hover { background: var(--bg-hover); }
  .srv-rename { width: 28px; height: 32px; min-height: 0; flex-shrink: 0; color: var(--text-muted); font-size: var(--text-sm); }
  .srv-rename:hover { color: var(--accent); }
  .srv-edit {
    flex: 1; min-width: 0; height: 32px; margin-left: var(--space-2); padding: 0 var(--space-2);
    background: var(--bg-base); border: 1px solid var(--accent); border-radius: var(--radius-sm);
    color: var(--text-primary); font-size: var(--text-sm); outline: none;
  }
  .srv-del { width: 28px; height: 32px; min-height: 0; color: var(--text-muted); font-size: var(--text-base); }
  .srv-del:hover { color: var(--error); }
  .srv-add { height: 32px; padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--accent); font-size: var(--text-sm); }
  .costs-btn { height: 34px; padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--text-secondary); font-size: var(--text-sm); border-radius: var(--radius-md); }
  .costs-btn:hover { background: var(--bg-hover); color: var(--accent); }
  .logout-btn { height: 34px; padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--text-muted); font-size: var(--text-sm); border-radius: var(--radius-md); }
  .logout-btn:hover { background: var(--bg-hover); color: var(--error); }

  /* ── Menu de contexto ── */
  .menu-backdrop { position: fixed; inset: 0; z-index: 40; }
  .ctx-menu {
    position: fixed; z-index: 41; min-width: 168px; padding: 4px;
    display: flex; flex-direction: column;
    background: var(--bg-elevated); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); box-shadow: 0 8px 28px rgba(0,0,0,0.4);
  }
  .ctx-menu button {
    height: 32px; padding: 0 10px; text-align: left; justify-content: flex-start;
    color: var(--text-primary); font-size: var(--text-sm); border-radius: var(--radius-sm);
  }
  .ctx-menu button:hover { background: var(--bg-hover); }
  .ctx-menu button.danger { color: var(--error); }
  .ctx-menu button.danger:hover { background: rgba(255,69,58,0.12); }
  .ctx-sep { height: 1px; margin: 4px 6px; background: var(--border-subtle); }
  /* Item que abre submenu: chevron a direita. */
  .ctx-more { margin-left: auto; color: var(--text-muted); padding-left: var(--space-3); }
  .ctx-back { color: var(--text-secondary); font-weight: 600; }
  .ctx-info { padding: 6px 10px; font-size: var(--text-sm); color: var(--text-muted); }
  /* Lista de branches rolavel (repo com muitas branches nao estoura a tela). */
  .ctx-scroll { max-height: 260px; overflow-y: auto; display: flex; flex-direction: column; }
  .ctx-branch { font-family: var(--font-mono); font-size: var(--text-xs); }
  .ctx-branch.current { color: var(--accent); }
  .ctx-cur { margin-left: auto; padding-left: var(--space-2); }
  /* Feature #12: form do encadeamento (alvo escolhido acima na lista + texto do prompt). */
  .ctx-chain-form { display: flex; gap: 4px; padding: 4px 6px; }
  .ctx-chain-input {
    flex: 1; min-width: 0; height: 28px; padding: 0 8px; font-size: var(--text-sm);
    color: var(--text-primary); background: var(--bg-base); border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
  }
  .ctx-chain-save {
    height: 28px; padding: 0 10px; font-size: var(--text-sm); font-weight: 600;
    color: var(--accent); background: var(--accent-dim); border-radius: var(--radius-sm);
  }
  .ctx-chain-save:disabled { opacity: 0.5; }

  /* ── Confirmar exclusao (modal centrado) ── */
  .confirm-backdrop { position: fixed; inset: 0; z-index: 50; background: rgba(0, 0, 0, 0.5); }
  .confirm-card {
    position: fixed; z-index: 51; top: 50%; left: 50%; transform: translate(-50%, -50%);
    width: min(340px, 90vw); padding: var(--space-5);
    display: flex; flex-direction: column; gap: var(--space-2);
    background: var(--bg-elevated); border: 1px solid var(--border-default);
    border-radius: var(--radius-lg); box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
    animation: confirm-in 160ms var(--ease-out) both;
  }
  @keyframes confirm-in {
    from { opacity: 0; transform: translate(-50%, -48%) scale(0.97); }
    to   { opacity: 1; transform: translate(-50%, -50%) scale(1); }
  }
  .confirm-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .confirm-hint { font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.5; }
  .confirm-name {
    font-family: var(--font-mono); font-size: var(--text-sm); color: var(--text-secondary);
    padding: var(--space-2) var(--space-3); background: var(--bg-base);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    word-break: break-all;
  }
  .confirm-actions { display: flex; gap: var(--space-2); margin-top: var(--space-2); }
  .c-btn {
    flex: 1; height: 40px; border-radius: var(--radius-md);
    font-size: var(--text-sm); font-weight: 600;
    background: var(--bg-hover); color: var(--text-secondary);
  }
  .c-btn:hover { background: var(--bg-surface); }
  .c-danger { background: var(--error); color: #fff; }
  .c-danger:hover { background: var(--error); filter: brightness(1.08); }

  /* Banner efemero (resultado do git pull / erro do editor). */
  .menu-toast {
    position: fixed; z-index: 42; left: 50%; bottom: 20px; transform: translateX(-50%);
    max-width: min(520px, 90vw); padding: 8px 14px;
    background: var(--bg-elevated); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); box-shadow: 0 6px 20px rgba(0,0,0,0.35);
    color: var(--text-primary); font-size: var(--text-sm); font-family: var(--font-mono);
    white-space: pre-wrap; word-break: break-word; max-height: 40vh; overflow-y: auto;
  }
  /* Ação primária do confirm (guardar e trocar): destaque com o accent. */
  .c-primary { background: var(--accent); border-color: var(--accent); color: var(--bg-base); font-weight: 600; }
</style>
