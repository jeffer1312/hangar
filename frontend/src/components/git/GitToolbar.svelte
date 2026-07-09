<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props {
    git: GitStore;
    onLog: () => void;
  }
  let { git, onLog }: Props = $props();
</script>

<div class="git-actions">
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('status')}>status</button>
  <button class="git-act" disabled={!!git.busy} onclick={onLog} title="últimos commits (git log)">log</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('fetch')}>fetch</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('pull')}>pull</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('stash')} title="guarda as mudanças (git stash)">stash</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('stash-pop')} title="reaplica o último stash">pop</button>
</div>

<style>
  .git-actions { display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .git-act {
    flex: 1 1 auto; min-width: 4rem; padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-secondary); font-size: var(--text-sm); font-family: var(--font-mono);
    cursor: pointer;
  }
  .git-act:disabled { opacity: 0.5; cursor: default; }
</style>
