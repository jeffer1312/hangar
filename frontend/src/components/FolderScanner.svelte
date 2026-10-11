<script lang="ts">
  import { onMount } from 'svelte';
  import { getRoots, getRootsForServer, makeDir, scanDir } from '@hangar/core';
  import type { Server } from '@hangar/core';
  import { relativeTime } from '@hangar/core';
  import type { FsRoot, FsEntry, FsScanError } from '@hangar/core';
  import * as m from '../paraglide/messages';

  // Scanner mobile de pastas de projeto: chips de raiz + busca + coluna tappavel de
  // drill-in. Toque no corpo da linha SELECIONA o caminho como cwd; o chevron desce um
  // nivel (re-scan). Breadcrumb de uma linha pra subir. A allowlist e validada no backend.
  interface Props {
    onPick: (path: string) => void;
    /** Painel desktop do CreateSessionSheet: o scanner vira coluna flex que PREENCHE a altura
     *  do pane e a lista rola sozinha (sem o teto de 46vh do fluxo mobile). */
    fill?: boolean;
    /** Caminho já escolhido (desktop de dois painéis): a linha fica marcada — é ELA que diz qual
     *  pasta o formulário à direita está configurando. */
    selected?: string | null;
    /** Navega as pastas DESTE servidor em vez do ativo (pasta do convidado em cada máquina). */
    server?: Server;
    /** Mostra "Nova pasta" no diretório atual. */
    canCreate?: boolean;
    /** Mostra "Pesquisar em todas as pastas": a busca passa a varrer todas as raízes (create.rs refilter). */
    searchAllOption?: boolean;
  }
  let { onPick, fill = false, selected = null, server, canCreate = false, searchAllOption = false }: Props = $props();

  const SEARCH_ALL_KEY = 'cp_create_search_all';
  let searchAll = $state((() => { try { return localStorage.getItem(SEARCH_ALL_KEY) === '1'; } catch { return false; } })());
  // Pastas de cada raiz, lidas uma vez enquanto o scanner está montado; `null` = falhou.
  let rootEntries = $state<Record<string, FsEntry[] | null | 'loading'>>({});
  function setSearchAll(on: boolean) {
    searchAll = on;
    try { localStorage.setItem(SEARCH_ALL_KEY, on ? '1' : '0'); } catch { /* sem storage, vale só agora */ }
  }
  $effect(() => {
    if (!searchAllOption || !searchAll || !query.trim()) return;
    for (const r of roots) {
      if (rootEntries[r.path] !== undefined) continue;
      rootEntries[r.path] = 'loading';
      scanDir(r.path, r.path, server)
        .then((res) => { rootEntries[r.path] = res.error ? null : res.entries; })
        .catch(() => { rootEntries[r.path] = null; });
    }
  });
  const allMode = $derived(searchAllOption && searchAll && !!query.trim());
  const allLoading = $derived(allMode && roots.some((r) => rootEntries[r.path] === 'loading'));
  const allFailed = $derived(allMode ? roots.filter((r) => rootEntries[r.path] === null).map((r) => r.name) : []);

  const LAST_ROOT_KEY = 'cp:last-root';

  let roots = $state<FsRoot[]>([]);
  let rootsLoading = $state(true);
  let rootsError = $state(false);
  let failText = $state('');
  let activeRoot = $state<FsRoot | null>(null);
  let path = $state('');                 // diretorio atual (default = raiz)
  let entries = $state<FsEntry[]>([]);
  let scanning = $state(false);
  let scanError = $state<FsScanError | null>(null);
  let query = $state('');

  // ── Carrega as raizes (chips) ──────────────────────────────────────────────
  onMount(async () => {
    try {
      roots = server ? await getRootsForServer(server) : await getRoots();
    } catch (e) {
      failText = e instanceof Error ? e.message : String(e);
      rootsError = true;
      rootsLoading = false;
      return;
    }
    rootsLoading = false;
    if (roots.length === 0) return;
    const last = server ? null : localStorage.getItem(LAST_ROOT_KEY);
    selectRoot(roots.find((r) => r.path === last) ?? roots[0]);
  });

  function selectRoot(r: FsRoot) {
    activeRoot = r;
    try {
      // A raiz lembrada é a do servidor ativo; a de outra máquina não casa com a dele.
      if (!server) localStorage.setItem(LAST_ROOT_KEY, r.path);
    } catch {
      // localStorage indisponivel (modo privado) -> segue sem persistir
    }
    query = '';
    scan(r.path);
  }

  async function scan(target: string) {
    if (!activeRoot) return;
    const root = activeRoot.path;
    path = target;
    scanning = true;
    scanError = null;
    failText = '';
    let res: Awaited<ReturnType<typeof scanDir>>;
    try {
      res = await scanDir(root, target, server);
    } catch (e) {
      // 401 e queda de rede sobem de scanDir; sem isto o esqueleto ficava na tela para sempre.
      res = { entries: [], error: 'unknown' };
      failText = e instanceof Error ? e.message : String(e);
    }
    // descarta respostas obsoletas se o usuario navegou rapido pra outra pasta/raiz
    if (activeRoot?.path !== root || path !== target) return;
    entries = res.entries;
    scanError = res.error ?? null;
    scanning = false;
  }

  let creating = $state(false);
  let newName = $state('');
  let createBusy = $state(false);
  let createError = $state('');

  async function create() {
    if (!activeRoot || createBusy || !newName.trim()) return;
    createBusy = true;
    createError = '';
    try {
      const made = await makeDir(activeRoot.path, path, newName.trim(), server);
      creating = false;
      newName = '';
      await scan(path);
      onPick(made.path);
    } catch (e) {
      createError = m.arquivo_criar_pasta_erro({ erro: e instanceof Error ? e.message : String(e) });
    } finally {
      createBusy = false;
    }
  }

  function drill(e: FsEntry) {
    query = '';
    scan(e.path);
  }

  // ── Breadcrumb: raiz + segmentos do path relativo a ela ────────────────────
  const drilled = $derived(!!activeRoot && path !== activeRoot.path);
  const crumbs = $derived.by(() => {
    if (!activeRoot) return [] as { label: string; path: string }[];
    const base = activeRoot.path;
    const rest = path.startsWith(base) ? path.slice(base.length) : '';
    const out = [{ label: activeRoot.name, path: base }];
    let acc = base;
    for (const seg of rest.split('/').filter(Boolean)) {
      acc = acc + '/' + seg;
      out.push({ label: seg, path: acc });
    }
    return out;
  });

  // Caminho exibido relativo ao PAI da raiz, ex: "pessoal/hangar".
  function relPath(p: string): string {
    if (!activeRoot) return p;
    const parent = activeRoot.path.replace(/\/[^/]+$/, '');
    return parent && p.startsWith(parent + '/') ? p.slice(parent.length + 1) : p;
  }

  // ── Busca: filtra os filhos ja carregados (nome + caminho relativo) ─────────
  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return entries;
    const pool = allMode
      ? roots.flatMap((r) => { const v = rootEntries[r.path]; return Array.isArray(v) ? v : []; })
      : entries;
    return pool.filter(
      (e) => e.name.toLowerCase().includes(q) || relPath(e.path).toLowerCase().includes(q),
    );
  });

  const SCAN_MSG: Record<FsScanError, string> = {
    permission_denied: m.arquivo_sem_permissao(),
    unreadable: m.arquivo_ilegivel(),
    root_not_allowed: m.arquivo_raiz_nao_liberada(),
    invalid_path: m.arquivo_caminho_invalido(),
    not_found: m.arquivo_pasta_nao_encontrada(),
    unknown: m.arquivo_ler_falhou(),
  };
</script>

<div class="scanner" class:scanner--fill={fill}>
  <!-- Chips de raiz -->
  {#if rootsLoading}
    <div class="chips">
      <span class="chip chip--skel"></span>
      <span class="chip chip--skel"></span>
    </div>
  {:else if rootsError}
    <p class="state-msg">{m.arquivo_carregar_raizes_erro()}{failText ? ` (${failText})` : ''}</p>
  {:else if roots.length === 0}
    <p class="state-msg">{m.arquivo_sem_raizes()}</p>
  {:else}
    <div class="chips" role="tablist" aria-label={m.arquivo_raizes_aria()}>
      {#each roots as r (r.path)}
        <button
          type="button"
          class="chip"
          class:chip--active={activeRoot?.path === r.path}
          role="tab"
          aria-selected={activeRoot?.path === r.path}
          onclick={() => selectRoot(r)}
        >
          {r.name}
        </button>
      {/each}
    </div>
  {/if}

  {#if activeRoot}
    <!-- Busca -->
    <input
      type="text"
      class="search"
      bind:value={query}
      placeholder={m.arquivo_buscar_pasta()}
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      aria-label={m.arquivo_buscar_pasta()}
      onkeydown={(e) => { if (e.key === 'Enter') e.preventDefault(); }}
    />
    {#if searchAllOption}
      <label class="search-all">
        <input type="checkbox" checked={searchAll} onchange={(e) => setSearchAll(e.currentTarget.checked)} />
        <span>{m.native_create_search_all()}</span>
      </label>
    {/if}

    <!-- Breadcrumb (so quando aprofundou): toque numa migalha sobe -->
    {#if drilled}
      <div class="crumbs" aria-label={m.arquivo_caminho_aria()}>
        {#each crumbs as c, i (c.path)}
          {#if i > 0}<span class="crumb-sep" aria-hidden="true">/</span>{/if}
          <button type="button" class="crumb" onclick={() => scan(c.path)}>{c.label}</button>
        {/each}
      </div>
      <button type="button" class="use-here" onclick={() => onPick(path)}>
        {m.arquivo_usar_pasta()}
      </button>
    {/if}

    {#if canCreate}
      {#if creating}
        <div class="new-folder">
          <input class="search" bind:value={newName} placeholder={m.arquivo_nova_pasta_nome()}
            onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); void create(); } }}
            aria-label={m.arquivo_nova_pasta_nome()} autocomplete="off" autocorrect="off"
            autocapitalize="off" spellcheck={false} disabled={createBusy} />
          <button class="use-here" type="button" onclick={create} disabled={createBusy || !newName.trim()} aria-busy={createBusy}>
            {m.arquivo_criar_pasta()}
          </button>
          <button class="use-here" type="button" disabled={createBusy}
            onclick={() => { creating = false; createError = ''; }}>{m.comum_cancelar()}</button>
        </div>
      {:else}
        <button class="use-here" type="button" onclick={() => (creating = true)}>{m.arquivo_nova_pasta()}</button>
      {/if}
      {#if createError}<p class="state-msg create-error" role="alert">{createError}</p>{/if}
    {/if}

    <!-- Coluna de subpastas -->
    <div class="rows" role="list">
      {#if allFailed.length}
        <p class="state-msg" role="alert">{m.arquivo_ler_falhou()} ({allFailed.join(', ')})</p>
      {/if}
      {#if scanning || allLoading}
        {#each Array(5) as _, i (i)}
          <div class="row-skel" aria-hidden="true">
            <span class="skel-line skel-name"></span>
            <span class="skel-line skel-path"></span>
          </div>
        {/each}
      {:else if scanError}
        <p class="state-msg">{SCAN_MSG[scanError]}{failText ? ` (${failText})` : ''}</p>
      {:else if filtered.length === 0}
        <p class="state-msg">
          {query.trim() ? m.arquivo_sem_resultados() : m.arquivo_sem_subpastas()}
        </p>
      {:else}
        {#each filtered as e (e.path)}
          <div class="row" class:row--sel={selected === e.path} role="listitem">
            <button type="button" class="row-body" aria-pressed={selected === e.path} onclick={() => onPick(e.path)}>
              <span class="row-name">{e.name}</span>
              <span class="row-path">{relPath(e.path)}</span>
              <span class="row-badges">
                {#if e.is_git}<span class="badge badge--git">git</span>{/if}
                {#if e.has_claude_md}<span class="badge badge--cl">CLAUDE.md</span>{/if}
                {#if e.mtime}<span class="row-time">{relativeTime(e.mtime)}</span>{/if}
              </span>
            </button>
            <button type="button" class="drill" onclick={() => drill(e)} aria-label={m.arquivo_abrir({ nome: e.name })}>
              <svg width="9" height="15" viewBox="0 0 9 15" fill="none" aria-hidden="true">
                <path d="M1 1l6.5 6.5L1 14" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
            </button>
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .search-all { display: flex; align-items: center; gap: 10px; min-height: 44px; font-size: var(--text-sm); color: var(--text-primary); cursor: pointer; }
  .search-all input { width: 22px; height: 22px; accent-color: var(--accent); }
  .scanner {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .scanner--fill {
    flex: 1;
    min-height: 0;
  }
  .scanner--fill .rows {
    flex: 1;
    /* min-height:0 não é enfeite: sem ele a lista não encolhe abaixo do conteúdo (min-height:auto
       do flex) e o rodapé/form de caminho é empurrado pra fora do pane, que tem overflow hidden —
       o "Avançado" abria o form fora da tela e parecia não fazer nada. */
    min-height: 0;
    max-height: none;
  }

  .new-folder {
    display: flex;
    gap: var(--space-2);
    align-items: center;
  }
  .new-folder .search { flex: 1; min-width: 0; }
  .new-folder .use-here { flex-shrink: 0; }
  .create-error { color: var(--error); }

  /* ── Chips de raiz ─────────────────────────────────────────────────────── */
  .chips {
    display: flex;
    gap: var(--space-2);
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
    padding-bottom: 2px;
    scrollbar-width: none;
  }
  .chips::-webkit-scrollbar {
    display: none;
  }

  .chip {
    flex-shrink: 0;
    height: 36px;
    min-height: 36px;
    padding: 0 var(--space-4);
    border-radius: var(--radius-full);
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    font-weight: 500;
    white-space: nowrap;
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out),
      border-color 160ms var(--ease-out);
  }

  .chip--active {
    background: var(--accent-dim);
    border-color: var(--accent);
    color: var(--text-primary);
  }

  .chip--skel {
    width: 84px;
    background: var(--bg-surface);
    border-color: var(--border-subtle);
    animation: skel-pulse 1.2s ease-in-out infinite;
  }

  /* ── Busca ─────────────────────────────────────────────────────────────── */
  .search {
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
    transition: border-color 180ms var(--ease-out);
  }
  .search::placeholder {
    color: var(--text-muted);
  }
  .search:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  /* ── Breadcrumb ────────────────────────────────────────────────────────── */
  .crumbs {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    overflow-x: auto;
    white-space: nowrap;
    scrollbar-width: none;
  }
  .crumbs::-webkit-scrollbar {
    display: none;
  }

  .crumb {
    min-height: 0;
    min-width: 0;
    height: 28px;
    padding: 0 var(--space-2);
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    color: var(--accent);
    flex-shrink: 0;
  }
  .crumb:active {
    background: var(--bg-hover);
  }

  .crumb-sep {
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .use-here {
    align-self: flex-start;
    height: 36px;
    min-height: 36px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-md);
    border: 1px solid var(--border-default);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    font-weight: 500;
  }
  .use-here:active {
    background: var(--bg-hover);
  }

  /* ── Linhas de subpasta ────────────────────────────────────────────────── */
  .rows {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    max-height: 46vh;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .row {
    display: flex;
    align-items: stretch;
    gap: var(--space-1);
    border-radius: var(--radius-md);
  }
  /* Pasta escolhida (desktop de dois painéis): mesma tinta de seleção dos chips. */
  .row--sel {
    background: var(--accent-dim);
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  /* Corpo: acao primaria = selecionar este caminho como cwd. */
  .row-body {
    flex: 1;
    min-width: 0;
    min-height: 56px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    justify-content: center;
    gap: 2px;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    text-align: left;
    background: transparent;
    transition: background 160ms var(--ease-out);
  }
  .row-body:active {
    background: var(--bg-hover);
  }

  .row-name {
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-path {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-muted);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-badges {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: 2px;
  }

  .badge {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 2px 6px;
    border-radius: var(--radius-full);
    color: var(--text-secondary);
    background: var(--bg-hover);
  }
  .badge--git {
    color: var(--accent);
    background: var(--accent-dim);
  }
  .badge--cl {
    color: var(--warning);
    background: rgba(255, 159, 10, 0.14);
  }

  .row-time {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }

  /* Chevron: desce um nivel (drill). Alvo de 44px. */
  .drill {
    flex-shrink: 0;
    width: 44px;
    border-radius: var(--radius-md);
    color: var(--text-muted);
    background: transparent;
  }
  .drill:active {
    background: var(--bg-hover);
    color: var(--text-secondary);
  }

  /* ── Skeleton + estados ────────────────────────────────────────────────── */
  .row-skel {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-height: 56px;
    justify-content: center;
    padding: var(--space-2) var(--space-3);
  }

  .skel-line {
    height: 10px;
    border-radius: var(--radius-full);
    background: var(--bg-hover);
    animation: skel-pulse 1.2s ease-in-out infinite;
  }
  .skel-name {
    width: 45%;
  }
  .skel-path {
    width: 70%;
    height: 8px;
  }

  @keyframes skel-pulse {
    0%, 100% { opacity: 0.45; }
    50%      { opacity: 0.85; }
  }

  .state-msg {
    font-size: var(--text-sm);
    color: var(--text-muted);
    text-align: center;
    padding: var(--space-5) var(--space-3);
  }
</style>
