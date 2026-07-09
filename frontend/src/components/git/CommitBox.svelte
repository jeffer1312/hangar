<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props { git: GitStore; onDone?: () => void; }
  let { git, onDone }: Props = $props();

  // Todos os arquivos alterados marcados por padrao (staged + unstaged + untracked).
  let sel = $state<Set<string>>(new Set(git.files.map((f) => f.path)));
  let message = $state('');
  const toggle = (p: string) => { sel.has(p) ? sel.delete(p) : sel.add(p); sel = new Set(sel); };
  const chosen = $derived(git.files.filter((f) => sel.has(f.path)).map((f) => f.path));
  const canCommit = $derived(!!message.trim() && chosen.length > 0 && !git.busy);

  async function doCommit(thenPush: boolean) {
    if (!canCommit) return;
    const ok = await git.doCommit(message, chosen);
    if (ok && thenPush) { const pushOk = await git.doPush(); }
    if (ok) { message = ''; onDone?.(); }
  }
</script>

<div class="cb">
  <div class="cb-files">
    {#each git.files as f (f.path)}
      <label class="cb-file">
        <input type="checkbox" checked={sel.has(f.path)} onchange={() => toggle(f.path)} />
        <span class="cb-code">{f.code.trim() || '?'}</span>
        <span class="cb-path">{f.path}</span>
      </label>
    {/each}
    {#if !git.files.length}<p class="git-muted">nada pra commitar</p>{/if}
  </div>
  <textarea class="cb-msg" bind:value={message} placeholder="mensagem do commit…" rows="3"
    autocapitalize="off" spellcheck="false"></textarea>
  <div class="cb-actions">
    <button class="cb-btn" disabled={!canCommit} onclick={() => doCommit(false)}>Commit</button>
    <button class="cb-btn primary" disabled={!canCommit} onclick={() => doCommit(true)}>Commit &amp; Push</button>
  </div>
  {#if git.error}<p class="git-error">{git.error}</p>{/if}
</div>

<style>
  .cb { display: flex; flex-direction: column; gap: var(--space-3); }
  .cb-files { display: flex; flex-direction: column; gap: 2px; max-height: 40vh; overflow-y: auto; }
  .cb-file { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) var(--space-2);
    font-size: var(--text-sm); cursor: pointer; }
  .cb-code { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); min-width: 1.4rem; }
  .cb-path { font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cb-msg { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); resize: vertical; }
  .cb-actions { display: flex; gap: var(--space-2); }
  .cb-btn { flex: 1; padding: var(--space-2); border-radius: var(--radius-md); border: 1px solid var(--border-default);
    background: var(--bg-elevated); color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer; }
  .cb-btn.primary { background: var(--accent); color: var(--bg-base); border-color: var(--accent); }
  .cb-btn:disabled { opacity: 0.5; cursor: default; }

  /* Svelte escopa CSS por componente — replica local do padrao usado em BranchList/CommitList/DiffView/GitSheet. */
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .git-error { margin: 0; font-size: var(--text-sm); color: var(--error); white-space: pre-wrap; word-break: break-word; }
</style>
