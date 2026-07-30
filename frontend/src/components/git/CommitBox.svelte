<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';
  import { getLastCommitMessage } from '../../lib/api';

  // `chosen` vem de fora: a lista de arquivos (com a selecao) e da aba Mudancas, que a desenha uma
  // vez so com checkbox E descartar na mesma linha. Este componente tinha a propria copia da lista.
  interface Props { git: GitStore; chosen: string[]; onDone?: () => void; }
  let { git, chosen, onDone }: Props = $props();

  let message = $state('');
  let amend = $state(false);
  let wantBranch = $state(false);
  let newBranch = $state('');

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
</div>

<style>
  .cb { display: flex; flex-direction: column; gap: var(--space-3); }
  .cb-msg { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--surface-inset); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); resize: vertical; }
  .cb-recent { width: 100%; padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--surface-inset); color: var(--text-secondary);
    font-size: var(--text-sm); }
  .cb-opts { display: flex; flex-direction: column; gap: var(--space-1); }
  .cb-opt { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm);
    color: var(--text-secondary); cursor: pointer; }
  .cb-branch { width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--surface-inset); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); }
  .cb-hint { margin: calc(-1 * var(--space-2)) 0 0; font-size: var(--text-xs); color: var(--text-muted); }
  .cb-actions { display: flex; gap: var(--space-2); }
  .cb-btn { flex: 1; padding: var(--space-2); border-radius: var(--radius-md); border: 1px solid var(--border-default);
    background: var(--surface-raised); color: var(--text-secondary); font-size: var(--text-sm); cursor: pointer; }
  .cb-btn.primary { background: var(--accent); color: var(--bg-base); border-color: var(--accent); }
  .cb-btn:disabled { opacity: 0.5; cursor: default; }
</style>
