<script lang="ts">
  import type { GitStore } from '../../lib/gitStore.svelte';

  // O aviso de tree suja (.git-warn) fica aqui, e nao no BranchList, pra preservar a posicao
  // visual original (ele aparecia logo ANTES da lista de arquivos, nao das branches).
  interface Props {
    git: GitStore;
    onOpenDiff: (path: string) => void;
  }
  let { git, onOpenDiff }: Props = $props();

  let confirmDiscard = $state('');   // path aguardando confirmacao de descarte

  // Rotulo curto do status XY do porcelain (M/A/D/R/? -> palavra).
  function fileTag(code: string): string {
    const c = code.trim()[0] ?? '';
    return { M: 'mod', A: 'novo', D: 'del', R: 'ren', C: 'copia', U: 'conflito', '?': 'novo' }[c] ?? c;
  }

  async function doDiscard(path: string) {
    if (await git.discard(path)) confirmDiscard = '';
  }
</script>

{#if git.dirty && git.files.length}
  <div class="git-warn">working tree suja — troque de branch só depois de commit ou stash</div>
  <p class="git-section">{git.files.length} arquivo{git.files.length > 1 ? 's' : ''} alterado{git.files.length > 1 ? 's' : ''}</p>
  <div class="git-files">
    {#each git.files as f (f.path)}
      {@const slash = f.path.lastIndexOf('/')}
      <div class="git-file-row" class:danger={confirmDiscard === f.path}>
        <button class="git-file" disabled={!!git.busy} onclick={() => onOpenDiff(f.path)} title="ver diff">
          <span class="git-file-tag" data-t={fileTag(f.code)}>{fileTag(f.code)}</span>
          <!-- basename em destaque: o dir trunca no COMECO (direction:rtl), o basename nunca encolhe.
               Um LRM (\u200e) no fim ancora a "/" final em contexto LTR — sem ele o rtl joga a
               barra de borda pro comeco (bug do bidi com pontuacao neutra). -->
          <span class="git-path">{#if slash >= 0}<span class="git-path-dir">{'\u200e' + f.path.slice(0, slash + 1) + '\u200e'}</span>{/if}<span class="git-path-base">{slash >= 0 ? f.path.slice(slash + 1) : f.path}</span></span>
        </button>
        {#if confirmDiscard === f.path}
          <button class="git-mini danger" disabled={!!git.busy} onclick={() => doDiscard(f.path)}>descartar</button>
          <button class="git-mini" disabled={!!git.busy} onclick={() => (confirmDiscard = '')}>não</button>
        {:else}
          <button class="git-mini" disabled={!!git.busy} onclick={() => (confirmDiscard = f.path)} aria-label="descartar mudanças" title="descartar mudanças">⟲</button>
        {/if}
      </div>
    {/each}
  </div>
{/if}

<style>
  .git-warn {
    padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--warning, #d9a441) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--warning, #d9a441) 40%, transparent);
    color: var(--text-secondary); font-size: var(--text-xs); line-height: 1.4;
  }
  .git-section {
    margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.05em;
  }

  .git-files { display: flex; flex-direction: column; gap: 2px; }
  .git-file-row { display: flex; align-items: center; gap: var(--space-2); }
  .git-file-row.danger { background: color-mix(in srgb, var(--error) 12%, transparent); border-radius: var(--radius-md); }
  .git-file {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2);
    padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .git-file:disabled { cursor: default; }
  @media (hover: hover) { .git-file:hover { background: var(--bg-hover); } }
  .git-file-tag {
    flex-shrink: 0; font-size: 10px; font-family: var(--font-mono); text-transform: uppercase;
    letter-spacing: 0.03em; color: var(--text-muted); min-width: 2.4rem;
  }
  .git-file-tag[data-t="novo"] { color: var(--accent); }
  .git-file-tag[data-t="del"] { color: var(--error); }
  .git-mini {
    flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer;
  }
  .git-mini:disabled { opacity: 0.5; cursor: default; }
  .git-mini.danger { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }

  /* Path do arquivo: basename em destaque + dir menor. O dir trunca no COMECO (direction:rtl deixa a
     ellipsis no inicio, mantendo o fim do dir + o basename visiveis); o basename nunca encolhe. */
  .git-path { display: flex; min-width: 0; align-items: baseline; font-family: var(--font-mono); }
  .git-path-dir { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; direction: rtl; color: var(--text-muted); font-size: var(--text-xs); }
  .git-path-base { flex: 0 0 auto; white-space: nowrap; color: var(--text-secondary); }
</style>
