<script lang="ts">
  // Metade de cima do que era o CommitDetail: a mensagem e os metadados do commit. SEM max-height
  // proprio — quem limita altura agora e o empilhado da aba, senao dois paineis brigariam pelo
  // mesmo 52vh e a soma estouraria a tela.
  import type { GitCommit } from '../../lib/api';

  interface Props { commit: GitCommit }
  let { commit }: Props = $props();
</script>

<div class="cm">
  <p class="cm-subject">{commit.subject}</p>
  {#if commit.refs}
    <div class="cm-refs">{#each commit.refs.split(', ') as r (r)}<span class="git-c-ref">{r.replace('HEAD -> ', '')}</span>{/each}</div>
  {/if}
  <!-- Corpo da mensagem (o `%b`, que so passou a vir do backend na Task 1). Vazio SOME: um bloco em
       branco entre o assunto e os metadados leria como "faltou carregar". -->
  {#if commit.body?.trim()}
    <p class="cm-body">{commit.body.trim()}</p>
  {/if}
  <dl class="cm-meta">
    <dt>hash</dt><dd class="mono">{commit.hash}</dd>
    <dt>autor</dt><dd>{commit.author}</dd>
    <dt>data</dt><dd>{new Date(commit.ts * 1000).toLocaleString()} · {commit.rel}</dd>
    <dt>parents</dt><dd class="mono">{commit.parents.length ? commit.parents.map((p) => p.slice(0, 7)).join(', ') : '(root)'}</dd>
  </dl>
</div>

<style>
  .cm { display: flex; flex-direction: column; gap: var(--space-3); min-height: 0; }
  .cm-subject { margin: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); line-height: 1.4; }
  /* pre-wrap: a mensagem de commit tem quebra de linha com significado (lista de itens, rodape de
     trailer). O texto ainda quebra sozinho quando a linha e longa demais pro painel. */
  .cm-body {
    margin: 0; white-space: pre-wrap; overflow-wrap: anywhere;
    font-size: var(--text-sm); line-height: 1.5; color: var(--text-secondary);
  }
  .cm-refs { display: flex; flex-wrap: wrap; gap: var(--space-1); margin: 0; }
  .git-c-ref {
    flex: 0 1 auto; max-width: 40%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font-size: 10px; padding: 0 6px; border-radius: var(--radius-full);
    background: var(--accent-dim); color: var(--accent); font-family: var(--font-mono);
  }
  .cm-meta { margin: 0; display: grid; grid-template-columns: auto 1fr; gap: var(--space-1) var(--space-3); }
  .cm-meta dt { color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.04em; font-size: var(--text-xs); align-self: center; }
  .cm-meta dd { margin: 0; color: var(--text-secondary); font-size: var(--text-sm); word-break: break-word; }
  .cm-meta dd.mono { font-family: var(--font-mono); font-size: var(--text-xs); }
</style>
