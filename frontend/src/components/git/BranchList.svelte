<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';

  // O input de busca (que escreve em `filter`) fica no GitSheet (header fixo, fora da area que
  // rola) — aqui so lemos o valor pra filtrar. Ver nota de fidelidade no relatorio da Task 6.
  interface Props {
    git: GitStore;
    filter: string;
  }
  let { git, filter }: Props = $props();

  const q = $derived(filter.trim().toLowerCase());
  // Branch atual sempre no topo das locais; depois a ordem por recencia que vem do backend.
  const localList = $derived.by(() => {
    const ordered = git.current ? [git.current, ...git.branches.filter((b) => b !== git.current)] : git.branches;
    return q ? ordered.filter((b) => b.toLowerCase().includes(q)) : ordered;
  });
  const remoteList = $derived(q ? git.remotes.filter((b) => b.toLowerCase().includes(q)) : git.remotes);
</script>

<div class="git-branches">
  {#each localList as b (b)}
    <button class="git-branch" class:current={b === git.current} disabled={!!git.busy} onclick={() => git.pick(b)}>
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
      <button class="git-branch git-remote" disabled={!!git.busy} onclick={() => git.pick(b)} title="cria uma branch local rastreando a remota">
        <span class="git-dot" aria-hidden="true">○</span>
        <span class="git-name">{b}</span>
        <span class="git-badge">remote</span>
        {#if git.busy === b}<span class="git-spin" aria-hidden="true">…</span>{/if}
      </button>
    {/each}
  </div>
{/if}

<style>
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .git-section {
    margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.05em;
  }

  .git-branches { display: flex; flex-direction: column; gap: 2px; }
  .git-branch {
    display: flex; align-items: center; gap: var(--space-2); width: 100%;
    padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .git-branch:disabled { cursor: default; }
  .git-branch.current { color: var(--text-primary); background: var(--bg-elevated); }
  @media (hover: hover) { .git-branch:hover { background: var(--bg-hover); } }
  .git-remote .git-dot { color: var(--text-muted); }   /* remota nao e a atual -> dot apagado */
  .git-dot { font-size: 10px; color: var(--accent); flex-shrink: 0; }
  .git-name { font-family: var(--font-mono); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
  .git-badge {
    margin-left: auto; font-size: 10px; font-family: var(--font-mono);
    color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em;
  }
  .git-spin { margin-left: auto; color: var(--text-muted); }
</style>
