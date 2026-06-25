<script lang="ts">
  import { onMount } from 'svelte';
  import { getRoots, scanDir } from '../lib/api';
  import { relativeTime } from '../lib/format';
  import type { FsRoot, FsEntry, FsScanError } from '../lib/types';

  // Scanner mobile de pastas de projeto: chips de raiz + busca + coluna tappavel de
  // drill-in. Toque no corpo da linha SELECIONA o caminho como cwd; o chevron desce um
  // nivel (re-scan). Breadcrumb de uma linha pra subir. A allowlist e validada no backend.
  interface Props {
    onPick: (path: string) => void;
  }
  let { onPick }: Props = $props();

  const LAST_ROOT_KEY = 'cp:last-root';

  let roots = $state<FsRoot[]>([]);
  let rootsLoading = $state(true);
  let rootsError = $state(false);
  let activeRoot = $state<FsRoot | null>(null);
  let path = $state('');                 // diretorio atual (default = raiz)
  let entries = $state<FsEntry[]>([]);
  let scanning = $state(false);
  let scanError = $state<FsScanError | null>(null);
  let query = $state('');

  // ── Carrega as raizes (chips) ──────────────────────────────────────────────
  onMount(async () => {
    try {
      roots = await getRoots();
    } catch {
      rootsError = true;
      rootsLoading = false;
      return;
    }
    rootsLoading = false;
    if (roots.length === 0) return;
    const last = localStorage.getItem(LAST_ROOT_KEY);
    selectRoot(roots.find((r) => r.path === last) ?? roots[0]);
  });

  function selectRoot(r: FsRoot) {
    activeRoot = r;
    try {
      localStorage.setItem(LAST_ROOT_KEY, r.path);
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
    const res = await scanDir(root, target);
    // descarta respostas obsoletas se o usuario navegou rapido pra outra pasta/raiz
    if (activeRoot?.path !== root || path !== target) return;
    entries = res.entries;
    scanError = res.error ?? null;
    scanning = false;
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

  // Caminho exibido relativo ao PAI da raiz, ex: "pessoal/claude-pocket".
  function relPath(p: string): string {
    if (!activeRoot) return p;
    const parent = activeRoot.path.replace(/\/[^/]+$/, '');
    return parent && p.startsWith(parent + '/') ? p.slice(parent.length + 1) : p;
  }

  // ── Busca: filtra os filhos ja carregados (nome + caminho relativo) ─────────
  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter(
      (e) => e.name.toLowerCase().includes(q) || relPath(e.path).toLowerCase().includes(q),
    );
  });

  const SCAN_MSG: Record<FsScanError, string> = {
    permission_denied: 'Sem permissão para ler esta pasta.',
    unreadable: 'Não foi possível ler esta pasta.',
    root_not_allowed: 'Esta raiz não está liberada.',
    invalid_path: 'Caminho inválido.',
    not_found: 'Pasta não encontrada.',
    unknown: 'Não foi possível ler a pasta.',
  };
</script>

<div class="scanner">
  <!-- Chips de raiz -->
  {#if rootsLoading}
    <div class="chips">
      <span class="chip chip--skel"></span>
      <span class="chip chip--skel"></span>
    </div>
  {:else if rootsError}
    <p class="state-msg">Não foi possível carregar as raízes.</p>
  {:else if roots.length === 0}
    <p class="state-msg">Nenhuma raiz configurada. Defina CP_SCAN_ROOTS no backend.</p>
  {:else}
    <div class="chips" role="tablist" aria-label="Raízes">
      {#each roots as r (r.path)}
        <button
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
      placeholder="Buscar pasta"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      aria-label="Buscar pasta"
    />

    <!-- Breadcrumb (so quando aprofundou): toque numa migalha sobe -->
    {#if drilled}
      <div class="crumbs" aria-label="Caminho">
        {#each crumbs as c, i (c.path)}
          {#if i > 0}<span class="crumb-sep" aria-hidden="true">/</span>{/if}
          <button class="crumb" onclick={() => scan(c.path)}>{c.label}</button>
        {/each}
      </div>
      <button class="use-here" onclick={() => onPick(path)}>
        Usar esta pasta
      </button>
    {/if}

    <!-- Coluna de subpastas -->
    <div class="rows" role="list">
      {#if scanning}
        {#each Array(5) as _, i (i)}
          <div class="row-skel" aria-hidden="true">
            <span class="skel-line skel-name"></span>
            <span class="skel-line skel-path"></span>
          </div>
        {/each}
      {:else if scanError}
        <p class="state-msg">{SCAN_MSG[scanError]}</p>
      {:else if filtered.length === 0}
        <p class="state-msg">
          {query.trim() ? 'Nenhuma pasta corresponde à busca.' : 'Esta pasta não tem subpastas.'}
        </p>
      {:else}
        {#each filtered as e (e.path)}
          <div class="row" role="listitem">
            <button class="row-body" onclick={() => onPick(e.path)}>
              <span class="row-name">{e.name}</span>
              <span class="row-path">{relPath(e.path)}</span>
              <span class="row-badges">
                {#if e.is_git}<span class="badge badge--git">git</span>{/if}
                {#if e.has_claude_md}<span class="badge badge--cl">CLAUDE.md</span>{/if}
                {#if e.mtime}<span class="row-time">{relativeTime(e.mtime)}</span>{/if}
              </span>
            </button>
            <button class="drill" onclick={() => drill(e)} aria-label="Abrir {e.name}">
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
  .scanner {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

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
