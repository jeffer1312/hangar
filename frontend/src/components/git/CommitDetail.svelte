<script lang="ts">
  import { getCommitFiles, type GitCommit, type ChangedFile } from '../../lib/api';

  interface Props {
    commit: GitCommit;
    sessionName: string;
    onOpenFile: (p: string) => void;
  }
  let { commit, sessionName, onOpenFile }: Props = $props();

  let files = $state<ChangedFile[]>([]);

  // Recarrega ao trocar de commit; zera antes pra nao piscar a lista do commit anterior.
  $effect(() => {
    const h = commit.hash;
    files = [];
    getCommitFiles(sessionName, h).then((r) => { files = r.files; }).catch(() => { files = []; });
  });
</script>

<div class="git-scroll git-commit-detail">
  <p class="git-cd-subject">{commit.subject}</p>
  {#if commit.refs}<div class="git-cd-refs">{#each commit.refs.split(', ') as r (r)}<span class="git-c-ref">{r.replace('HEAD -> ', '')}</span>{/each}</div>{/if}
  <dl class="git-cd-meta">
    <dt>hash</dt><dd class="mono">{commit.hash}</dd>
    <dt>autor</dt><dd>{commit.author}</dd>
    <dt>data</dt><dd>{new Date(commit.ts * 1000).toLocaleString()} · {commit.rel}</dd>
    <dt>parents</dt><dd class="mono">{commit.parents.length ? commit.parents.map((p) => p.slice(0, 7)).join(', ') : '(root)'}</dd>
  </dl>
  <div class="git-cd-files">
    {#each files as f (f.path)}
      <button class="git-file" onclick={() => onOpenFile(f.path)} title="ver diff">
        <span class="git-file-tag">{f.code}</span><span class="git-path-base">{f.path}</span>
      </button>
    {:else}
      <p class="git-muted">nenhum arquivo alterado</p>
    {/each}
  </div>
</div>

<style>
  .git-scroll {
    overflow-y: auto; max-height: 52vh; min-height: 0;
    overscroll-behavior: contain; -webkit-overflow-scrolling: touch;
    display: flex; flex-direction: column; gap: var(--space-1);
  }
  @media (min-width: 820px) { .git-scroll { max-height: 68vh; } }

  .git-commit-detail { gap: var(--space-3); }
  .git-cd-subject { margin: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); line-height: 1.4; }
  .git-cd-refs { display: flex; flex-wrap: wrap; gap: var(--space-1); margin: 0; }
  .git-c-ref {
    flex: 0 1 auto; max-width: 40%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font-size: 10px; padding: 0 6px; border-radius: var(--radius-full);
    background: var(--accent-dim); color: var(--accent); font-family: var(--font-mono);
  }
  .git-cd-meta {
    margin: 0; display: grid; grid-template-columns: auto 1fr; gap: var(--space-1) var(--space-3);
  }
  .git-cd-meta dt { color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.04em; font-size: var(--text-xs); align-self: center; }
  .git-cd-meta dd { margin: 0; color: var(--text-secondary); font-size: var(--text-sm); word-break: break-word; }
  .git-cd-meta dd.mono { font-family: var(--font-mono); font-size: var(--text-xs); }

  .git-cd-files { display: flex; flex-direction: column; gap: 2px; }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .git-file {
    display: flex; align-items: center; gap: var(--space-2); min-width: 0;
    padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  @media (hover: hover) { .git-file:hover { background: var(--bg-hover); } }
  .git-file-tag {
    flex-shrink: 0; font-size: 10px; font-family: var(--font-mono); text-transform: uppercase;
    letter-spacing: 0.03em; color: var(--text-muted); min-width: 1.6rem;
  }
  .git-path-base { flex: 0 0 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); color: var(--text-secondary); }
</style>
