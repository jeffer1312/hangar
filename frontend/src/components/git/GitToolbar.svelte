<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props {
    git: GitStore;
    onLog: () => void;
  }
  let { git, onLog }: Props = $props();

  let confirmAbort = $state(false);

  // So reseta o confirm no SUCESSO: recusado pelo git, o botao "confirmar abort" continua a vista
  // com o erro embaixo (mesmo padrao do confirmDiscard). Sem isto, um SEGUNDO conflito mais tarde
  // na mesma sheet aberta reapareceria ja em "confirmar abort", pulando o passo de aviso.
  async function doAbort() {
    if (await git.abortOp()) confirmAbort = false;
  }
</script>

<div class="git-actions">
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('status')}>status</button>
  <button class="git-act" disabled={!!git.busy} onclick={onLog} title="últimos commits (git log)">log</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('fetch')}>fetch</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('pull')}>pull</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.doPush()} title="envia os commits (git push)">push</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('stash')} title="guarda as mudanças (git stash)">stash</button>
  <button class="git-act" disabled={!!git.busy} onclick={() => git.runAction('stash-pop')} title="reaplica o último stash">pop</button>
  {#if git.pendingAbort}
    {#if confirmAbort}
      <button class="git-act git-abort" disabled={!!git.busy} onclick={doAbort}>confirmar abort</button>
      <button class="git-act" onclick={() => (confirmAbort = false)}>não</button>
    {:else}
      <button class="git-act git-abort" disabled={!!git.busy}
        onclick={() => (confirmAbort = true)} title="desiste da operação em conflito">abort…</button>
    {/if}
  {/if}
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
  .git-abort { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }
</style>
