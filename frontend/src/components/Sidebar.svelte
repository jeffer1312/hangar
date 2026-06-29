<script lang="ts">
  import { onMount } from 'svelte';
  import { openSessionsStream, createSession, deleteSession, renameSession } from '../lib/api';
  import { listServers, getActiveId, selectServer, removeServer, addServer, renameServer, serverColor, clearCredentials } from '../lib/auth';
  import CreateSessionSheet from './CreateSessionSheet.svelte';
  import QrScanner from './QrScanner.svelte';
  import type { SessionInfo, State } from '../lib/types';
  import type { Server } from '../lib/auth';
  import Lottie from './Lottie.svelte';
  import pensando from '../lib/lottie/pensando.json';

  const stateLabels: Record<State, string> = { working: 'exec', idle: 'pronto', awaiting_input: 'aguardando', dead: 'encerrado' };
  const stateColors: Record<State, string> = { working: 'var(--accent)', idle: 'var(--success)', awaiting_input: 'var(--warning)', dead: 'var(--error)' };
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
    onLogout: () => void;
  }
  let { currentSession, onSelect, onLogout }: Props = $props();

  interface Group { server: Server; sessions: SessionInfo[]; error: string | null }
  let groups = $state<Group[]>([]);
  let collapsed = $state(false);
  let servers = $state(listServers());
  let activeId = $state(getActiveId());
  let scanning = $state(false);
  let showCreate = $state(false);
  let serversOpen = $state(false);

  const urgency: Record<State, number> = { awaiting_input: 0, working: 1, idle: 2, dead: 3 };
  // Ordena DENTRO de cada grupo: atividade desc, depois urgência do estado.
  function sortSessions(list: SessionInfo[]): SessionInfo[] {
    return [...list].sort((a, b) => {
      const byAct = (b.last_activity ?? 0) - (a.last_activity ?? 0);
      return byAct !== 0 ? byAct : urgency[a.state] - urgency[b.state];
    });
  }

  const slots = new Map<string, { sessions: SessionInfo[] | null; error: string | null }>();
  function recompute() {
    if (servers.length === 0) { groups = []; return; }
    const seen = new Set<string>(); // dedup global: backend compartilhado por 2 URLs não duplica
    groups = servers.map((srv) => {
      const slot = slots.get(srv.id);
      if (!slot || !slot.sessions) return { server: srv, sessions: [], error: slot?.error ?? null };
      const fresh = slot.sessions.filter((s) => {
        const key = `${s.jsonl ?? s.cwd ?? ''}::${s.name}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      });
      return { server: srv, sessions: sortSessions(fresh), error: null };
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
  async function handleDelete(name: string, serverId: string, e: MouseEvent) {
    e.stopPropagation();
    const prev = getActiveId(); // C1: save before pointing at target server
    selectServer(serverId); // api.ts mira o server ativo -> aponta pro dono da sessão
    try { await deleteSession(name); } catch { /* ignora */ }
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
    }
    editingServer = null;
  }

  function pickServer(id: string) {
    if (id === getActiveId()) { serversOpen = false; return; }
    selectServer(id);
    window.location.reload();
  }
  function dropServer(id: string) {
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

  const activeServer = $derived(servers.find((s) => s.id === activeId) ?? servers[0] ?? null);
</script>

<aside class="sidebar" class:collapsed>
  <div class="side-top">
    <button class="icon-btn" onclick={() => (collapsed = !collapsed)} aria-label={collapsed ? 'Expandir' : 'Recolher'}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
        <rect x="3" y="4" width="18" height="16" rx="2"/>
        <line x1="9" y1="4" x2="9" y2="20"/>
      </svg>
    </button>
    {#if !collapsed}<span class="side-brand">claude pocket</span>{/if}
  </div>

  <button class="new-btn" onclick={() => (showCreate = true)} aria-label="Nova sessão">
    <span class="new-plus" aria-hidden="true">+</span>
    {#if !collapsed}<span>Nova sessão</span>{/if}
  </button>

  <nav class="sess-list" aria-label="Sessões">
    {#each groups as g (g.server.id)}
      {#if !collapsed}
        <div class="grp-head" title={g.error ? `${g.server.label}: ${g.error}` : g.server.label}>
          <span class="grp-dot" style="background: {serverColor(g.server.id)};" aria-hidden="true"></span>
          <span class="grp-label">{g.server.label}</span>
          {#if g.error}<span class="grp-off">offline</span>{/if}
        </div>
      {/if}
      {#each g.sessions as s (s.name)}
        {@const rowKey = `${g.server.id}::${s.name}`}
        <div class="sess-row" class:active={g.server.id === activeId && s.name === currentSession}>
          {#if editing === rowKey}
            <input
              class="sess-edit"
              bind:value={editValue}
              use:autofocus
              onkeydown={(e) => onEditKey(e, s.name)}
              onblur={() => saveEdit(s.name, g.server.id)}
              aria-label="Renomear sessão"
            />
          {:else}
            <button
              class="sess-main"
              class:untracked={s.tracked === false}
              title={collapsed ? s.name : (s.tracked === false ? 'claude aberto sem --session-id: transcript nao rastreavel' : 'Toque longo pra renomear')}
              onpointerdown={() => pressStart(rowKey)}
              onpointerup={pressEnd}
              onpointerleave={pressEnd}
              onpointercancel={pressEnd}
              oncontextmenu={(e) => e.preventDefault()}
              onclick={() => onMainClick(s.name, g.server.id, s.tracked)}
            >
              <span class="lead" aria-hidden="true">
                {#if s.state === 'working'}
                  <Lottie data={pensando as any} size={18} loop autoplay />
                {:else}
                  <Lottie data={pensando as any} size={18} loop={false} autoplay={false} frame={STATIC_FRAME} />
                {/if}
              </span>
              {#if !collapsed}
                <span class="row-info">
                  <span class="name-row">
                    <span class="sess-name">{s.name}</span>
                    {#if s.tracked === false}<span class="sess-badge" title="sem --session-id: nao rastreavel">sem id</span>{/if}
                  </span>
                  {#if s.cwd}
                    {@const cp = cwdParts(s.cwd)}
                    <span class="cwd" title={s.cwd}><span class="cwd-prefix">{cp.prefix}</span><span class="cwd-base">{cp.base}</span></span>
                  {/if}
                </span>
                <span class="state-chip" style="color: {stateColors[s.state]}; background: {stateChipBg[s.state]};">{stateLabels[s.state]}</span>
              {/if}
            </button>
            {#if !collapsed}
              <button class="sess-del" onclick={(e) => handleDelete(s.name, g.server.id, e)} aria-label={`Apagar ${s.name}`}>×</button>
            {/if}
          {/if}
        </div>
      {/each}
    {/each}
  </nav>

  {#if !collapsed}
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
      <button class="logout-btn" onclick={logout}>Sair</button>
    </div>
  {/if}
</aside>

<CreateSessionSheet open={showCreate} {servers} onClose={() => (showCreate = false)} onCreate={handleCreate} onOpenSession={onSelect} />
{#if scanning}<QrScanner onScan={handleScan} onClose={() => (scanning = false)} />{/if}

<style>
  .sidebar {
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

  .new-btn {
    display: flex; align-items: center; gap: var(--space-2); height: 40px; padding: 0 var(--space-3);
    border-radius: var(--radius-md); background: var(--accent-dim); color: var(--text-primary);
    font-size: var(--text-sm); font-weight: 500; justify-content: flex-start; white-space: nowrap;
  }
  .sidebar.collapsed .new-btn { justify-content: center; padding: 0; }
  .new-btn:hover { background: var(--accent); color: #fff; }
  .new-plus { font-size: var(--text-lg); line-height: 1; flex-shrink: 0; }

  .sess-list { flex: 1; overflow-y: auto; display: flex; flex-direction: column; gap: 2px; margin-top: var(--space-2); }
  .grp-head {
    display: flex; align-items: center; gap: var(--space-2);
    padding: var(--space-2) var(--space-2) 4px;
    font-size: var(--text-xs); font-weight: 600; color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.04em;
  }
  .grp-head:not(:first-child) { margin-top: var(--space-2); }
  .grp-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; }
  .grp-label { flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .grp-off { color: var(--warning); font-weight: 600; text-transform: none; letter-spacing: 0; }
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
  .cwd { display: flex; min-width: 0; font-family: var(--font-mono); font-size: 10px; }
  .cwd-prefix { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-muted); }
  .cwd-base { flex: 0 0 auto; white-space: nowrap; color: var(--text-secondary); }
  .state-chip {
    flex-shrink: 0; font-size: 10px; font-weight: 600; letter-spacing: 0.02em;
    padding: 2px 7px; border-radius: var(--radius-full); white-space: nowrap;
  }
  .lead { width: 18px; flex-shrink: 0; display: inline-flex; align-items: center; justify-content: center; }
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
  .logout-btn { height: 34px; padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--text-muted); font-size: var(--text-sm); border-radius: var(--radius-md); }
  .logout-btn:hover { background: var(--bg-hover); color: var(--error); }
</style>
