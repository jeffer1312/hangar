<script lang="ts">
  import { onMount } from 'svelte';
  import { getSessions, createSession, deleteSession, renameSession } from '../lib/api';
  import { listServers, getActiveId, selectServer, removeServer, addServer, clearCredentials } from '../lib/auth';
  import CreateSessionSheet from './CreateSessionSheet.svelte';
  import QrScanner from './QrScanner.svelte';
  import type { SessionInfo, State } from '../lib/types';

  // Sidebar do DESKTOP (so monta >=820px). Reusa as MESMAS APIs/componentes do mobile, sem tocar
  // no fluxo mobile (SessionList continua intacto). Recolhe pra um trilho de ícones.
  interface Props {
    currentSession: string | null;
    onSelect: (name: string) => void;
    onLogout: () => void;
  }
  let { currentSession, onSelect, onLogout }: Props = $props();

  let sessions = $state<SessionInfo[]>([]);
  let collapsed = $state(false);
  let servers = $state(listServers());
  let activeId = $state(getActiveId());
  let scanning = $state(false);
  let showCreate = $state(false);
  let serversOpen = $state(false);

  const urgency: Record<State, number> = { awaiting_input: 0, working: 1, idle: 2, dead: 3 };
  const sorted = $derived(
    [...sessions].sort((a, b) => {
      const byAct = (b.last_activity ?? 0) - (a.last_activity ?? 0);
      return byAct !== 0 ? byAct : urgency[a.state] - urgency[b.state];
    }),
  );

  async function load() {
    try { sessions = await getSessions(); } catch { /* mantem a lista atual */ }
  }
  onMount(() => {
    load();
    const iv = setInterval(load, 5000);
    return () => clearInterval(iv);
  });

  async function handleCreate(name: string, cwd?: string) {
    const s = await createSession(name, cwd);
    sessions = [s, ...sessions];
    onSelect(name);
  }
  async function handleDelete(name: string, e: MouseEvent) {
    e.stopPropagation();
    try {
      await deleteSession(name);
      sessions = sessions.filter((s) => s.name !== name);
    } catch { /* ignora */ }
  }

  // ── Renomear sessão do tmux: TOQUE LONGO no nome -> edita inline ──────────────
  let editing = $state<string | null>(null);   // nome da sessão em edição
  let editValue = $state('');
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  let longPressed = false;

  function pressStart(name: string) {
    longPressed = false;
    clearTimeout(pressTimer);
    pressTimer = setTimeout(() => { longPressed = true; editing = name; editValue = name; }, 500);
  }
  function pressEnd() {
    clearTimeout(pressTimer);
  }
  function onMainClick(name: string) {
    if (longPressed) { longPressed = false; return; }   // foi toque longo (renomear) -> não abre
    // sem id confiavel (tracked===false): nao abre o chat (evitaria mostrar transcript errado).
    if (sessions.find((x) => x.name === name)?.tracked === false) return;
    onSelect(name);
  }

  // session-id (uuid) = basename do jsonl sem extensao; mostra os 8 primeiros pra identificar a sessao.
  function shortId(s: SessionInfo): string | null {
    const f = s.jsonl?.split('/').pop();
    return f ? f.replace(/\.jsonl$/, '').slice(0, 8) : null;
  }
  async function saveEdit(old: string) {
    const nv = editValue.trim();
    editing = null;
    if (!nv || nv === old) return;
    try {
      const r = await renameSession(old, nv);
      sessions = sessions.map((s) => (s.name === old ? { ...s, name: r.name } : s));
      if (old === currentSession) onSelect(r.name);
    } catch { /* reload corrige */ }
    load();
  }
  function onEditKey(e: KeyboardEvent, old: string) {
    if (e.key === 'Enter') { e.preventDefault(); (e.target as HTMLInputElement).blur(); }
    else if (e.key === 'Escape') { editValue = old; editing = null; }   // cancela (blur vira no-op)
  }
  function autofocus(node: HTMLInputElement) {
    node.focus();
    node.select();
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
    {#each sorted as s (s.name)}
      <div class="sess-row" class:active={s.name === currentSession}>
        {#if editing === s.name}
          <!-- Toque longo no nome -> edita inline. Enter/blur salva, Esc cancela. -->
          <input
            class="sess-edit"
            bind:value={editValue}
            use:autofocus
            onkeydown={(e) => onEditKey(e, s.name)}
            onblur={() => saveEdit(s.name)}
            aria-label="Renomear sessão"
          />
        {:else}
          <button
            class="sess-main"
            class:untracked={s.tracked === false}
            title={collapsed ? s.name : (s.tracked === false ? 'claude aberto sem --session-id: transcript nao rastreavel' : 'Toque longo pra renomear')}
            onpointerdown={() => pressStart(s.name)}
            onpointerup={pressEnd}
            onpointerleave={pressEnd}
            onpointercancel={pressEnd}
            oncontextmenu={(e) => e.preventDefault()}
            onclick={() => onMainClick(s.name)}
          >
            <span class="dot dot--{s.state}" aria-hidden="true"></span>
            {#if !collapsed}<span class="sess-name">{s.name}</span>{/if}
            {#if !collapsed}
              {#if s.tracked === false}
                <span class="sess-badge" title="sem --session-id: nao rastreavel">sem id</span>
              {:else if shortId(s)}
                <span class="sess-id" title={`session-id: ${shortId(s)}…`}>#{shortId(s)}</span>
              {/if}
            {/if}
          </button>
          {#if !collapsed}
            <button class="sess-del" onclick={(e) => handleDelete(s.name, e)} aria-label={`Apagar ${s.name}`}>×</button>
          {/if}
        {/if}
      </div>
    {/each}
  </nav>

  {#if !collapsed}
    <div class="side-foot">
      {#if serversOpen}
        <div class="srv-menu">
          {#each servers as s (s.id)}
            <div class="srv-row">
              <button class="srv-pick" onclick={() => pickServer(s.id)}>
                <span class="srv-dot" class:on={s.id === activeId} aria-hidden="true"></span>
                <span class="srv-label">{s.label}</span>
              </button>
              {#if servers.length > 1}<button class="srv-del" onclick={() => dropServer(s.id)} aria-label="Remover">×</button>{/if}
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
  .sess-row { display: flex; align-items: center; border-radius: var(--radius-md); }
  /* hover SÓ em dispositivo com mouse. No touch (tablet), o :hover fazia o 1º toque virar "hover" e
     o 2º o clique -> precisava de 2 toques pra abrir a sessão. hover:hover isola isso. */
  @media (hover: hover) { .sess-row:hover { background: var(--bg-hover); } }
  .sess-row.active { background: var(--bg-elevated); }
  .sess-main {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2); height: 38px;
    padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--text-secondary);
    border-radius: var(--radius-md);
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
  .sess-id { flex-shrink: 0; font-family: var(--font-mono); font-size: 10px; color: var(--text-muted); white-space: nowrap; }
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

  .dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; background: var(--text-muted); }
  /* working/awaiting "pensando": pulsa pra dar o badge de atividade na sidebar (igual ao botão de
     atividade do topo). prefers-reduced-motion -> só a cor. */
  .dot--working { background: var(--accent); animation: dot-pulse 1.4s ease-in-out infinite; }
  .dot--awaiting_input { background: var(--warning); animation: dot-pulse 1.4s ease-in-out infinite; }
  .dot--idle { background: var(--success, #3fb950); }
  .dot--dead { background: var(--error); }
  @keyframes dot-pulse { 0%, 100% { opacity: 1; transform: scale(1); } 50% { opacity: 0.4; transform: scale(0.7); } }
  @media (prefers-reduced-motion: reduce) { .dot--working, .dot--awaiting_input { animation: none; } }

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
  .srv-del { width: 28px; height: 32px; min-height: 0; color: var(--text-muted); font-size: var(--text-base); }
  .srv-del:hover { color: var(--error); }
  .srv-add { height: 32px; padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--accent); font-size: var(--text-sm); }
  .logout-btn { height: 34px; padding: 0 var(--space-2); text-align: left; justify-content: flex-start; color: var(--text-muted); font-size: var(--text-sm); border-radius: var(--radius-md); }
  .logout-btn:hover { background: var(--bg-hover); color: var(--error); }
</style>
