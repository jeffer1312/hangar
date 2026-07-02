<script lang="ts">
  import { onMount } from 'svelte';
  import NavBar from '../components/NavBar.svelte';
  import SessionCard from '../components/SessionCard.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
  import QrScanner from '../components/QrScanner.svelte';
  import BottomSheet from '../components/BottomSheet.svelte';
  import ConfirmSheet from '../components/ConfirmSheet.svelte';
  import { getSessions, createSession, deleteSession, resumeSession, openSessionsStream } from '../lib/api';
  import { clearCredentials, listServers, getActiveId, selectServer, removeServer, addServer, renameServer, serverColor } from '../lib/auth';
  import type { Server } from '../lib/auth';
  import type { AggSession, SessionInfo, ResumeCandidate } from '../lib/types';
  import { enablePush, pushSupported } from '../lib/push';

  interface Props {
    onNavigateToChat: (name: string) => void;
    onLogout: () => void;
  }
  let { onNavigateToChat, onLogout }: Props = $props();

  // Visão agregada: sessões de TODOS os servidores numa lista só, cada uma marcada com a origem.
  let sessions = $state<AggSession[]>([]);
  let serverErrors = $state<{ label: string; error: string }[]>([]);
  let loading = $state(true);
  let error = $state('');
  let showCreateSheet = $state(false);
  let showMenu = $state(false);
  let filterText = $state('');

  // Lista de servidores (gerenciada no menu: adicionar/remover). Sem "ativo" fixo — a lista é
  // agregada; o servidor-alvo de uma sessão é o dela, escolhido ao abrir/criar.
  let servers = $state(listServers());
  let scanning = $state(false);

  // Web push: liga notificacao de "sessao aguardando" (assina + registra nos servidores).
  let pushBusy = $state(false);
  let pushMsg = $state('');
  async function handleEnablePush() {
    pushBusy = true;
    pushMsg = '';
    try {
      const n = await enablePush();
      pushMsg = `Ativado em ${n} servidor${n > 1 ? 'es' : ''}.`;
    } catch (e) {
      pushMsg = e instanceof Error ? e.message : 'Erro ao ativar.';
    } finally {
      pushBusy = false;
    }
  }

  // Rename inline de servidor no menu: id em edicao + valor do input.
  let editingId = $state<string | null>(null);
  let editLabel = $state('');

  function startRename(id: string, current: string) {
    editingId = id;
    editLabel = current;
  }
  function saveRename() {
    if (editingId) {
      renameServer(editingId, editLabel);
      servers = listServers();
      recompute(); // reagrega pra os badges das sessoes pegarem o nome novo
    }
    editingId = null;
  }
  const multiServer = $derived(servers.length > 1);

  // Adicionar servidor manual (no PC: digitar URL+token em vez de escanear QR).
  let showAddServer = $state(false);
  let addUrl = $state('');
  let addToken = $state('');
  let addError = $state('');
  let addBusy = $state(false);

  // Ordem ALFABETICA por nome (estavel — nao pula). Antes ordenava por urgencia+atividade, e a
  // atividade muda a todo poll -> a lista dancava. Alfabetico fixa a posicao de cada sessao.
  const visibleSessions = $derived.by(() => {
    const sorted = [...sessions].sort((a, b) => a.name.localeCompare(b.name));
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

  // Resumo do header: total + quantas precisam de voce (aguardando).
  const awaitingCount = $derived(sessions.filter((s) => s.state === 'awaiting_input').length);
  const summaryText = $derived(`${sessions.length} ${sessions.length === 1 ? 'sessão' : 'sessões'}`);

  // Agrupamento por servidor (so multi-servidor): cada grupo = um servidor, com header colapsavel +
  // contagem de sessoes abertas. Ordem dos grupos segue `servers` (deterministica, igual ao menu);
  // grupos sem sessao visivel somem. Reaproveita visibleSessions (ja ordenado + filtrado).
  const grouped = $derived.by(() => {
    const byId = new Map<string, AggSession[]>();
    for (const s of visibleSessions) {
      const arr = byId.get(s.serverId);
      if (arr) arr.push(s);
      else byId.set(s.serverId, [s]);
    }
    return servers
      .map((srv) => ({ id: srv.id, label: srv.label, color: serverColor(srv.id), sessions: byId.get(srv.id) ?? [] }))
      .filter((g) => g.sessions.length > 0)
      .sort((a, b) => a.label.localeCompare(b.label)); // grupos fixos em ordem alfabetica (nao pulam)
  });

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
    try { localStorage.setItem(COLLAPSE_KEY, JSON.stringify([...next])); } catch { /* quota/priv mode: ignora */ }
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
  async function handleCreate(name: string, cwd?: string, configDir?: string | null) {
    await createSession(name, cwd, configDir);
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

  // Abre o menu recarregando a lista de servidores (pode ter mudado desde a última abertura).
  function openMenu() {
    servers = listServers();
    showMenu = !showMenu;
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
    showMenu = false;
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
  <NavBar
    title="claude pocket"
    subtitle={sessions.length > 0 ? summaryText : null}
    subtitleHot={awaitingCount > 0 ? `${awaitingCount} aguardando` : null}
    onMenu={openMenu}
  />

  {#if showMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      class="menu-backdrop"
      role="button"
      tabindex="-1"
      aria-label="Fechar menu"
      onclick={() => (showMenu = false)}
    >
      <div class="menu-popup" role="menu">
        <div class="menu-section-label">Servidores</div>
        {#each servers as s (s.id)}
          <div class="menu-server">
            <div class="server-row">
              <span class="server-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
              {#if editingId === s.id}
                <!-- svelte-ignore a11y_autofocus -->
                <input
                  class="server-edit"
                  bind:value={editLabel}
                  onclick={(e) => e.stopPropagation()}
                  onkeydown={(e) => { if (e.key === 'Enter') saveRename(); if (e.key === 'Escape') editingId = null; }}
                  onblur={saveRename}
                  autofocus
                  aria-label="Novo nome do servidor"
                />
              {:else}
                <span class="server-label">{s.label}</span>
                <button class="server-rename" aria-label={`Renomear ${s.label}`} title="Renomear" onclick={(e) => { e.stopPropagation(); startRename(s.id, s.label); }}>✎</button>
              {/if}
            </div>
            <button class="server-remove" aria-label={`Remover ${s.label}`} onclick={() => dropServer(s.id)}>×</button>
          </div>
        {/each}
        <button class="menu-item" role="menuitem" onclick={openAddServer}>
          + Adicionar servidor
        </button>
        <div class="menu-divider"></div>
        {#if pushSupported()}
          <button class="menu-item" role="menuitem" onclick={handleEnablePush} disabled={pushBusy}>
            {pushBusy ? 'Ativando…' : '🔔 Ativar notificações'}
          </button>
          {#if pushMsg}
            <div class="menu-push-msg">{pushMsg}</div>
          {/if}
        {/if}
        <button class="menu-item" role="menuitem" onclick={() => { for (const es of streams.values()) es.close(); streams.clear(); connect(servers); showMenu = false; }}>
          Atualizar
        </button>
        <button class="menu-item" role="menuitem" onclick={() => { showMenu = false; window.location.hash = '#/costs'; }}>
          Custos
        </button>
        <button class="menu-item" role="menuitem" onclick={() => { showMenu = false; window.location.hash = '#/archive'; }}>
          Arquivo
        </button>
        <button class="menu-item menu-item--danger" role="menuitem" onclick={() => { showMenu = false; confirmLogout = true; }}>
          Sair
        </button>
      </div>
    </div>
  {/if}

  <div class="list-content">
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
        {#if multiServer}
          {#each grouped as g (g.id)}
            {@const awaiting = g.sessions.filter((s) => s.state === 'awaiting_input').length}
            <div class="group">
              <button
                class="group-head"
                onclick={() => toggleGroup(g.id)}
                aria-expanded={!collapsed.has(g.id)}
                aria-label={`${g.label}: ${g.sessions.length} ${g.sessions.length === 1 ? 'sessão' : 'sessões'}`}
              >
                <span class="group-chevron" class:collapsed={collapsed.has(g.id)} aria-hidden="true">▾</span>
                <span class="group-dot" style="background: {g.color};" aria-hidden="true"></span>
                <span class="group-label">{g.label}</span>
                <span class="group-count">{g.sessions.length}</span>
                {#if awaiting > 0}
                  <span class="group-await">{awaiting} aguardando</span>
                {/if}
              </button>
              {#if !collapsed.has(g.id)}
                {#each g.sessions as session (session.serverId + ':' + session.name)}
                  <SessionCard
                    {session}
                    serverBadge={null}
                    onClick={() => openSession(session)}
                    onDelete={() => handleDelete(session)}
                    onResume={() => handleResume(session)}
                  />
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
            />
          {/each}
        {/if}
      {/if}
    {/if}
  </div>

  <!-- FAB: new session -->
  <button
    class="fab"
    onclick={() => (showCreateSheet = true)}
    aria-label="Nova sessão"
  >
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" aria-hidden="true">
      <line x1="12" y1="5" x2="12" y2="19"/>
      <line x1="5" y1="12" x2="19" y2="12"/>
    </svg>
  </button>

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
</div>

<style>
  .session-list-screen {
    display: flex;
    flex-direction: column;
    height: 100%;
    position: relative;
    overflow: hidden;
  }

  .list-content {
    flex: 1;
    overflow-y: scroll;
    -webkit-overflow-scrolling: touch;
    overscroll-behavior-y: contain;
    padding: var(--space-2) 0;
    padding-bottom: calc(env(safe-area-inset-bottom) + 80px);
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

  .fab {
    position: fixed;
    bottom: calc(env(safe-area-inset-bottom) + var(--space-5));
    right: var(--space-5);
    width: 52px;
    height: 52px;
    background: var(--accent);
    border-radius: var(--radius-full);
    color: #fff;
    box-shadow: 0 4px 16px rgba(124,106,247,0.4);
    transition: background 180ms ease-out, transform 80ms ease-in-out;
    z-index: 20;
  }

  .fab:active {
    background: var(--accent-press);
    transform: scale(0.94);
  }

  /* Overflow menu */
  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
  }

  .menu-popup {
    position: absolute;
    top: calc(env(safe-area-inset-top) + 56px);
    right: var(--space-4);
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    overflow: hidden;
    min-width: 200px;
    box-shadow: 0 8px 32px rgba(0,0,0,0.4);
  }

  /* Seção de servidores (multi-PC) no menu */
  .menu-section-label {
    font-size: var(--text-xs);
    color: var(--text-muted);
    padding: var(--space-3) var(--space-4) var(--space-1);
  }
  .menu-server {
    display: flex;
    align-items: center;
    border-bottom: 1px solid var(--border-subtle);
  }
  .server-row {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 44px;
    padding: var(--space-2) var(--space-4);
    font-size: var(--text-sm);
    color: var(--text-primary);
    min-width: 0;
  }
  .server-dot {
    width: 8px; height: 8px; border-radius: 50%;
    background: var(--border-default); flex-shrink: 0;
  }
  .server-label {
    flex: 1; min-width: 0;
    overflow-wrap: anywhere; word-break: break-word;
  }
  .server-rename {
    width: 32px; height: 32px; flex-shrink: 0;
    color: var(--text-muted); font-size: var(--text-sm);
    border-radius: var(--radius-sm);
  }
  .server-rename:active { color: var(--accent); }
  .server-edit {
    flex: 1; min-width: 0; height: 32px;
    background: var(--bg-base);
    border: 1px solid var(--accent);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    padding: 0 var(--space-2);
    outline: none;
  }
  .server-remove {
    width: 40px; height: 44px; flex-shrink: 0;
    color: var(--text-muted); font-size: var(--text-lg); line-height: 1;
  }
  .server-remove:active { color: var(--error); }
  .menu-divider { height: 1px; background: var(--border-subtle); }

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

  .menu-item {
    width: 100%;
    height: 48px;
    padding: 0 var(--space-4);
    text-align: left;
    font-size: var(--text-base);
    color: var(--text-primary);
    border-bottom: 1px solid var(--border-subtle);
    justify-content: flex-start;
    border-radius: 0;
  }

  .menu-item:last-child {
    border-bottom: none;
  }

  .menu-item:active {
    background: var(--bg-hover);
  }

  .menu-item--danger {
    color: var(--error);
  }

  .menu-push-msg {
    font-size: var(--text-xs);
    color: var(--text-muted);
    padding: var(--space-1) var(--space-4) var(--space-2);
    border-bottom: 1px solid var(--border-subtle);
  }

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
