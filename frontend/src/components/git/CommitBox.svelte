<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';
  import { getLastCommitMessage } from '../../lib/api';

  interface Props { git: GitStore; onDone?: () => void; }
  let { git, onDone }: Props = $props();

  // Todos os arquivos alterados marcados por padrao (staged + unstaged + untracked).
  let sel = $state<Set<string>>(new Set());
  let selectionInitialized = $state(false);
  $effect(() => {
    if (!selectionInitialized && git.files.length) {
      sel = new Set(git.files.map((f) => f.path));
      selectionInitialized = true;
    }
  });
  let message = $state('');
  let amend = $state(false);
  let wantBranch = $state(false);
  let newBranch = $state('');

  const toggle = (p: string) => { sel.has(p) ? sel.delete(p) : sel.add(p); sel = new Set(sel); };
  const chosen = $derived(git.files.filter((f) => sel.has(f.path)).map((f) => f.path));
  // amend sem arquivos marcados = so reword (o backend faz --amend --only: staged nao vaza).
  const canCommit = $derived(
    !!message.trim() && (chosen.length > 0 || amend) && !git.busy && (!wantBranch || !!newBranch.trim()),
  );

  // Mensagens recentes: conveniencia de UI por sessao, sem backend (localStorage).
  const MSG_KEY = $derived(`cp_git_msgs::${git.sessionName}`);
  let recent = $state<string[]>([]);
  $effect(() => {
    try { recent = JSON.parse(localStorage.getItem(MSG_KEY) ?? '[]'); } catch { recent = []; }
  });
  function rememberMessage(msg: string) {
    const list = [msg, ...recent.filter((m) => m !== msg)].slice(0, 10);
    recent = list;
    try { localStorage.setItem(MSG_KEY, JSON.stringify(list)); } catch { /* modo privado/quota: nao trava o commit */ }
  }

  async function toggleAmend() {
    amend = !amend;
    if (amend && !message.trim()) {
      try { message = (await getLastCommitMessage(git.sessionName)).message; }
      catch { /* repo sem HEAD: o commit devolve o 409 do backend, falha aparece la */ }
    }
  }

  async function doCommit(thenPush: boolean) {
    if (!canCommit) return;
    const ok = await git.doCommit(message, chosen, {
      amend, newBranch: wantBranch ? newBranch.trim() : undefined,
    });
    // Push de amend exigiria --force (proibido) -> o botao Commit & Push some com amend marcado.
    if (ok && thenPush && !amend) { await git.doPush(); }
    if (ok) {
      rememberMessage(message.trim());
      message = ''; amend = false; wantBranch = false; newBranch = '';
      onDone?.();
    }
  }
</script>

<div class="cb">
  <div class="cb-sel-row">
    <button class="git-mini" onclick={() => (sel = new Set(git.files.map((f) => f.path)))}>todos</button>
    <button class="git-mini" onclick={() => (sel = new Set())}>nenhum</button>
  </div>
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
  {#if recent.length}
    <select class="cb-recent" value="" onchange={(e) => { const v = e.currentTarget.value; if (v) message = v; e.currentTarget.value = ''; }}>
      <option value="">mensagens recentes…</option>
      {#each recent as r (r)}<option value={r}>{r.length > 72 ? r.slice(0, 72) + '…' : r}</option>{/each}
    </select>
  {/if}
  <textarea class="cb-msg" bind:value={message} placeholder="mensagem do commit…" rows="3"
    autocapitalize="off" spellcheck="false"></textarea>
  <div class="cb-opts">
    <label class="cb-opt"><input type="checkbox" checked={amend} onchange={toggleAmend} /> reescrever o último commit (amend)</label>
    <label class="cb-opt"><input type="checkbox" bind:checked={wantBranch} /> commitar numa branch nova</label>
  </div>
  {#if wantBranch}
    <input class="cb-branch" bind:value={newBranch} placeholder="nome da branch nova"
      autocapitalize="off" autocorrect="off" spellcheck="false" />
    <p class="cb-hint">cria a branch a partir da atual antes de commitar</p>
  {/if}
  <div class="cb-actions">
    <button class="cb-btn" disabled={!canCommit} onclick={() => doCommit(false)}>Commit</button>
    {#if !amend}
      <button class="cb-btn primary" disabled={!canCommit} onclick={() => doCommit(true)}>Commit &amp; Push</button>
    {/if}
  </div>
  {#if git.error}<p class="git-error">{git.error}</p>{/if}
</div>

<style>
  .cb { display: flex; flex-direction: column; gap: var(--space-3); }
  .cb-sel-row { display: flex; gap: var(--space-2); justify-content: flex-end; }
  .cb-files { display: flex; flex-direction: column; gap: 2px; max-height: 40vh; overflow-y: auto; }
  .cb-file { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) var(--space-2);
    font-size: var(--text-sm); cursor: pointer; }
  .cb-code { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); min-width: 1.4rem; }
  .cb-path { font-family: var(--font-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cb-msg { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); resize: vertical; }
  .cb-recent { width: 100%; padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-secondary);
    font-size: var(--text-sm); }
  .cb-opts { display: flex; flex-direction: column; gap: var(--space-1); }
  .cb-opt { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);
    color: var(--text-secondary); cursor: pointer; }
  .cb-branch { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); }
  .cb-hint { margin: calc(-1 * var(--space-2)) 0 0; font-size: var(--text-xs); color: var(--text-muted); }
  .cb-actions { display: flex; gap: var(--space-2); }
  .cb-btn { flex: 1; padding: var(--space-2); border-radius: var(--radius-md); border: 1px solid var(--border-default);
    background: var(--bg-elevated); color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer; }
  .cb-btn.primary { background: var(--accent); color: var(--bg-base); border-color: var(--accent); }
  .cb-btn:disabled { opacity: 0.5; cursor: default; }

  /* Svelte escopa CSS por componente — replicas locais dos padroes de ChangedFiles/CommitList. */
  .git-mini { flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer; }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .git-error { margin: 0; font-size: var(--text-sm); color: var(--error); white-space: pre-wrap; word-break: break-word; }
</style>
