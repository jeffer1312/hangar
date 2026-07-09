<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { getFileDiff, discardFile, type GitCommit } from '../lib/api';
  import { createGitStore } from '../lib/gitStore.svelte';
  // Import de TIPO (elidido no build); a lib do Shiki entra via import() dinamico no openDiff -> o
  // core+temas viram um chunk carregado SO ao abrir um diff, sem pesar o bundle inicial do app.
  import type { DiffRow } from '../lib/highlight';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  const git = createGitStore(sessionName);

  let filter = $state('');      // busca que filtra as branches
  let confirmDiscard = $state('');  // path aguardando confirmacao de descarte
  // View ativa: cada uma OCUPA a sheet (push-view). O "voltar" leva de volta a 'list'; o diff/commit
  // e alcancado a partir da lista/log. Enum em vez de booleanos soltos -> um so caminho de render.
  type GitView = 'list' | 'log' | 'diff' | 'commit';
  let view = $state<GitView>('list');
  let diffPath = $state('');    // arquivo aberto no diff viewer (qual)
  let diffRows = $state<DiffRow[]>([]);   // diff tokenizado (Shiki) pra render
  let diffLoading = $state(false);
  let logLoading = $state(false);
  let commitSel = $state<GitCommit | null>(null);  // commit aberto no detalhe (view 'commit')

  // openDiff/doDiscard ainda nao vivem no store (proxima tarefa) — busy/error proprios, so
  // usados por eles; nas telas onde importam (view 'list'), o template combina com git.busy/git.error.
  let busy = $state('');
  let error = $state('');

  function cleanErr(e: unknown): string {
    const m = e instanceof Error ? e.message : 'falhou';
    return m.replace(/^\d+:\s*/, '');   // tira o prefixo "409: " do status HTTP
  }

  // ── Grafo de commits: coluna de lanes (dots + linhas) a esquerda de cada commit na view de log.
  // Historico linear -> uma coluna so. Cap de lanes pra nao estourar o sheet estreito.
  const GRAPH_STEP = 14, GRAPH_R = 4.5, GRAPH_CAP = 5, GRAPH_H = 34;
  const graphLanes = $derived(Math.min(GRAPH_CAP, Math.max(1, ...git.commits.flatMap((c) => [
    (c.col ?? 0) + 1,
    ...(c.passthrough ?? []).map((p) => p + 1),
    ...(c.edges ?? []).map((e) => e.to_col + 1),
  ]))));
  const graphW = $derived(graphLanes * GRAPH_STEP + 4);
  const laneX = (col: number) => Math.min(col, GRAPH_CAP - 1) * GRAPH_STEP + GRAPH_R + 3;
  const laneColor = (col: number) => `hsl(${(col * 67) % 360} 55% 62%)`;   // cor estavel por lane

  // +N / -M do diff aberto (contado do proprio diffRows; GitLens/TortoiseGit mostram no topo).
  const diffStat = $derived({
    add: diffRows.filter((r) => r.kind === 'add').length,
    del: diffRows.filter((r) => r.kind === 'del').length,
  });

  const q = $derived(filter.trim().toLowerCase());
  // Branch atual sempre no topo das locais; depois a ordem por recencia que vem do backend.
  const localList = $derived.by(() => {
    const ordered = git.current ? [git.current, ...git.branches.filter((b) => b !== git.current)] : git.branches;
    return q ? ordered.filter((b) => b.toLowerCase().includes(q)) : ordered;
  });
  const remoteList = $derived(q ? git.remotes.filter((b) => b.toLowerCase().includes(q)) : git.remotes);

  // Recarrega a cada abertura (o estado do repo pode ter mudado fora do app). Fecha o diff/busca.
  $effect(() => {
    if (open) { filter = ''; view = 'list'; diffPath = ''; confirmDiscard = ''; error = ''; git.load(); }
  });

  async function openDiff(path: string) {
    if (git.busy || busy) return;
    diffPath = path;
    diffRows = [];
    diffLoading = true;
    error = '';
    view = 'diff';
    try {
      const { diff } = await getFileDiff(sessionName, path);
      const { highlightDiff } = await import('../lib/highlight');   // Shiki carrega on-demand
      diffRows = await highlightDiff(diff, path);
    } catch (e) {
      error = cleanErr(e);
      diffPath = '';
      view = 'list';   // falhou -> volta pra lista
    } finally {
      diffLoading = false;
    }
  }

  // Carrega o log e abre a view dedicada (uma-linha-por-commit). Espelha o openDiff.
  async function openLog() {
    if (git.busy || busy) return;
    view = 'log';
    logLoading = true;
    try {
      await git.openLog();
      if (git.error) view = 'list';
    } finally {
      logLoading = false;
    }
  }

  async function doDiscard(path: string) {
    busy = path;
    error = '';
    try {
      await discardFile(sessionName, path);
      confirmDiscard = '';
      if (diffPath === path) { diffPath = ''; if (view === 'diff') view = 'list'; }
      await git.refresh();
    } catch (e) {
      error = cleanErr(e);
    } finally {
      busy = '';
    }
  }

  // Rotulo curto do status XY do porcelain (M/A/D/R/? -> palavra).
  function fileTag(code: string): string {
    const c = code.trim()[0] ?? '';
    return { M: 'mod', A: 'novo', D: 'del', R: 'ren', C: 'copia', U: 'conflito', '?': 'novo' }[c] ?? c;
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Git" resizable>
  {#if view === 'diff'}
    <!-- Visualizador de diff: ocupa a sheet no lugar da lista (volta pelo botao). -->
    <div class="git">
      <div class="git-head">
        <button class="git-back" onclick={() => (view = 'list')} aria-label="Voltar">‹ voltar</button>
        <span class="git-diff-name">{diffPath}</span>
        {#if !diffLoading && (diffStat.add || diffStat.del)}
          <span class="git-diff-stat"><span class="stat-add">+{diffStat.add}</span> <span class="stat-del">−{diffStat.del}</span></span>
        {/if}
      </div>
      {#if diffLoading}
        <p class="git-muted">carregando diff…</p>
      {:else}
        <pre class="git-diff">{#each diffRows as row, i (i)}<span
            class:add={row.kind === 'add'}
            class:del={row.kind === 'del'}
            class:hunk={row.kind === 'hunk'}
            class:meta={row.kind === 'meta'}
          >{#if row.prefix}<span class="diff-prefix">{row.prefix}</span>{/if}{#each row.tokens as t, j (j)}<span style={t.color ? `color: ${t.color}` : undefined}>{t.content}</span>{/each}</span>{/each}</pre>
      {/if}
    </div>
  {:else if view === 'log'}
    <!-- Log dedicado: uma linha por commit (hash + ref + assunto + data). Tap abre o detalhe. -->
    <div class="git">
      <div class="git-head">
        <button class="git-back" onclick={() => (view = 'list')} aria-label="Voltar">‹ voltar</button>
        <span class="git-diff-name">git log</span>
      </div>
      {#if logLoading}
        <p class="git-muted">carregando…</p>
      {:else}
        <div class="git-scroll git-log">
          {#each git.commits as c (c.hash)}
            {@const cx = laneX(c.col ?? 0)}
            <button class="git-commit" onclick={() => { commitSel = c; view = 'commit'; }} title={c.subject}>
              <svg class="git-graph" width={graphW} height={GRAPH_H} viewBox="0 0 {graphW} {GRAPH_H}" aria-hidden="true">
                <!-- lanes de OUTRAS branches que cruzam esta linha (vertical cheia, sem dot) -->
                {#each c.passthrough ?? [] as pc (pc)}
                  {@const px = laneX(pc)}
                  <line x1={px} y1="0" x2={px} y2={GRAPH_H} stroke={laneColor(pc)} stroke-width="2" />
                {/each}
                <!-- lane subindo (conecta ao commit acima na mesma coluna) -->
                <line x1={cx} y1="0" x2={cx} y2={GRAPH_H / 2} stroke={laneColor(c.col ?? 0)} stroke-width="2" />
                <!-- arestas descendo pros parents (merge = curva) -->
                {#each c.edges ?? [] as e, k (k)}
                  {@const tx = laneX(e.to_col)}
                  {#if e.curved && tx !== cx}
                    <path d="M{cx} {GRAPH_H / 2} C {cx} {GRAPH_H - 6}, {tx} {GRAPH_H / 2 + 4}, {tx} {GRAPH_H}" stroke={laneColor(e.to_col)} stroke-width="2" fill="none" />
                  {:else}
                    <line x1={cx} y1={GRAPH_H / 2} x2={tx} y2={GRAPH_H} stroke={laneColor(e.to_col)} stroke-width="2" />
                  {/if}
                {/each}
                <circle cx={cx} cy={GRAPH_H / 2} r={GRAPH_R} fill={laneColor(c.col ?? 0)} />
              </svg>
              <span class="git-c-hash">{c.short}</span>
              {#if c.refs}<span class="git-c-ref">{c.refs.split(', ')[0].replace('HEAD -> ', '')}</span>{/if}
              <span class="git-c-sub">{c.subject}</span>
              <span class="git-c-when">{c.rel}</span>
            </button>
          {/each}
          {#if !git.commits.length}<p class="git-muted">sem commits</p>{/if}
        </div>
      {/if}
    </div>
  {:else if view === 'commit'}
    <!-- Detalhe de UM commit (push-view, sem accordion): metadados completos. -->
    <div class="git">
      <div class="git-head">
        <button class="git-back" onclick={() => (view = 'log')} aria-label="Voltar">‹ voltar</button>
        <span class="git-diff-name">commit {commitSel?.short}</span>
      </div>
      {#if commitSel}
        {@const c = commitSel}
        <div class="git-scroll git-commit-detail">
          <p class="git-cd-subject">{c.subject}</p>
          {#if c.refs}<div class="git-cd-refs">{#each c.refs.split(', ') as r (r)}<span class="git-c-ref">{r.replace('HEAD -> ', '')}</span>{/each}</div>{/if}
          <dl class="git-cd-meta">
            <dt>hash</dt><dd class="mono">{c.hash}</dd>
            <dt>autor</dt><dd>{c.author}</dd>
            <dt>data</dt><dd>{new Date(c.ts * 1000).toLocaleString()} · {c.rel}</dd>
            <dt>parents</dt><dd class="mono">{c.parents.length ? c.parents.map((p) => p.slice(0, 7)).join(', ') : '(root)'}</dd>
          </dl>
        </div>
      {/if}
    </div>
  {:else}
    <div class="git">
      <!-- HEADER FIXO: titulo + acoes + busca nao rolam junto com a lista -->
      <div class="git-head">
        <h2 class="git-title">Git</h2>
        <div class="git-actions">
          <button class="git-act" disabled={!!git.busy || !!busy} onclick={() => git.runAction('status')}>status</button>
          <button class="git-act" disabled={!!git.busy || !!busy} onclick={openLog} title="últimos commits (git log)">log</button>
          <button class="git-act" disabled={!!git.busy || !!busy} onclick={() => git.runAction('fetch')}>fetch</button>
          <button class="git-act" disabled={!!git.busy || !!busy} onclick={() => git.runAction('pull')}>pull</button>
          <button class="git-act" disabled={!!git.busy || !!busy} onclick={() => git.runAction('stash')} title="guarda as mudanças (git stash)">stash</button>
          <button class="git-act" disabled={!!git.busy || !!busy} onclick={() => git.runAction('stash-pop')} title="reaplica o último stash">pop</button>
        </div>
        {#if git.branches.length > 6 || git.remotes.length}
          <input
            class="git-search"
            type="search"
            placeholder="filtrar branch…"
            bind:value={filter}
            autocapitalize="off"
            autocorrect="off"
            spellcheck="false"
          />
        {/if}
      </div>

      {#if git.loading}
        <p class="git-muted">carregando…</p>
      {:else}
        <!-- CORPO SCROLLÁVEL -->
        <div class="git-scroll">
          {#if git.dirty && git.files.length}
            <div class="git-warn">working tree suja — troque de branch só depois de commit ou stash</div>
            <p class="git-section">{git.files.length} arquivo{git.files.length > 1 ? 's' : ''} alterado{git.files.length > 1 ? 's' : ''}</p>
            <div class="git-files">
              {#each git.files as f (f.path)}
                {@const slash = f.path.lastIndexOf('/')}
                <div class="git-file-row" class:danger={confirmDiscard === f.path}>
                  <button class="git-file" disabled={!!git.busy || !!busy} onclick={() => openDiff(f.path)} title="ver diff">
                    <span class="git-file-tag" data-t={fileTag(f.code)}>{fileTag(f.code)}</span>
                    <!-- basename em destaque: o dir trunca no COMECO (direction:rtl), o basename nunca encolhe.
                         Um LRM (\u200e) no fim ancora a "/" final em contexto LTR — sem ele o rtl joga a
                         barra de borda pro comeco (bug do bidi com pontuacao neutra). -->
                    <span class="git-path">{#if slash >= 0}<span class="git-path-dir">{'\u200e' + f.path.slice(0, slash + 1) + '\u200e'}</span>{/if}<span class="git-path-base">{slash >= 0 ? f.path.slice(slash + 1) : f.path}</span></span>
                  </button>
                  {#if confirmDiscard === f.path}
                    <button class="git-mini danger" disabled={!!git.busy || !!busy} onclick={() => doDiscard(f.path)}>descartar</button>
                    <button class="git-mini" disabled={!!git.busy || !!busy} onclick={() => (confirmDiscard = '')}>não</button>
                  {:else}
                    <button class="git-mini" disabled={!!git.busy || !!busy} onclick={() => (confirmDiscard = f.path)} aria-label="descartar mudanças" title="descartar mudanças">⟲</button>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}

          <div class="git-branches">
            {#each localList as b (b)}
              <button class="git-branch" class:current={b === git.current} disabled={!!git.busy || !!busy} onclick={() => git.pick(b)}>
                <span class="git-dot" aria-hidden="true">{b === git.current ? '●' : '○'}</span>
                <span class="git-name">{b}</span>
                {#if git.busy === b}<span class="git-spin" aria-hidden="true">…</span>{/if}
              </button>
            {/each}
            {#if !localList.length}<p class="git-muted">{q ? 'nenhuma branch local com esse filtro' : 'nenhuma branch local'}</p>{/if}
          </div>

          {#if remoteList.length}
            <p class="git-section">remotas</p>
            <div class="git-branches">
              {#each remoteList as b (b)}
                <button class="git-branch git-remote" disabled={!!git.busy || !!busy} onclick={() => git.pick(b)} title="cria uma branch local rastreando a remota">
                  <span class="git-dot" aria-hidden="true">○</span>
                  <span class="git-name">{b}</span>
                  <span class="git-badge">remote</span>
                  {#if git.busy === b}<span class="git-spin" aria-hidden="true">…</span>{/if}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}

      {#if git.output}<pre class="git-output">{git.output}</pre>{/if}
      {#if git.error || error}<p class="git-error">{git.error || error}</p>{/if}
    </div>
  {/if}
</BottomSheet>

<style>
  .git {
    display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; min-height: 0;
    /* push-view: cada troca (lista<->log<->diff<->commit) desliza pra dentro, reforcando o "voltar".
       O prefers-reduced-motion global do app.css neutraliza -> sem media query aqui. */
    animation: view-in 200ms var(--ease-out) both;
  }
  @keyframes view-in { from { opacity: 0; transform: translateX(12px); } to { opacity: 1; transform: translateX(0); } }

  /* Header fixo: nao rola com a lista. */
  .git-head { display: flex; flex-direction: column; gap: var(--space-2); flex-shrink: 0; }
  /* Toolbar de voltar (log/diff/commit) ganha uma costura fina do conteudo abaixo. */
  .git-head:has(.git-back) { padding-bottom: var(--space-2); border-bottom: 1px solid var(--border-subtle); }
  .git-diff-stat { flex: 0 0 auto; font-family: var(--font-mono); font-size: var(--text-xs); }
  .git-diff-stat .stat-add { color: var(--success); }
  .git-diff-stat .stat-del { color: var(--error); }
  .git-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); margin: 0; }

  .git-actions { display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .git-act {
    flex: 1 1 auto; min-width: 4rem; padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-secondary); font-size: var(--text-sm); font-family: var(--font-mono);
    cursor: pointer;
  }
  .git-act:disabled { opacity: 0.5; cursor: default; }

  .git-search {
    width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base);
    color: var(--text-primary); font-size: var(--text-sm); font-family: var(--font-mono);
  }
  .git-search::placeholder { color: var(--text-muted); }

  /* So a lista rola. max-height limita a altura (o sheet no mobile nao tem overflow proprio). */
  .git-scroll {
    overflow-y: auto; max-height: 52vh; min-height: 0;
    overscroll-behavior: contain; -webkit-overflow-scrolling: touch;
    display: flex; flex-direction: column; gap: var(--space-1);
  }
  @media (min-width: 820px) { .git-scroll { max-height: 68vh; } }

  .git-warn {
    padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--warning, #d9a441) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--warning, #d9a441) 40%, transparent);
    color: var(--text-secondary); font-size: var(--text-xs); line-height: 1.4;
  }

  .git-section {
    margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.05em;
  }

  /* ── arquivos alterados ── */
  .git-files { display: flex; flex-direction: column; gap: 2px; }
  .git-file-row { display: flex; align-items: center; gap: var(--space-2); }
  .git-file-row.danger { background: color-mix(in srgb, var(--error) 12%, transparent); border-radius: var(--radius-md); }
  .git-file {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2);
    padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .git-file:disabled { cursor: default; }
  .git-file-tag {
    flex-shrink: 0; font-size: 10px; font-family: var(--font-mono); text-transform: uppercase;
    letter-spacing: 0.03em; color: var(--text-muted); min-width: 2.4rem;
  }
  .git-file-tag[data-t="novo"] { color: var(--accent); }
  .git-file-tag[data-t="del"] { color: var(--error); }
  .git-mini {
    flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer;
  }
  .git-mini:disabled { opacity: 0.5; cursor: default; }
  .git-mini.danger { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }

  /* ── branches ── */
  .git-branches { display: flex; flex-direction: column; gap: 2px; }
  .git-branch {
    display: flex; align-items: center; gap: var(--space-2); width: 100%;
    padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .git-branch:disabled { cursor: default; }
  .git-branch.current { color: var(--text-primary); background: var(--bg-elevated); }
  .git-remote .git-dot { color: var(--text-muted); }   /* remota nao e a atual -> dot apagado */
  .git-dot { font-size: 10px; color: var(--accent); flex-shrink: 0; }
  .git-name { font-family: var(--font-mono); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
  /* Path do arquivo: basename em destaque + dir menor. O dir trunca no COMECO (direction:rtl deixa a
     ellipsis no inicio, mantendo o fim do dir + o basename visiveis); o basename nunca encolhe. */
  .git-path { display: flex; min-width: 0; align-items: baseline; font-family: var(--font-mono); }
  .git-path-dir { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; direction: rtl; color: var(--text-muted); font-size: var(--text-xs); }
  .git-path-base { flex: 0 0 auto; white-space: nowrap; color: var(--text-secondary); }
  .git-badge {
    margin-left: auto; font-size: 10px; font-family: var(--font-mono);
    color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em;
  }
  .git-spin { margin-left: auto; color: var(--text-muted); }

  /* ── diff viewer ── */
  .git-back {
    align-self: flex-start; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer;
  }
  .git-diff-name {
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .git-diff {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--bg-base); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.5;
    max-height: 62vh; overflow: auto; white-space: pre;
  }
  /* So os filhos DIRETOS sao linhas (block); os tokens do Shiki dentro delas ficam inline. */
  .git-diff > span { display: block; }
  /* Fundo tingido por linha (add/del) + cor default do prefixo/fallback. O codigo em si recebe a cor
     inline dos tokens do Shiki (tema VS Code); a cor abaixo so pinta o prefixo +/- e o modo sem-highlight. */
  .git-diff .add { color: var(--success); background: color-mix(in srgb, var(--success) 10%, transparent); }
  .git-diff .del { color: var(--error); background: color-mix(in srgb, var(--error) 10%, transparent); }
  .git-diff .hunk { color: var(--accent); }
  .git-diff .meta { color: var(--text-muted); }
  .git-diff .diff-prefix { opacity: 0.7; user-select: none; }

  .git-output {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--bg-elevated); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: pre-wrap; word-break: break-all; max-height: 200px; overflow: auto; flex-shrink: 0;
  }
  .git-error {
    margin: 0; font-size: var(--text-sm); color: var(--error);
    white-space: pre-wrap; word-break: break-word; flex-shrink: 0;
  }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }

  /* ── log: uma linha por commit (sem wrap; assunto com ellipsis) ── */
  .git-log { gap: 0; }
  .git-commit {
    display: flex; align-items: center; gap: var(--space-2); width: 100%;
    padding: 0 var(--space-2); min-height: 44px; border-radius: var(--radius-md);
    background: transparent; text-align: left; cursor: pointer;
  }
  .git-commit:active { background: var(--bg-elevated); }
  /* Feedback de mouse (desktop dock) — sem isto so havia :active (touch). Mesmo gate do Sidebar. */
  @media (hover: hover) {
    .git-commit:hover, .git-branch:hover, .git-file:hover, .git-back:hover { background: var(--bg-hover); }
  }
  /* Coluna do grafo: dots + linhas de lane. overflow visible pra curva de merge nao cortar. */
  .git-graph { flex: 0 0 auto; overflow: visible; }
  /* Hash e o campo MENOS importante -> muted; o assunto lidera (primary), estilo GitLens/TortoiseGit. */
  .git-c-hash { flex: 0 0 auto; font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); }
  .git-c-ref {
    flex: 0 1 auto; max-width: 40%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font-size: 10px; padding: 0 6px; border-radius: var(--radius-full);
    background: var(--accent-dim); color: var(--accent); font-family: var(--font-mono);
  }
  .git-c-sub {
    flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font-size: var(--text-sm); color: var(--text-primary);
  }
  .git-c-when { flex: 0 0 auto; font-size: var(--text-xs); color: var(--text-muted); white-space: nowrap; }

  /* ── detalhe de commit ── */
  .git-commit-detail { gap: var(--space-3); }
  .git-cd-subject { margin: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); line-height: 1.4; }
  .git-cd-refs { display: flex; flex-wrap: wrap; gap: var(--space-1); margin: 0; }
  .git-cd-meta {
    margin: 0; display: grid; grid-template-columns: auto 1fr; gap: var(--space-1) var(--space-3);
  }
  .git-cd-meta dt { color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.04em; font-size: var(--text-xs); align-self: center; }
  .git-cd-meta dd { margin: 0; color: var(--text-secondary); font-size: var(--text-sm); word-break: break-word; }
  .git-cd-meta dd.mono { font-family: var(--font-mono); font-size: var(--text-xs); }
</style>
