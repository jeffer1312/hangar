<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { getBranches, checkoutBranch, gitAction } from '../lib/api';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  let branches = $state<string[]>([]);
  let current = $state<string | null>(null);
  let loading = $state(false);
  let busy = $state('');        // branch sendo trocada OU acao rodando (trava os botoes)
  let error = $state('');
  let output = $state('');      // saida do status/pull

  async function load() {
    loading = true;
    error = '';
    output = '';
    try {
      const r = await getBranches(sessionName);
      branches = r.branches;
      current = r.current;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Falha ao listar branches';
    } finally {
      loading = false;
    }
  }

  // Recarrega a cada abertura (o estado do repo pode ter mudado fora do app).
  $effect(() => {
    if (open) load();
  });

  async function pick(b: string) {
    if (b === current || busy) return;
    busy = b;
    error = '';
    output = '';
    try {
      const r = await checkoutBranch(sessionName, b);
      current = r.current;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Falha ao trocar branch';
    } finally {
      busy = '';
    }
  }

  async function runAction(action: 'status' | 'pull') {
    if (busy) return;
    busy = action;
    error = '';
    output = '';
    try {
      const r = await gitAction(sessionName, action);
      output = r.output || (r.ok ? 'ok' : 'sem saída');
      if (action === 'pull') await load();   // pull pode mover a branch/estado
    } catch (e) {
      error = e instanceof Error ? e.message : 'Falha na ação';
    } finally {
      busy = '';
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Git">
  <div class="git">
    <h2 class="git-title">Git</h2>

    <div class="git-actions">
      <button class="git-act" disabled={!!busy} onclick={() => runAction('status')}>status</button>
      <button class="git-act" disabled={!!busy} onclick={() => runAction('pull')}>pull</button>
    </div>

    {#if loading}
      <p class="git-muted">carregando…</p>
    {:else}
      <div class="git-branches">
        {#each branches as b (b)}
          <button class="git-branch" class:current={b === current} disabled={!!busy} onclick={() => pick(b)}>
            <span class="git-dot" aria-hidden="true">{b === current ? '●' : '○'}</span>
            <span class="git-name">{b}</span>
            {#if busy === b}<span class="git-spin" aria-hidden="true">…</span>{/if}
          </button>
        {/each}
        {#if !branches.length}<p class="git-muted">nenhuma branch local</p>{/if}
      </div>
    {/if}

    {#if output}<pre class="git-output">{output}</pre>{/if}
    {#if error}<p class="git-error">{error}</p>{/if}
  </div>
</BottomSheet>

<style>
  .git { display: flex; flex-direction: column; gap: var(--space-3); padding: var(--space-2) 0; }
  .git-title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); margin: 0; }
  .git-actions { display: flex; gap: var(--space-2); }
  .git-act {
    flex: 1; padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-secondary); font-size: var(--text-sm); font-family: var(--font-mono);
    cursor: pointer;
  }
  .git-act:disabled { opacity: 0.5; cursor: default; }
  .git-branches { display: flex; flex-direction: column; gap: 2px; }
  .git-branch {
    display: flex; align-items: center; gap: var(--space-2);
    padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .git-branch:disabled { cursor: default; }
  .git-branch.current { color: var(--text-primary); background: var(--bg-elevated); }
  .git-dot { font-size: 10px; color: var(--accent); flex-shrink: 0; }
  .git-name { font-family: var(--font-mono); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .git-spin { margin-left: auto; color: var(--text-muted); }
  .git-output {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--bg-elevated); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: pre-wrap; word-break: break-all; max-height: 200px; overflow: auto;
  }
  .git-error { margin: 0; font-size: var(--text-sm); color: var(--error); }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
</style>
