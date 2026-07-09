<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import GitToolbar from './git/GitToolbar.svelte';
  import BranchList from './git/BranchList.svelte';
  import ChangedFiles from './git/ChangedFiles.svelte';
  import CommitList from './git/CommitList.svelte';
  import CommitDetail from './git/CommitDetail.svelte';
  import DiffView from './git/DiffView.svelte';
  import { getFileDiff, type GitCommit } from '../lib/api';
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

  let filter = $state('');      // busca que filtra as branches (BranchList so le, o input mora aqui)
  // View ativa: cada uma OCUPA a sheet (push-view). O "voltar" leva de volta a 'list'; o diff/commit
  // e alcancado a partir da lista/log. Enum em vez de booleanos soltos -> um so caminho de render.
  type GitView = 'list' | 'log' | 'diff' | 'commit';
  let view = $state<GitView>('list');
  let diffPath = $state('');    // arquivo aberto no diff viewer (qual)
  let diffRows = $state<DiffRow[]>([]);   // diff tokenizado (Shiki) pra render
  let diffLoading = $state(false);
  let logLoading = $state(false);
  let commitSel = $state<GitCommit | null>(null);  // commit aberto no detalhe (view 'commit')

  function cleanErr(e: unknown): string {
    const m = e instanceof Error ? e.message : 'falhou';
    return m.replace(/^\d+:\s*/, '');   // tira o prefixo "409: " do status HTTP
  }

  // Recarrega a cada abertura (o estado do repo pode ter mudado fora do app). Fecha o diff/busca.
  $effect(() => {
    if (open) { filter = ''; view = 'list'; diffPath = ''; git.load(); }
  });

  async function openDiff(path: string) {
    if (git.busy) return;
    diffPath = path;
    diffRows = [];
    diffLoading = true;
    git.error = '';
    git.busy = path;
    view = 'diff';
    try {
      const { diff } = await getFileDiff(sessionName, path);
      const { highlightDiff } = await import('../lib/highlight');   // Shiki carrega on-demand
      diffRows = await highlightDiff(diff, path);
    } catch (e) {
      git.error = cleanErr(e);
      diffPath = '';
      view = 'list';   // falhou -> volta pra lista
    } finally {
      diffLoading = false;
      git.busy = '';
    }
  }

  // Carrega o log e abre a view dedicada (uma-linha-por-commit). Espelha o openDiff.
  async function openLog() {
    if (git.busy) return;
    view = 'log';
    logLoading = true;
    try {
      await git.openLog();
      if (git.error) view = 'list';
    } finally {
      logLoading = false;
    }
  }

  // null = a linha sintetica "Working tree changes" (sem detalhe dedicado ainda -> volta pra lista,
  // onde as mudancas do working tree ja aparecem via ChangedFiles); commit real -> abre o detalhe.
  function selectCommit(c: GitCommit | null) {
    if (c) { commitSel = c; view = 'commit'; } else { view = 'list'; }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Git" resizable>
  {#if view === 'diff'}
    <!-- Visualizador de diff: ocupa a sheet no lugar da lista (volta pelo botao). -->
    <div class="git">
      <button class="git-back" onclick={() => (view = 'list')} aria-label="Voltar">‹ voltar</button>
      <DiffView path={diffPath} rows={diffRows} loading={diffLoading} />
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
        <CommitList commits={git.commits} onSelect={selectCommit} wtCount={git.files.length} />
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
        <CommitDetail commit={commitSel} />
      {/if}
    </div>
  {:else}
    <div class="git">
      <!-- HEADER FIXO: titulo + acoes + busca nao rolam junto com a lista -->
      <div class="git-head">
        <h2 class="git-title">Git</h2>
        <GitToolbar {git} onLog={openLog} />
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
          <ChangedFiles {git} onOpenDiff={openDiff} />
          <BranchList {git} {filter} />
        </div>
      {/if}

      {#if git.output}<pre class="git-output">{git.output}</pre>{/if}
      {#if git.error}<p class="git-error">{git.error}</p>{/if}
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
  /* Toolbar de voltar (log/commit) ganha uma costura fina do conteudo abaixo. Na view de diff o
     .git-back fica fora do .git-head (ver DiffView.svelte), que tem a sua propria costura. */
  .git-head:has(.git-back) { padding-bottom: var(--space-2); border-bottom: 1px solid var(--border-subtle); }
  .git-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); margin: 0; }

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

  .git-back {
    align-self: flex-start; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer;
  }
  @media (hover: hover) { .git-back:hover { background: var(--bg-hover); } }
  .git-diff-name {
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }

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
</style>
