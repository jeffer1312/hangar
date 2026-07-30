<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';
  interface Props { git: GitStore }
  let { git }: Props = $props();

  // Espelha a query do store: quando load() zera logQuery (reabrir a sheet), o campo acompanha em
  // vez de mostrar a busca velha sobre uma lista completa.
  let q = $state('');
  $effect(() => { q = git.logQuery; });
</script>

<form class="git-search" onsubmit={(e) => { e.preventDefault(); git.searchLog(q.trim()); }}>
  <input class="git-search-input" bind:value={q} placeholder="buscar na mensagem dos commits…"
    autocapitalize="off" autocorrect="off" spellcheck="false" />
  <button type="submit" class="git-mini" disabled={!!git.busy}>buscar</button>
  {#if git.logQuery}
    <button type="button" class="git-mini" onclick={() => git.searchLog('')}>limpar</button>
  {/if}
</form>
{#if git.logQuery}
  <p class="git-muted">resultados de "{git.logQuery}" — o grafo fica oculto enquanto a busca está ativa</p>
{/if}

<style>
  .git-search { display: flex; gap: var(--space-2); }
  .git-search-input {
    flex: 1; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base);
    color: var(--text-primary); font-size: var(--text-sm); font-family: var(--font-mono);
  }
  .git-search-input::placeholder { color: var(--text-muted); }
  .git-mini {
    flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer;
  }
  .git-mini:disabled { opacity: 0.5; cursor: default; }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
</style>
