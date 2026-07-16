<script lang="ts">
  import { onMount } from 'svelte';
  import SessionCard from '../components/SessionCard.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
  import QrScanner from '../components/QrScanner.svelte';
  import BottomSheet from '../components/BottomSheet.svelte';
  import ConfirmSheet from '../components/ConfirmSheet.svelte';
  import GitSheet from '../components/GitSheet.svelte';
  import AttentionFeed from '../components/AttentionFeed.svelte';
  import AccountMenu from '../components/AccountMenu.svelte';
  import SessionSwitcherSheet from '../components/SessionSwitcherSheet.svelte';
  import { getSessions, createSession, deleteSession, renameSession, resumeSession, openSessionsStream, broadcast } from '../lib/api';
  import { clearCredentials, listServers, getActiveId, selectServer, removeServer, addServer, renameServer, serverColor } from '../lib/auth';
  import type { Server } from '../lib/auth';
  import type { AggSession, SessionInfo, ResumeCandidate } from '../lib/types';
  import { countAwaiting, groupSelectedByServer, initials, projectKey, projectLabel, sortSessions, clusterByPair } from '../lib/format';
  import { updateBadge } from '../lib/badge';

  interface Props {
    onNavigateToChat: (name: string) => void;
    onCompare: (ids: { serverId: string; name: string }[]) => void;
    onLogout: () => void;
  }
  let { onNavigateToChat, onCompare, onLogout }: Props = $props();

  // Visão agregada: sessões de TODOS os servidores numa lista só, cada uma marcada com a origem.
  let sessions = $state<AggSession[]>([]);
  let serverErrors = $state<{ label: string; error: string }[]>([]);
  let loading = $state(true);
  let error = $state('');
  let showCreateSheet = $state(false);
  let drawerOpen = $state(false);    // menu lateral (hamburger): navegação + conta
  let searchOpen = $state(false);    // "Buscar conversas" (switcher em modo só-busca)
  let filterText = $state('');

  // Lista de servidores (gerenciada no menu de conta: adicionar/remover). Sem "ativo" fixo — a lista é
  // agregada; o servidor-alvo de uma sessão é o dela, escolhido ao abrir/criar.
  let servers = $state(listServers());
  let scanning = $state(false);

  // Renomear servidor: o AccountMenu cuida da UI inline; aqui só persistimos e reagregamos pra os
  // badges das sessões pegarem o nome novo.
  function onRenameServer(id: string, label: string) {
    renameServer(id, label);
    servers = listServers();
    recompute();
  }
  // Reconectar (item "Reconectar" do menu de conta): fecha e reabre os SSE de todos os servidores.
  function reconnectStreams() {
    for (const es of streams.values()) es.close();
    streams.clear();
    connect(servers);
  }
  const multiServer = $derived(servers.length > 1);

  // Conta (avatar do rodapé): lista agregada, sem servidor "ativo" — usa o rótulo do 1º servidor como
  // nome e a contagem como subtítulo. Iniciais reusam o helper compartilhado (format).
  const accountName = $derived(servers[0]?.label ?? 'conta');
  const accountSub = $derived(`${servers.length} servidor${servers.length === 1 ? '' : 'es'}`);
  const accountInitials = $derived(initials(accountName));

  // Toggle do modo seleção/broadcast (feature #9), agora no botão do header (antes vivia no menu "…").

  // Adicionar servidor manual (no PC: digitar URL+token em vez de escanear QR).
  let showAddServer = $state(false);
  let addUrl = $state('');
  let addToken = $state('');
  let addError = $state('');
  let addBusy = $state(false);

  // Aguardando primeiro, depois alfabetico por nome (sortSessions compartilhado com a Sidebar — as
  // duas listas ja divergiram na ordenacao no passado). Estavel: so pula quando o ESTADO muda. Antes
  // ordenava por urgencia+atividade, e a atividade muda a todo poll -> a lista dancava.
  const visibleSessions = $derived.by(() => {
    const sorted = sortSessions(sessions);
    const q = filterText.trim().toLowerCase();
    if (!q) return sorted;
    return sorted.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        (s.cwd ?? '').toLowerCase().includes(q) ||
        s.serverLabel.toLowerCase().includes(q),
    );
  });

  // Filtro so aparece quando a lista fica longa.
  const showFilter = $derived(sessions.length > 6);

  // Quantas sessões precisam de você (aguardando) — alimenta o badge do ícone do app.
  const awaitingCount = $derived(countAwaiting(sessions));

  // Badge do ícone do app (feature #13): reflete o agregado de TODOS os servidores, sempre que a
  // lista mudar (novo snapshot SSE, servidor removido, etc). Zero aguardando -> limpa o badge.
  $effect(() => { updateBadge(awaitingCount); });

  // Toggle "Servidor | Projeto" (feature #3): alterna a CHAVE de agrupamento, persistido — igual
  // ao padrao de cp_collapsed_servers/cp_sidebar_w. "Servidor" = comportamento de sempre.
  const GROUP_BY_KEY = 'cp_group_by';
  type GroupBy = 'server' | 'project';
  function loadGroupBy(): GroupBy {
    return localStorage.getItem(GROUP_BY_KEY) === 'project' ? 'project' : 'server';
  }
  let groupBy = $state<GroupBy>(loadGroupBy());
  function setGroupBy(mode: GroupBy) {
    groupBy = mode;
    try { localStorage.setItem(GROUP_BY_KEY, mode); } catch { /* quota/priv mode: ignora */ }
  }

  // Agrupamento: por SERVIDOR (so multi-servidor, comportamento de sempre) ou por PROJETO (cwd da
  // sessao — feature #3, uma sessao qualquer servidor). Cada grupo = header colapsavel + contagem +
  // badge de aguardando. Ordem alfabetica (deterministica, nao pula). Reaproveita visibleSessions
  // (ja ordenado + filtrado).
  const grouped = $derived.by(() => {
    if (groupBy === 'project') {
      const byKey = new Map<string, AggSession[]>();
      for (const s of visibleSessions) {
        const key = projectKey(s.cwd);
        const arr = byKey.get(key);
        if (arr) arr.push(s);
        else byKey.set(key, [s]);
      }
      return [...byKey.entries()]
        .map(([key, list]) => ({ id: key, label: projectLabel(list[0]?.cwd), color: null as string | null, sessions: list }))
        .sort((a, b) => a.label.localeCompare(b.label));
    }
    const byId = new Map<string, AggSession[]>();
    for (const s of visibleSessions) {
      const arr = byId.get(s.serverId);
      if (arr) arr.push(s);
      else byId.set(s.serverId, [s]);
    }
    return servers
      .map((srv) => ({ id: srv.id, label: srv.label, color: serverColor(srv.id) as string | null, sessions: byId.get(srv.id) ?? [] }))
      .filter((g) => g.sessions.length > 0)
      .sort((a, b) => a.label.localeCompare(b.label)); // grupos fixos em ordem alfabetica (nao pulam)
  });

  // Mostra a UI agrupada quando ha mais de 1 servidor OU quando o modo e "projeto" (o ponto da
  // feature e ver "todas as sessoes do repo X" mesmo com 1 so servidor).
  const showGrouped = $derived(multiServer || groupBy === 'project');

  // ── Broadcast (feature #9): selecionar N sessoes e mandar 1 prompt pra todas ──────────────────
  // Selecao = chaves "<serverId>:<name>" (mesma composta usada nas keys #each abaixo). Cross-server:
  // groupSelectedByServer particiona por servidor-dono -> 1 chamada a broadcast() por servidor
  // (selectServer/restore, igual ao resto do app — ver withServer do Sidebar).
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
    drawerOpen = false;
  }
  function toggleSelected(key: string) {
    const next = new Set(selected);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    selected = next;
  }
  // "enviar p/ todas" no header do grupo: entra em modo selecao ja com o grupo inteiro marcado
  // (so as sessoes rastreaveis — "sem id" nao aceita input). Continua editavel (dá pra desmarcar).
  function selectGroupForBroadcast(g: { sessions: AggSession[] }) {
    selectMode = true;
    selected = new Set(
      g.sessions.filter((s) => s.tracked !== false).map((s) => `${s.serverId}:${s.name}`),
    );
  }
  // Slash-command manda por sessao (correcao de rota, nao broadcast) — desabilita o envio aqui em vez
  // de replicar "/comando" pra N sessoes de uma vez (ambiguo/perigoso, ex: /clear em todas sem querer).
  const broadcastIsSlash = $derived(broadcastText.trim().startsWith('/'));
  const broadcastDisabled = $derived(broadcastBusy || selected.size === 0 || !broadcastText.trim() || broadcastIsSlash);

  // "Comparar" (feature #11): reusa a MESMA seleção multipla do broadcast pra abrir a grade lado a
  // lado. Precisa de 2+ (comparar 1 sessão não tem propósito).
  const compareDisabled = $derived(selected.size < 2);
  function openCompare() {
    const ids = sessions
      .filter((s) => selected.has(`${s.serverId}:${s.name}`))
      .map((s) => ({ serverId: s.serverId, name: s.name }));
    onCompare(ids);
  }

  async function sendBroadcast() {
    const text = broadcastText.trim();
    if (broadcastDisabled) return;
    broadcastBusy = true;
    broadcastMsg = '';
    const groups = groupSelectedByServer(sessions, selected);
    const prevActive = getActiveId();
    const failed: string[] = [];
    for (const [serverId, names] of groups) {
      selectServer(serverId);
      try {
        const results = await broadcast(names, text);
        for (const [n, r] of Object.entries(results)) if (!r.ok) failed.push(n);
      } catch {
        failed.push(...names); // servidor offline/erro de rede -> conta todo o lote dele como falho
      }
    }
    if (prevActive) selectServer(prevActive);
    broadcastBusy = false;
    if (failed.length) {
      broadcastMsg = `falha: ${failed.join(', ')}`;
    } else {
      broadcastText = '';
      selected = new Set();
      selectMode = false;
    }
  }

  // Estado colapsado por servidor, persistido (sobrevive ao reload que add/scan de servidor dispara).
  const COLLAPSE_KEY = 'cp_collapsed_servers';
  function loadCollapsed(): Set<string> {
    try { return new Set(JSON.parse(localStorage.getItem(COLLAPSE_KEY) ?? '[]')); } catch { return new Set(); }
  }
  let collapsed = $state<Set<string>>(loadCollapsed());
  function toggleGroup(id: string) {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    collapsed = next;
    // Persiste só servidor/projeto; colapso de CLUSTER de pareamento ('pair:<gid>') é efêmero (gid
    // regenera a cada pareamento -> salvar acumularia lixo morto). Ver mesmo filtro na Sidebar.
    try { localStorage.setItem(COLLAPSE_KEY, JSON.stringify([...next].filter((k) => !k.startsWith('pair:')))); } catch { /* quota/priv mode: ignora */ }
  }

  // Slot por servidor; cada stream SSE preenche o seu e dispara recompute. Recompute reflatten na
  // ORDEM de `servers` (determinística). Servidor offline fica só em serverErrors, não derruba a lista.
  const slots = new Map<string, { sessions: SessionInfo[] | null; error: string | null }>();

  function recompute() {
    const agg: AggSession[] = [];
    const errs: { label: string; error: string }[] = [];
    // Dedupe: vários servidores podem apontar pro MESMO backend (URLs diferentes). A identidade
    // real da sessão é (jsonl, name) — o jsonl tem um uuid único por sessão, então sessões
    // distintas nunca colidem; só a mesma sessão vista por 2 URLs colapsa (fica a 1ª).
    const seen = new Set<string>();
    for (const srv of servers) {
      const slot = slots.get(srv.id);
      if (!slot) continue; // ainda não recebeu evento
      if (slot.sessions) {
        for (const s of slot.sessions) {
          const key = `${s.jsonl ?? s.cwd ?? ''}::${s.name}`;
          if (seen.has(key)) continue;
          seen.add(key);
          agg.push({ ...s, serverId: srv.id, serverLabel: srv.label, serverColor: serverColor(srv.id) });
        }
      } else if (slot.error) {
        errs.push({ label: srv.label, error: slot.error });
      }
    }
    sessions = agg;
    serverErrors = errs;
  }

  let streams = new Map<string, EventSource>(); // server.id → EventSource

  function connect(list: Server[]) {
    if (list.length === 0) { sessions = []; serverErrors = []; loading = false; return; }
    // fecha streams de servers removidos
    for (const [id, es] of streams) {
      if (!list.some((s) => s.id === id)) { es.close(); streams.delete(id); slots.delete(id); }
    }
    // abre streams novos (já conectado = pula)
    for (const s of list) {
      if (streams.has(s.id)) continue;
      const es = openSessionsStream(s);
      es.addEventListener('sessions', (e) => {
        slots.set(s.id, { sessions: JSON.parse((e as MessageEvent).data) as SessionInfo[], error: null });
        loading = false;
        recompute();
      });
      es.onerror = () => {
        // falha ISOLADA: só este servidor. EventSource auto-reconecta.
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        loading = false;
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

  // O sheet de criar já posicionou o servidor-alvo como ativo (selectServer), então createSession
  // cai no servidor certo. O stream SSE emitirá um evento sessions com a sessão nova.
  async function handleCreate(name: string, cwd?: string, configDir?: string | null, provider?: 'claude' | 'codex') {
    await createSession(name, cwd, configDir, provider);
  }

  // Abrir/apagar precisam mirar o servidor DA sessão: selectServer(serverId) antes, pois api.ts lê
  // o ativo a cada chamada (sem reload). Assim chat/SSE/delete vão pro backend certo.
  function openSession(s: AggSession) {
    if (s.tracked === false) return; // sem id confiavel: chat bloqueado (evita transcript errado)
    selectServer(s.serverId);
    onNavigateToChat(s.name);
  }

  // Excluir sessao pede confirmacao (com o nome + estado) — a superficie de toque no mobile e imprecisa,
  // e um toque acidental matava o tmux vivo na hora. O delete real so acontece no doDelete (paridade com
  // o desktop, que ja confirmava).
  let confirmDel = $state<AggSession | null>(null);
  function handleDelete(s: AggSession) {
    confirmDel = s;
  }
  async function doDelete() {
    if (!confirmDel) return;
    const s = confirmDel;
    confirmDel = null;
    selectServer(s.serverId);
    await deleteSession(s.name);
    sessions = sessions.filter((x) => !(x.serverId === s.serverId && x.name === s.name));
  }

  // Renomear sessao (toque longo no card): renomeia o pane tmux no servidor dela. O stream SSE re-emite
  // a sessao com o nome novo -> nao mexemos na lista aqui (igual ao Sidebar do desktop).
  async function handleRename(s: AggSession, newName: string) {
    selectServer(s.serverId);
    try {
      await renameSession(s.name, newName);
    } catch {
      /* falha -> o proximo poll do stream corrige o nome exibido */
    }
  }

  // Gerenciador git (GitSheet) aberto pelo botao git do card, no repo da sessao, SEM abrir o chat.
  // A GitSheet mira o server ATIVO (api.ts) -> aponto pro dono da sessao enquanto aberta e restauro
  // no fechar (mesmo padrao do Sidebar do desktop: menuGit/closeGitSheet).
  let gitSheet = $state<{ name: string } | null>(null);
  let gitSheetPrevServer: string | null = null;
  function handleGit(s: AggSession) {
    gitSheetPrevServer = getActiveId();
    selectServer(s.serverId);
    gitSheet = { name: s.name };
  }
  function closeGitSheet() {
    gitSheet = null;
    if (gitSheetPrevServer) { selectServer(gitSheetPrevServer); gitSheetPrevServer = null; }
  }

  // Retomar uma sessão "sem id": relança o pane com `claude --resume <uuid>` -> passa a rastrear. Caso
  // seguro (sessão sozinha no cwd) resolve direto; caso ambíguo (outras sessões no mesmo cwd) o backend
  // devolve candidatos e abrimos o sheet pra confirmar qual conversa retomar. O SSE de sessions atualiza
  // o card sozinho (vira tracked) — não mexemos na lista aqui.
  let resumeSheet = $state<{ session: AggSession; candidates: ResumeCandidate[] } | null>(null);
  let resumeBusy = $state('');   // nome da sessão em processamento (desabilita o botão/itens)
  let resumeError = $state('');

  async function handleResume(s: AggSession, sessionId?: string) {
    resumeError = '';
    resumeBusy = s.name;
    selectServer(s.serverId);
    try {
      const r = await resumeSession(s.name, sessionId);
      if (r && 'ambiguous' in r && r.ambiguous) {
        resumeSheet = { session: s, candidates: r.candidates };
      } else {
        resumeSheet = null;   // religada (caso seguro ou escolha confirmada)
      }
    } catch (e) {
      resumeError = e instanceof Error ? e.message : 'falha ao retomar';
    } finally {
      resumeBusy = '';
    }
  }

  // Formata a data da última atividade do candidato (epoch s) de forma curta e local.
  function fmtWhen(mtime?: number | null): string {
    if (!mtime) return '';
    return new Date(mtime * 1000).toLocaleString([], { dateStyle: 'short', timeStyle: 'short' });
  }

  function handleLogout() {
    clearCredentials();
    onLogout();
  }
  // Sair pelo botao pede confirmacao (recuperacao exige o token/QR de novo, e o token pode estar no PC).
  // O handleLogout cru continua sendo chamado direto quando o ultimo servidor e removido (ja confirmado la).
  let confirmLogout = $state(false);

  // Abre o drawer recarregando a lista de servidores (pode ter mudado desde a última abertura) — o
  // AccountMenu embedded vive lá dentro.
  function openDrawer() {
    servers = listServers();
    drawerOpen = true;
  }

  // Remover servidor pede confirmacao — o × de um toque removia na hora e, se fosse o unico
  // servidor, deslogava junto (com o token de pareamento la no PC). O remove real so acontece
  // no doDropServer. Sem "ativo" pra restaurar — fecha o stream e reagrega (ou desloga se zerou).
  let confirmSrv = $state<{ id: string; label: string } | null>(null);
  function dropServer(id: string) {
    const s = servers.find((x) => x.id === id);
    confirmSrv = { id, label: s?.label ?? id };
  }
  function doDropServer() {
    if (!confirmSrv) return;
    const id = confirmSrv.id;
    confirmSrv = null;
    removeServer(id);
    servers = listServers();
    if (servers.length === 0) { handleLogout(); return; }
    connect(servers);
  }

  // Abre o sheet de adicionar servidor manual (URL + token), limpando o estado anterior.
  function openAddServer() {
    addUrl = '';
    addToken = '';
    addError = '';
    drawerOpen = false;
    showAddServer = true;
  }

  // Adiciona um servidor digitado à mão. Valida com getSessions (api.ts lê o ativo) e faz rollback
  // em falha — igual ao Login — pra um servidor ruim não sujar a lista nem trocar o server bom.
  async function submitAddServer(e: SubmitEvent) {
    e.preventDefault();
    addBusy = true;
    addError = '';
    const prevActive = getActiveId();
    const { id, existed } = addServer(addUrl.trim(), addToken.trim());
    try {
      await getSessions();
      showAddServer = false;
      window.location.reload();
    } catch (err) {
      if (!existed) removeServer(id);
      if (prevActive) selectServer(prevActive);
      addError = err instanceof Error ? `Falha na conexão: ${err.message}` : 'Erro desconhecido';
    } finally {
      addBusy = false;
    }
  }

  // Adiciona um servidor pelo QR (parecido com o Login): pega token + origem absoluta e ativa.
  function handleScanServer(text: string) {
    let tok = text.trim();
    let base = '';
    try {
      const u = new URL(text);
      const t = u.searchParams.get('token');
      if (t) tok = t;
      base = u.searchParams.get('api') ?? u.origin;
    } catch {
      base = ''; // token cru sem URL -> sem origem confiável; ignora
    }
    scanning = false;
    if (!tok || !base) return;
    addServer(base, tok);
    window.location.reload();
  }
</script>

<div class="session-list-screen">
  <!-- Cabeçalho: hamburger (abre o drawer: navegação + conta) + marca + seleção/broadcast. -->
  <header class="sl-head">
    <button class="sl-ham" onclick={openDrawer} aria-label="Abrir menu">
      <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true">
        <path d="M4 7h16M4 12h13M4 17h16"/>
      </svg>
    </button>
    <span class="sl-brand">claude pocket</span>
    <button
      class="sl-icon-btn"
      class:active={selectMode}
      onclick={toggleSelectMode}
      aria-label={selectMode ? 'Cancelar seleção' : 'Selecionar sessões'}
      title={selectMode ? 'Cancelar seleção' : 'Selecionar / broadcast'}
    >
      <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M9 11l3 3L22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/>
      </svg>
    </button>
  </header>

  <div class="list-content" class:select-mode={selectMode}>
    <!-- "Precisa de você" (feature #6): fila fixa no topo com as sessoes AGUARDANDO de TODOS os
         servidores. Responder aqui (picker inline) nao abre o chat; nativo AskUserQuestion abre. -->
    <AttentionFeed {sessions} onOpenChat={openSession} />
    {#if sessions.length > 0 && multiServer}
      <!-- Toggle Servidor|Projeto (feature #3): so aparece com >=2 servidores (paridade com o desktop
           via effectiveGroupBy) — com 1 servidor "por servidor" seria 1 grupo so, entao o toggle vira
           ruido e some. -->
      <div class="group-toggle" role="radiogroup" aria-label="Agrupar por">
        <button type="button" class:active={groupBy === 'server'} role="radio" aria-checked={groupBy === 'server'} onclick={() => setGroupBy('server')}>Servidor</button>
        <button type="button" class:active={groupBy === 'project'} role="radio" aria-checked={groupBy === 'project'} onclick={() => setGroupBy('project')}>Projeto</button>
      </div>
    {/if}
    {#if serverErrors.length > 0}
      <div class="server-warn" role="status">
        {#each serverErrors as e (e.label)}
          <span class="server-warn-item">⚠ {e.label} offline</span>
        {/each}
      </div>
    {/if}
    {#if loading && sessions.length === 0}
      <div class="empty-state">
        <div class="spinner-large" aria-label="Carregando…">⟳</div>
        <p>Carregando sessões…</p>
      </div>
    {:else if error}
      <div class="empty-state">
        <p class="error-text">{error}</p>
        <button class="retry-btn" onclick={() => { for (const es of streams.values()) es.close(); streams.clear(); connect(servers); }}>Tentar novamente</button>
      </div>
    {:else if sessions.length === 0}
      <div class="empty-state">
        <p class="empty-title">Nenhuma sessão ativa</p>
        <p class="empty-sub">Toque em + para criar</p>
      </div>
    {:else}
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
      {#if visibleSessions.length === 0}
        <p class="filter-empty">Nenhuma sessão corresponde ao filtro.</p>
      {:else}
        {#if showGrouped}
          {#each grouped as g (g.id)}
            {@const awaiting = countAwaiting(g.sessions)}
            <div class="group">
              <div class="group-head-row">
                <button
                  class="group-head"
                  onclick={() => toggleGroup(g.id)}
                  aria-expanded={!collapsed.has(g.id)}
                  aria-label={`${g.label}: ${g.sessions.length} ${g.sessions.length === 1 ? 'sessão' : 'sessões'}`}
                >
                  <span class="group-chevron" class:collapsed={collapsed.has(g.id)} aria-hidden="true">▾</span>
                  {#if g.color}<span class="group-dot" style="background: {g.color};" aria-hidden="true"></span>{/if}
                  <span class="group-label">{g.label}</span>
                  <span class="group-count">{g.sessions.length}</span>
                  {#if awaiting > 0}
                    <span class="group-await">{awaiting} aguardando</span>
                  {/if}
                </button>
                <!-- "enviar p/ todas" (feature #9): entra em modo seleção com o grupo inteiro marcado. -->
                <button
                  class="group-broadcast"
                  onclick={() => selectGroupForBroadcast(g)}
                  aria-label={`Enviar mensagem para todas as sessões de ${g.label}`}
                  title="Enviar p/ todas"
                >➤</button>
              </div>
              {#if !collapsed.has(g.id)}
                {#each clusterByPair(g.sessions) as item (item.kind === 'header' ? `ph:${item.gid}` : `${item.session.serverId}:${item.session.name}`)}
                  {#if item.kind === 'header'}
                    <!-- Cluster de pareamento (Opção C): sub-header colapsável do grupo. -->
                    <button class="pair-head" onclick={() => toggleGroup(`pair:${item.gid}`)}
                            aria-expanded={!collapsed.has(`pair:${item.gid}`)}>
                      <span class="pair-chev" class:collapsed={collapsed.has(`pair:${item.gid}`)} aria-hidden="true">▾</span>
                      <span class="pair-label">🤝&nbsp;{item.label}</span>
                      <span class="pair-count">{item.count}</span>
                    </button>
                  {:else if !item.gid || !collapsed.has(`pair:${item.gid}`)}
                    {@const session = item.session}
                    <div class="pair-wrap" class:pair-member={!!item.gid}>
                      <SessionCard
                        {session}
                        serverBadge={null}
                        onClick={() => openSession(session)}
                        onDelete={() => handleDelete(session)}
                        onResume={() => handleResume(session)}
                        onRename={(nv) => handleRename(session, nv)}
                        onGit={() => handleGit(session)}
                        {selectMode}
                        selected={selected.has(`${session.serverId}:${session.name}`)}
                        onToggleSelect={() => toggleSelected(`${session.serverId}:${session.name}`)}
                      />
                    </div>
                  {/if}
                {/each}
              {/if}
            </div>
          {/each}
        {:else}
          {#each visibleSessions as session (session.serverId + ':' + session.name)}
            <SessionCard
              {session}
              serverBadge={null}
              onClick={() => openSession(session)}
              onDelete={() => handleDelete(session)}
              onResume={() => handleResume(session)}
              onRename={(nv) => handleRename(session, nv)}
              onGit={() => handleGit(session)}
              {selectMode}
              selected={selected.has(`${session.serverId}:${session.name}`)}
              onToggleSelect={() => toggleSelected(`${session.serverId}:${session.name}`)}
            />
          {/each}
        {/if}
      {/if}
    {/if}
  </div>

  {#if selectMode}
    <!-- Composer compacto do broadcast (feature #9): so texto + enviar, sem anexos/slash-UI (isso
         fica no Composer normal, por sessão). Slash-command desabilita o envio (rota por sessão). -->
    <div class="broadcast-bar">
      <div class="broadcast-row">
        <button class="broadcast-cancel" onclick={toggleSelectMode} aria-label="Cancelar seleção">×</button>
        <span class="broadcast-count">{selected.size} selecionada{selected.size === 1 ? '' : 's'}</span>
        <button class="broadcast-compare" onclick={openCompare} disabled={compareDisabled} aria-label="Comparar sessões selecionadas" title="Comparar">Comparar</button>
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
  {:else}
    <!-- Rodapé: só o CTA "Nova sessão" (a conta migrou pro drawer do hamburger). -->
    <footer class="sl-foot">
      <button class="cta-new" onclick={() => (showCreateSheet = true)} aria-label="Nova sessão">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
        Nova sessão
      </button>
    </footer>
  {/if}

  <!-- Drawer (hamburger, estilo Claude): navegação + conta. Desliza da esquerda; backdrop fecha. Fica
       sempre montado (só o transform anima) pra a transição rodar nos dois sentidos. -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    class="drawer-backdrop"
    class:open={drawerOpen}
    onclick={() => (drawerOpen = false)}
    role="button"
    tabindex="-1"
    aria-label="Fechar menu"
  ></div>
  <aside class="drawer" class:open={drawerOpen} aria-hidden={!drawerOpen}>
    <div class="drawer-acct">
      <span class="drawer-avatar" aria-hidden="true">{accountInitials}</span>
      <span class="drawer-who">
        <span class="drawer-name">{accountName}</span>
        <span class="drawer-sub">{accountSub}</span>
      </span>
      <button class="drawer-close" onclick={() => (drawerOpen = false)} aria-label="Fechar menu">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"/></svg>
      </button>
    </div>
    <div class="drawer-sep"></div>
    <nav class="drawer-nav" aria-label="Navegação">
      <button class="drawer-nav-item on" aria-current="page" onclick={() => (drawerOpen = false)}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
        Sessões
      </button>
      <button class="drawer-nav-item" onclick={() => { drawerOpen = false; searchOpen = true; }}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="m21 21-4.3-4.3"/></svg>
        Buscar conversas
      </button>
      <button class="drawer-nav-item" onclick={() => { drawerOpen = false; window.location.hash = '#/archive'; }}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="4" rx="1"/><path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8"/><path d="M10 12h4"/></svg>
        Arquivo
      </button>
      <button class="drawer-nav-item" onclick={() => { drawerOpen = false; window.location.hash = '#/costs'; }}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 3v18h18"/><path d="M7 14l4-4 3 3 5-6"/></svg>
        Custos
      </button>
    </nav>
    <div class="drawer-sep"></div>
    <div class="drawer-scroll">
      <AccountMenu
        embedded
        open={drawerOpen}
        onClose={() => (drawerOpen = false)}
        initials={accountInitials}
        {accountName}
        {accountSub}
        {servers}
        {onRenameServer}
        onRemoveServer={dropServer}
        onAddServer={openAddServer}
        onReconnect={reconnectStreams}
        onLogout={() => (confirmLogout = true)}
      />
    </div>
  </aside>

  <!-- "Buscar conversas" (nav): switcher em modo só-busca (busca de conteúdo cross-servidor, feature #10). -->
  <SessionSwitcherSheet
    open={searchOpen}
    searchOnly
    sessions={[]}
    currentName=""
    onPick={(name) => { searchOpen = false; onNavigateToChat(name); }}
    onNew={() => { searchOpen = false; showCreateSheet = true; }}
    onClose={() => (searchOpen = false)}
  />

  <CreateSessionSheet
    open={showCreateSheet}
    {servers}
    onClose={() => (showCreateSheet = false)}
    onCreate={handleCreate}
    onOpenSession={onNavigateToChat}
  />

  <!-- Caso ambíguo do resume: várias sessões no mesmo cwd -> o usuário confirma QUAL conversa retomar. -->
  <BottomSheet open={resumeSheet !== null} onClose={() => (resumeSheet = null)} ariaLabel="Retomar conversa">
    {#if resumeSheet}
      {@const sheet = resumeSheet}
      <div class="resume-sheet">
        <h2 class="resume-title">Retomar qual conversa?</h2>
        <p class="resume-sub">
          Há mais de uma sessão nesta pasta — escolha o transcript pra continuar em <strong>{sheet.session.name}</strong>.
        </p>
        {#if resumeError}<p class="resume-err">{resumeError}</p>{/if}
        <ul class="resume-list">
          {#each sheet.candidates as c (c.session_id)}
            <li>
              <button
                class="resume-item"
                disabled={c.in_use || resumeBusy === sheet.session.name}
                onclick={() => handleResume(sheet.session, c.session_id)}
              >
                <span class="resume-item-preview">{c.preview || '(sem prévia)'}</span>
                <span class="resume-item-meta">
                  {fmtWhen(c.mtime)}{#if c.in_use} · em uso por outra sessão{/if}
                </span>
              </button>
            </li>
          {/each}
        </ul>
      </div>
    {/if}
  </BottomSheet>

  {#if showAddServer}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      class="sheet-backdrop"
      role="button"
      tabindex="-1"
      aria-label="Fechar"
      onclick={() => (showAddServer = false)}
    >
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="add-sheet" role="dialog" tabindex="-1" aria-label="Adicionar servidor" onclick={(e) => e.stopPropagation()}>
        <h2 class="add-title">Adicionar servidor</h2>
        <form onsubmit={submitAddServer} class="add-form">
          <div class="field">
            <label class="field-label" for="add-url">URL do servidor</label>
            <input
              id="add-url"
              type="url"
              class="field-input"
              bind:value={addUrl}
              placeholder="https://meu-pc.ts.net"
              autocomplete="url"
              autocorrect="off"
              autocapitalize="off"
              spellcheck={false}
              inputmode="url"
            />
          </div>
          <div class="field">
            <label class="field-label" for="add-token">Token</label>
            <input
              id="add-token"
              type="password"
              class="field-input"
              bind:value={addToken}
              placeholder="••••••••••••••••"
              autocomplete="current-password"
            />
          </div>
          {#if addError}
            <p class="error-msg" role="alert">{addError}</p>
          {/if}
          <button type="submit" class="add-primary" disabled={addBusy || !addUrl.trim() || !addToken.trim()}>
            {addBusy ? 'Conectando…' : 'Adicionar'}
          </button>
          <button type="button" class="add-secondary" onclick={() => { showAddServer = false; scanning = true; }}>
            Escanear QR
          </button>
        </form>
      </div>
    </div>
  {/if}

  {#if scanning}
    <QrScanner onScan={handleScanServer} onClose={() => (scanning = false)} />
  {/if}

  <ConfirmSheet
    open={confirmDel !== null}
    title="Excluir esta sessão?"
    message={confirmDel
      ? confirmDel.state === 'working'
        ? `${confirmDel.name} está em execução — excluir encerra o processo do tmux e perde o que estiver rodando.`
        : confirmDel.name
      : null}
    confirmLabel="Excluir"
    danger
    onConfirm={doDelete}
    onClose={() => (confirmDel = null)}
  />

  <ConfirmSheet
    open={confirmLogout}
    title="Sair do app?"
    message="Você vai precisar do token (QR ou digitado) pra entrar de novo — e ele pode estar no PC."
    confirmLabel="Sair"
    danger
    onConfirm={() => { confirmLogout = false; handleLogout(); }}
    onClose={() => (confirmLogout = false)}
  />

  <ConfirmSheet
    open={confirmSrv !== null}
    title="Remover este servidor?"
    message={confirmSrv
      ? servers.length === 1
        ? `${confirmSrv.label} é o único servidor — remover desconecta o app e o pareamento precisa ser refeito (QR ou token no PC).`
        : confirmSrv.label
      : null}
    confirmLabel="Remover"
    danger
    onConfirm={doDropServer}
    onClose={() => (confirmSrv = null)}
  />

  <!-- Gerenciador git aberto pelo botao git do card (repo da sessao, sem abrir o chat). -->
  {#if gitSheet}
    <GitSheet open={true} sessionName={gitSheet.name} onClose={closeGitSheet} />
  {/if}
</div>

<style>
  /* Cluster de pareamento (Opção C): sub-header colapsável + faixa accent ligando os membros. */
  .pair-head {
    display: flex; align-items: center; gap: var(--space-2);
    width: 100%; text-align: left;
    padding: var(--space-2) var(--space-4);
    background: none; border: none; cursor: pointer;
    font-size: var(--text-sm); font-weight: 600; color: var(--accent);
    -webkit-tap-highlight-color: transparent;
  }
  .pair-chev { flex-shrink: 0; font-size: 10px; transition: transform 160ms var(--ease-out); }
  .pair-chev.collapsed { transform: rotate(-90deg); }
  .pair-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; }
  .pair-count {
    flex-shrink: 0; font-size: var(--text-xs); color: var(--accent);
    background: var(--accent-dim); border-radius: var(--radius-full); padding: 1px 8px;
  }
  .pair-wrap.pair-member { border-left: 2px solid var(--accent-dim); }

  .session-list-screen {
    display: flex;
    flex-direction: column;
    height: 100%;
    position: relative;
    overflow: hidden;
  }

  /* ── Cabeçalho (hamburger + marca + seleção/broadcast) ── */
  .sl-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-shrink: 0;
    padding: calc(env(safe-area-inset-top) + var(--space-3)) var(--space-4) var(--space-2);
  }
  .sl-ham {
    width: 40px;
    height: 40px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    color: var(--text-primary);
    border-radius: var(--radius-full);
    transition: background 150ms var(--ease-out);
  }
  .sl-ham:active { background: var(--bg-hover); }
  .sl-brand {
    flex: 1;
    min-width: 0;
    font-size: var(--text-lg);
    font-weight: 700;
    color: var(--text-primary);
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .sl-icon-btn {
    width: 40px;
    height: 40px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    color: var(--text-secondary);
    border-radius: var(--radius-full);
    transition: background 150ms var(--ease-out), color 150ms var(--ease-out);
  }
  .sl-icon-btn:active { background: var(--bg-hover); color: var(--text-primary); }
  .sl-icon-btn.active { color: var(--accent); background: var(--accent-dim); }

  .list-content {
    flex: 1;
    overflow-y: scroll;
    -webkit-overflow-scrolling: touch;
    overscroll-behavior-y: contain;
    padding: var(--space-2) 0 var(--space-4);
  }
  /* Broadcast-bar (feature #9) é fixa embaixo -> reserva espaço pra não cobrir a última linha. */
  .list-content.select-mode {
    padding-bottom: calc(env(safe-area-inset-bottom) + 160px);
  }

  /* ── Rodapé: só o CTA "Nova sessão" (a conta migrou pro drawer do hamburger). ── */
  .sl-foot {
    display: flex;
    flex-shrink: 0;
    padding: var(--space-3) var(--space-4) calc(env(safe-area-inset-bottom) + var(--space-3));
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-base);
  }
  .cta-new {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-2);
    flex: 1;
    height: 48px;
    background: var(--accent);
    color: #fff;
    font-size: var(--text-base);
    font-weight: 600;
    border-radius: var(--radius-full);
    white-space: nowrap;
    transition: background 150ms var(--ease-out), transform 80ms ease-in-out;
  }
  .cta-new svg { flex-shrink: 0; }
  .cta-new:active { background: var(--accent-press); transform: scale(0.98); }

  /* ── Drawer (hamburger): navegação + conta. ── */
  .drawer-backdrop {
    position: fixed;
    inset: 0;
    z-index: 40;
    background: rgba(0, 0, 0, 0.55);
    opacity: 0;
    pointer-events: none;
    transition: opacity 280ms var(--ease-out);
  }
  .drawer-backdrop.open { opacity: 1; pointer-events: auto; }
  .drawer {
    position: fixed;
    top: 0;
    left: 0;
    bottom: 0;
    z-index: 41;
    width: min(310px, 86vw);
    display: flex;
    flex-direction: column;
    background: var(--bg-base);
    border-right: 1px solid var(--border-default);
    box-shadow: 10px 0 44px rgba(0, 0, 0, 0.5);
    transform: translateX(-102%);
    transition: transform 300ms cubic-bezier(0.32, 0.72, 0, 1);
  }
  .drawer.open { transform: translateX(0); }
  .drawer-acct {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: calc(env(safe-area-inset-top) + var(--space-3)) var(--space-3) var(--space-3);
  }
  .drawer-avatar {
    width: 36px;
    height: 36px;
    flex-shrink: 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: linear-gradient(135deg, var(--accent), #a06de0);
    color: #fff;
    font-size: var(--text-sm);
    font-weight: 700;
  }
  .drawer-who { min-width: 0; flex: 1; display: flex; flex-direction: column; }
  .drawer-name { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .drawer-sub { font-size: var(--text-xs); color: var(--text-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .drawer-close {
    width: 34px;
    height: 34px;
    flex-shrink: 0;
    display: grid;
    place-items: center;
    color: var(--text-secondary);
    border-radius: var(--radius-full);
  }
  .drawer-close:active { background: var(--bg-hover); }
  .drawer-sep { height: 1px; background: var(--border-subtle); margin: 0 var(--space-3); }
  .drawer-nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2);
  }
  .drawer-nav-item {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    min-height: 44px;
    padding: var(--space-2) var(--space-3);
    justify-content: flex-start;
    text-align: left;
    color: var(--text-secondary);
    font-size: var(--text-base);
    font-weight: 500;
    border-radius: var(--radius-md);
    transition: background 150ms var(--ease-out), color 150ms var(--ease-out);
  }
  .drawer-nav-item svg { flex-shrink: 0; }
  .drawer-nav-item:active { background: var(--bg-hover); color: var(--text-primary); }
  .drawer-nav-item.on { background: var(--accent-dim); color: var(--text-primary); font-weight: 600; }
  .drawer-scroll {
    flex: 1;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .server-warn {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin: 0 var(--space-4) var(--space-3);
  }
  .server-warn-item {
    font-size: var(--text-xs);
    color: var(--warning);
    background: rgba(224, 162, 59, 0.1);
    border: 1px solid rgba(224, 162, 59, 0.25);
    border-radius: var(--radius-full);
    padding: 3px 10px;
  }

  .filter-input {
    display: block;
    width: auto;
    height: 44px;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    padding: 0 var(--space-3);
    outline: none;
    margin: 0 var(--space-4) var(--space-3);
    transition: border-color 180ms var(--ease-out);
  }

  .filter-input::placeholder {
    color: var(--text-muted);
  }

  .filter-input:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  /* Toggle Servidor|Projeto (feature #3): slim/inline (NÃO full-width) — pill discreto alinhado à
     esquerda, mesma linguagem do toggle do Sidebar desktop. */
  .group-toggle {
    display: inline-flex;
    gap: 2px;
    margin: 0 var(--space-4) var(--space-3);
    padding: 2px;
    height: 28px;
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-full);
  }
  .group-toggle button {
    min-height: 0;
    height: 24px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-full);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-secondary);
  }
  .group-toggle button.active {
    background: var(--bg-elevated);
    color: var(--text-primary);
  }

  .filter-empty {
    font-size: var(--text-sm);
    color: var(--text-muted);
    text-align: center;
    padding: var(--space-6) var(--space-3);
  }

  /* Grupo por servidor: header colapsavel (mobile + desktop, mesmo componente). */
  .group { margin-bottom: var(--space-1); }
  .group-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    padding: var(--space-2) var(--space-4);
    background: transparent;
    text-align: left;
    -webkit-tap-highlight-color: transparent;
  }
  .group-head:active { background: var(--bg-hover); }
  .group-chevron {
    font-size: var(--text-xs);
    color: var(--text-muted);
    transition: transform 160ms var(--ease-out);
  }
  .group-chevron.collapsed { transform: rotate(-90deg); }
  .group-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  .group-label {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
  }
  .group-count {
    font-size: var(--text-xs);
    color: var(--text-muted);
    background: var(--bg-surface);
    border-radius: var(--radius-full);
    padding: 1px 8px;
    min-width: 20px;
    text-align: center;
  }
  .group-await {
    margin-left: auto;
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--warning);
  }

  /* Header do grupo virou uma row (toggle colapsar + "enviar p/ todas", feature #9). */
  .group-head-row { display: flex; align-items: center; }
  .group-head-row .group-head { flex: 1; min-width: 0; }
  .group-broadcast {
    flex-shrink: 0;
    width: 36px;
    height: 36px;
    margin-right: var(--space-2);
    color: var(--text-muted);
    font-size: var(--text-sm);
    border-radius: var(--radius-sm);
  }
  .group-broadcast:active { color: var(--accent); background: var(--bg-hover); }

  /* Composer compacto do broadcast: texto + enviar, no lugar do FAB enquanto seleciona (feature #9). */
  .broadcast-bar {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 20;
    background: var(--bg-elevated);
    border-top: 1px solid var(--border-default);
    padding: var(--space-3) var(--space-4) calc(env(safe-area-inset-bottom) + var(--space-3));
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .broadcast-row { display: flex; align-items: center; gap: var(--space-3); }
  .broadcast-cancel {
    width: 32px; height: 32px; flex-shrink: 0;
    color: var(--text-secondary); font-size: var(--text-lg); line-height: 1;
    border-radius: var(--radius-sm);
  }
  .broadcast-cancel:active { background: var(--bg-hover); }
  .broadcast-count { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .broadcast-compare {
    margin-left: auto;
    font-size: var(--text-xs); font-weight: 600; color: var(--accent);
    padding: 4px 10px; border: 1px solid var(--accent); border-radius: var(--radius-full);
    background: transparent;
  }
  .broadcast-compare:disabled { color: var(--text-muted); border-color: var(--border-default); }
  .broadcast-compare:active:not(:disabled) { background: var(--accent-dim); }
  .broadcast-msg { font-size: var(--text-xs); color: var(--warning); margin: 0; }
  .broadcast-hint { font-size: var(--text-xs); color: var(--text-muted); margin: 0; }
  .broadcast-input-row { display: flex; gap: var(--space-2); }
  .broadcast-input {
    flex: 1;
    height: 44px;
    background: var(--bg-base);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    padding: 0 var(--space-3);
    outline: none;
  }
  .broadcast-input:focus { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-dim); }
  .broadcast-send {
    width: 44px; height: 44px; flex-shrink: 0;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    font-size: var(--text-base);
  }
  .broadcast-send:disabled { background: var(--bg-hover); color: var(--text-muted); }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    padding-top: 80px;
  }

  .empty-title {
    font-size: var(--text-lg);
    color: var(--text-secondary);
    font-weight: 500;
  }

  .empty-sub {
    font-size: var(--text-sm);
    color: var(--text-muted);
  }

  .error-text {
    font-size: var(--text-sm);
    color: var(--error);
    text-align: center;
  }

  .retry-btn {
    height: 44px;
    padding: 0 var(--space-5);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-secondary);
    font-size: var(--text-sm);
  }

  .spinner-large {
    font-size: 32px;
    color: var(--accent);
    animation: spin 0.8s linear infinite;
  }

  /* Sheet de adicionar servidor manual */
  .sheet-backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-6);
  }
  .add-sheet {
    width: 100%;
    max-width: 400px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    padding: var(--space-5);
  }
  .add-title {
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-4);
  }
  .add-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .field { display: flex; flex-direction: column; gap: var(--space-2); }
  .field-label { font-size: var(--text-sm); color: var(--text-secondary); font-weight: 500; }
  .field-input {
    height: 48px;
    background: var(--bg-base);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px;
    padding: 0 var(--space-4);
    outline: none;
    transition: border-color 180ms ease-out;
  }
  .field-input::placeholder { color: var(--text-muted); }
  .field-input:focus { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-dim); }
  .error-msg {
    font-size: var(--text-sm);
    color: var(--error);
    background: rgba(255, 69, 58, 0.08);
    border: 1px solid rgba(255, 69, 58, 0.2);
    border-radius: var(--radius-sm);
    padding: var(--space-3);
  }
  .add-primary {
    height: 52px;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    font-size: var(--text-base);
    font-weight: 600;
    width: 100%;
    transition: background 180ms ease-out;
  }
  .add-primary:active:not(:disabled) { background: var(--accent-press); }
  /* Disabled inerte (bg neutro + texto muted), nao indigo cheio a 50% que parece meio-clicavel. */
  .add-primary:disabled { background: var(--bg-hover); color: var(--text-muted); cursor: default; }
  .add-secondary {
    height: 48px;
    background: transparent;
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-secondary);
    font-size: var(--text-base);
    font-weight: 500;
    width: 100%;
    transition: background 180ms ease-out;
  }
  .add-secondary:active { background: var(--bg-hover); }

  /* Sheet de resume (caso ambíguo): lista de transcripts candidatos pra escolher. */
  .resume-sheet {
    padding: var(--space-4) var(--space-4) var(--space-6);
  }
  .resume-title {
    font-size: var(--text-lg);
    font-weight: 700;
    margin: 0 0 var(--space-1);
  }
  .resume-sub {
    font-size: var(--text-sm);
    color: var(--text-muted);
    margin: 0 0 var(--space-3);
  }
  .resume-err {
    font-size: var(--text-sm);
    color: var(--error);
    margin: 0 0 var(--space-2);
  }
  .resume-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .resume-item {
    width: 100%;
    text-align: left;
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: var(--space-3);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .resume-item:active {
    background: var(--bg-elevated, var(--bg-surface));
  }
  .resume-item:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .resume-item-preview {
    font-size: var(--text-sm);
    color: var(--text-base);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .resume-item-meta {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }
</style>
