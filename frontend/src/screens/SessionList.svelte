<script lang="ts">
  import { onMount } from 'svelte';
  import NavBar from '../components/NavBar.svelte';
  import SessionCard from '../components/SessionCard.svelte';
  import CreateSessionSheet from '../components/CreateSessionSheet.svelte';
  import QrScanner from '../components/QrScanner.svelte';
  import { getSessions, createSession, deleteSession } from '../lib/api';
  import { clearCredentials, listServers, getActiveId, selectServer, removeServer, addServer } from '../lib/auth';
  import type { SessionInfo, State } from '../lib/types';

  interface Props {
    onNavigateToChat: (name: string) => void;
    onLogout: () => void;
  }
  let { onNavigateToChat, onLogout }: Props = $props();

  let sessions = $state<SessionInfo[]>([]);
  let loading = $state(true);
  let error = $state('');
  let showCreateSheet = $state(false);
  let showMenu = $state(false);
  let filterText = $state('');

  // Multi-servidor: lista + ativo (recarregados ao abrir o menu). Trocar de server recarrega o app.
  let servers = $state(listServers());
  let activeId = $state(getActiveId());
  let scanning = $state(false);

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
      (s) => s.name.toLowerCase().includes(q) || (s.cwd ?? '').toLowerCase().includes(q),
    );
  });

  // Filtro so aparece quando a lista fica longa.
  const showFilter = $derived(sessions.length > 6);

  async function loadSessions() {
    loading = true;
    error = '';
    try {
      sessions = await getSessions();
    } catch (err) {
      error = err instanceof Error ? err.message : 'Erro ao carregar sessões';
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    loadSessions();
    // Poll for updates every 5 seconds
    const interval = setInterval(loadSessions, 5000);
    return () => clearInterval(interval);
  });

  async function handleCreate(name: string, cwd?: string) {
    const session = await createSession(name, cwd);
    sessions = [session, ...sessions];
  }

  async function handleDelete(name: string) {
    await deleteSession(name);
    sessions = sessions.filter(s => s.name !== name);
  }

  function handleLogout() {
    clearCredentials();
    onLogout();
  }

  // Abre o menu recarregando a lista de servidores (pode ter mudado desde a última abertura).
  function openMenu() {
    servers = listServers();
    activeId = getActiveId();
    showMenu = !showMenu;
  }

  function pickServer(id: string) {
    if (id === getActiveId()) { showMenu = false; return; }
    selectServer(id);
    // Server ativo mudou -> recarrega pra re-buscar sessões/SSE do novo backend.
    window.location.reload();
  }

  function dropServer(id: string) {
    const wasActive = id === getActiveId();
    removeServer(id);
    servers = listServers();
    activeId = getActiveId();
    if (servers.length === 0) { handleLogout(); return; }
    if (wasActive) window.location.reload();
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
            <button
              class="menu-item server-pick"
              role="menuitemradio"
              aria-checked={s.id === activeId}
              onclick={() => pickServer(s.id)}
            >
              <span class="server-dot" class:on={s.id === activeId} aria-hidden="true"></span>
              <span class="server-label">{s.label}</span>
            </button>
            {#if servers.length > 1}
              <button class="server-remove" aria-label={`Remover ${s.label}`} onclick={() => dropServer(s.id)}>×</button>
            {/if}
          </div>
        {/each}
        <button class="menu-item" role="menuitem" onclick={() => { scanning = true; showMenu = false; }}>
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
    {#if loading && sessions.length === 0}
      <div class="empty-state">
        <div class="spinner-large" aria-label="Carregando…">⟳</div>
        <p>Carregando sessões…</p>
      </div>
    {:else if error}
      <div class="empty-state">
        <p class="error-text">{error}</p>
        <button class="retry-btn" onclick={loadSessions}>Tentar novamente</button>
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
        {#each visibleSessions as session (session.name)}
          <SessionCard
            {session}
            onClick={() => onNavigateToChat(session.name)}
            onDelete={() => handleDelete(session.name)}
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
    onClose={() => (showCreateSheet = false)}
    onCreate={handleCreate}
    onOpenSession={onNavigateToChat}
  />

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
  .server-pick {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    height: 44px;
    padding: 0 var(--space-4);
    text-align: left;
    font-size: var(--text-sm);
    color: var(--text-primary);
    justify-content: flex-start;
    border-radius: 0;
    border-bottom: none;
  }
  .server-pick:active { background: var(--bg-hover); }
  .server-dot {
    width: 8px; height: 8px; border-radius: 50%;
    background: var(--border-default); flex-shrink: 0;
  }
  .server-dot.on { background: var(--accent); }
  .server-label {
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis; max-width: 150px;
  }
  .server-remove {
    width: 40px; height: 44px; flex-shrink: 0;
    color: var(--text-muted); font-size: var(--text-lg); line-height: 1;
  }
  .server-remove:active { color: var(--error); }
  .menu-divider { height: 1px; background: var(--border-subtle); }

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
