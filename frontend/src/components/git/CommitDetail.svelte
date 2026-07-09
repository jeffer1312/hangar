<script lang="ts">
  import type { GitCommit } from '../../lib/api';

  interface Props {
    commit: GitCommit;
  }
  let { commit }: Props = $props();
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
</style>
