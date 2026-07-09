<script lang="ts">
  import BranchList from './git/BranchList.svelte';
  import ChangedFiles from './git/ChangedFiles.svelte';
  import CommitList from './git/CommitList.svelte';
  import CommitDetail from './git/CommitDetail.svelte';
  import CommitBox from './git/CommitBox.svelte';
  import DiffView from './git/DiffView.svelte';
  import GitToolbar from './git/GitToolbar.svelte';
  import type { GitCommit } from '../lib/api';
  import { getFileDiff, getCommitFileDiff } from '../lib/api';
  import type { DiffRow } from '../lib/highlight';
  import type { GitStore } from '../lib/gitStore.svelte';

  interface Props { git: GitStore; }
  let { git }: Props = $props();

  // Selecao no centro: null = "Working tree changes" (commit box); um commit = detalhe.
  let selected = $state<GitCommit | null | undefined>(undefined);  // undefined = nada selecionado ainda
  let diffPath = $state('');
  let diffRows = $state<DiffRow[]>([]);
  let diffLoading = $state(false);
  let diffSha = $state('');  // sha do commit dono do diffPath aberto ('' = diff da working tree)

  async function openWtDiff(path: string) {   // diff de arquivo na working tree
    if (git.busy) return;
    selected = undefined;  // Reset selection so diffPath wins in the if chain
    diffSha = '';
    diffPath = path;
    diffRows = [];
    diffLoading = true;
    git.busy = path;
    try {
      const { diff } = await getFileDiff(git.sessionName, path);
      const { highlightDiff } = await import('../lib/highlight');
      diffRows = await highlightDiff(diff, path);
    } finally {
      diffLoading = false;
      git.busy = '';
    }
  }

  async function openCommitDiff(sha: string, path: string) {   // diff de arquivo dentro de um commit historico
    if (git.busy) return;
    diffSha = sha;
    diffPath = path;
    diffRows = [];
    diffLoading = true;
    git.busy = path;
    try {
      const { diff } = await getCommitFileDiff(git.sessionName, sha, path);
      const { highlightDiff } = await import('../lib/highlight');
      diffRows = await highlightDiff(diff, path);
    } finally {
      diffLoading = false;
      git.busy = '';
    }
  }
</script>

<div class="gp">
  <GitToolbar {git} onLog={git.openLog} />
  <div class="gp-cols">
    <aside class="gp-left">
      <BranchList {git} filter="" />
      <ChangedFiles {git} onOpenDiff={openWtDiff} onCommit={() => { selected = null; diffPath = ''; }} />
    </aside>
    <section class="gp-center">
      <CommitList commits={git.commits} wtCount={git.files.length}
        selectedHash={selected === null ? '' : selected?.hash}
        onSelect={(c) => { selected = c; diffPath = ''; diffSha = ''; }} />
    </section>
    <section class="gp-right">
      {#if selected === null}
        <CommitBox {git} />
      {:else if selected}
        {@const sha = selected.hash}
        <CommitDetail commit={selected} sessionName={git.sessionName} onOpenFile={(p) => openCommitDiff(sha, p)} />
        {#if diffPath && diffSha}<DiffView path={diffPath} rows={diffRows} loading={diffLoading} />{/if}
      {:else if diffPath}
        <DiffView path={diffPath} rows={diffRows} loading={diffLoading} />
      {:else}
        <p class="git-muted">selecione um commit</p>
      {/if}
    </section>
  </div>
  {#if git.output}<pre class="git-output">{git.output}</pre>{/if}
  {#if git.error}<p class="git-error">{git.error}</p>{/if}
</div>

<style>
  .gp { display: flex; flex-direction: column; gap: var(--space-3); height: 100%; min-height: 0; }
  .gp-cols { display: grid; grid-template-columns: 210px 1fr 380px; gap: var(--space-3); flex: 1; min-height: 0; }
  .gp-left, .gp-center, .gp-right { min-width: 0; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; gap: var(--space-2); }
  .gp-left { border-right: 1px solid var(--border-subtle); padding-right: var(--space-2); }
  .gp-right { border-left: 1px solid var(--border-subtle); padding-left: var(--space-2); }

  .git-output {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--bg-elevated); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: pre-wrap; word-break: break-all; max-height: 160px; overflow: auto; flex-shrink: 0;
  }
  .git-error {
    margin: 0; font-size: var(--text-sm); color: var(--error);
    white-space: pre-wrap; word-break: break-word; flex-shrink: 0;
  }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
</style>
