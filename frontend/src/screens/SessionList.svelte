<script lang="ts">
  import { onMount } from 'svelte';
  import NavBar from '../components/NavBar.svelte';
  import SessionCard from '../components/SessionCard.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
  import QrScanner from '../components/QrScanner.svelte';
  import { getAllSessions, getSessions, createSession, deleteSession } from '../lib/api';
  import { clearCredentials, listServers, getActiveId, selectServer, removeServer, addServer, renameServer, serverColor } from '../lib/auth';
  import type { AggSession, State } from '../lib/types';

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
      loadSessions(true);   // reagrega pra os badges das sessoes pegarem o nome novo
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

  // Urgencia pra desempate: aguardando_input puxa pro topo; dead pro fim.
  const urgency: Record<State, number> = {
    awaiting_input: 0,
    working: 1,
    idle: 2,
    dead: 3,
  };

  // Ordena por atividade (desc) e desempata por urgencia; depois aplica o filtro.
  const visibleSessions = $derived.by(() => {
    const sorted = [...sessions].sort((a, b) => {
      const byAct = (b.last_activity ?? 0) - (a.last_activity ?? 0);
      if (byAct !== 0) return byAct;
      return urgency[a.state] - urgency[b.state];
    });
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

  // Agrega sessões de todos os servidores, marcando cada uma com a origem (id/label/cor). Servidor
  // offline vira aviso em serverErrors, não derruba a lista. silent=true nos polls (sem spinner).
  async function loadSessions(silent = false) {
    const list = listServers();
    servers = list;
    if (list.length === 0) { sessions = []; serverErrors = []; loading = false; return; }
    if (!silent) loading = true;
    error = '';
    try {
      const results = await getAllSessions(list);
      const agg: AggSession[] = [];
      const errs: { label: string; error: string }[] = [];
      // Dedupe: vários servidores podem apontar pro MESMO backend (URLs diferentes). A identidade
      // real da sessão é (jsonl, name) — o jsonl tem um uuid único por sessão, então sessões
      // distintas nunca colidem; só a mesma sessão vista por 2 URLs colapsa (fica a 1ª).
      const seen = new Set<string>();
      for (const r of results) {
        if (r.sessions) {
          for (const s of r.sessions) {
            const key = `${s.jsonl ?? s.cwd ?? ''}::${s.name}`;
            if (seen.has(key)) continue;
            seen.add(key);
            agg.push({ ...s, serverId: r.server.id, serverLabel: r.server.label, serverColor: serverColor(r.server.id) });
          }
        } else {
          errs.push({ label: r.server.label, error: r.error ?? 'offline' });
        }
      }
      sessions = agg;
      serverErrors = errs;
    } catch (err) {
      error = err instanceof Error ? err.message : 'Erro ao carregar sessões';
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    loadSessions();
    // Poll for updates every 5 seconds (silent: sem spinner)
    const interval = setInterval(() => loadSessions(true), 5000);
    return () => clearInterval(interval);
  });

  // O sheet de criar já posicionou o servidor-alvo como ativo (selectServer), então createSession
  // cai no servidor certo. Depois reagrega a lista pra a sessão nova aparecer com a marca correta.
  async function handleCreate(name: string, cwd?: string) {
    await createSession(name, cwd);
    await loadSessions(true);
  }

  // Abrir/apagar precisam mirar o servidor DA sessão: selectServer(serverId) antes, pois api.ts lê
  // o ativo a cada chamada (sem reload). Assim chat/SSE/delete vão pro backend certo.
  function openSession(s: AggSession) {
    if (s.tracked === false) return; // sem id confiavel: chat bloqueado (evita transcript errado)
    selectServer(s.serverId);
    onNavigateToChat(s.name);
  }

  async function handleDelete(s: AggSession) {
    selectServer(s.serverId);
    await deleteSession(s.name);
    sessions = sessions.filter((x) => !(x.serverId === s.serverId && x.name === s.name));
  }

  function handleLogout() {
    clearCredentials();
    onLogout();
  }

  // Abre o menu recarregando a lista de servidores (pode ter mudado desde a última abertura).
  function openMenu() {
    servers = listServers();
    showMenu = !showMenu;
  }

  // Remove um servidor da lista. Sem "ativo" pra restaurar — só reagrega (ou desloga se zerou).
  function dropServer(id: string) {
    removeServer(id);
    servers = listServers();
    if (servers.length === 0) { handleLogout(); return; }
    loadSessions(true);
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
        <button class="menu-item" role="menuitem" onclick={() => { loadSessions(); showMenu = false; }}>
          Atualizar
        </button>
        <button class="menu-item menu-item--danger" role="menuitem" onclick={handleLogout}>
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
        <button class="retry-btn" onclick={() => loadSessions()}>Tentar novamente</button>
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
        {#each visibleSessions as session (session.serverId + ':' + session.name)}
          <SessionCard
            {session}
            serverBadge={multiServer ? { label: session.serverLabel, color: session.serverColor } : null}
            onClick={() => openSession(session)}
            onDelete={() => handleDelete(session)}
          />
        {/each}
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
    padding: var(--space-4);
    padding-bottom: calc(env(safe-area-inset-bottom) + 80px);
  }

  .server-warn {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin-bottom: var(--space-3);
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
    width: 100%;
    height: 44px;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    padding: 0 var(--space-3);
    outline: none;
    margin-bottom: var(--space-3);
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
  .add-primary:disabled { opacity: 0.5; cursor: default; }
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
</style>
