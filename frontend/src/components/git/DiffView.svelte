<script lang="ts">
  import type { DiffRow } from '../../lib/highlight';

  interface Props {
    path: string;
    rows: DiffRow[];
    loading: boolean;
    truncated?: boolean;   // diff do commit inteiro/vs-worktree capado em 200KB pelo backend (_DIFF_MAX)
  }
  let { path, rows, loading, truncated = false }: Props = $props();

  // +N / -M do diff aberto (contado do proprio rows; GitLens/TortoiseGit mostram no topo).
  const diffStat = $derived({
    add: rows.filter((r) => r.kind === 'add').length,
    del: rows.filter((r) => r.kind === 'del').length,
  });
</script>

<!-- .git-diff-head carrega a mesma costura fina que o cabecalho da folha antiga tinha —
     o botao "voltar" fica fora deste componente (quem o desenha e a aba), entao a borda migrou pra cá pra manter
     a divisoria exatamente na mesma posicao visual (logo antes do conteudo do diff). -->
<div class="git-diff-head">
  <span class="git-diff-name">{path}</span>
  {#if !loading && (diffStat.add || diffStat.del)}
    <span class="git-diff-stat"><span class="stat-add">+{diffStat.add}</span> <span class="stat-del">−{diffStat.del}</span></span>
  {/if}
</div>
{#if loading}
  <p class="git-muted">carregando diff…</p>
{:else if !rows.length}
  <!-- "git diff HEAD" legitimamente vazio (ex: comparar o topo com a working tree limpa) e nao pode
       parecer carga falhada — a caixa em branco era indistinguivel de um erro engolido. -->
  <p class="git-muted">sem diferenças</p>
{:else}
  {#if truncated}
    <p class="git-warn">diff cortado em 200 KB — abra o commit no terminal pra ver o resto</p>
  {/if}
  <pre class="git-diff">{#each rows as row, i (i)}<span
      class:add={row.kind === 'add'}
      class:del={row.kind === 'del'}
      class:hunk={row.kind === 'hunk'}
      class:meta={row.kind === 'meta'}
    >{#if row.prefix}<span class="diff-prefix">{row.prefix}</span>{/if}{#each row.tokens as t, j (j)}<span style={t.color ? `color: ${t.color}` : undefined}>{t.content}</span>{/each}</span>{/each}</pre>
{/if}

<style>
  .git-diff-head {
    display: flex; flex-direction: column; gap: var(--space-2); flex-shrink: 0;
    padding-bottom: var(--space-2); border-bottom: 1px solid var(--border-subtle);
  }
  .git-diff-name {
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .git-diff-stat { flex: 0 0 auto; font-family: var(--font-mono); font-size: var(--text-xs); }
  .git-diff-stat .stat-add { color: var(--success); }
  .git-diff-stat .stat-del { color: var(--error); }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .git-warn {
    margin: 0 0 var(--space-2); padding: var(--space-2); border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--warning) 12%, transparent); font-size: var(--text-xs);
    color: var(--warning);
  }

  .git-diff {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--bg-base); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.5;
    max-height: 62vh; overflow: auto; white-space: pre;
  }
  /* So os filhos DIRETOS sao linhas (block); os tokens do Shiki dentro delas ficam inline. */
  .git-diff > span { display: block; }
  /* Fundo tingido por linha (add/del) + cor default do prefixo/fallback. O codigo em si recebe a cor
     inline dos tokens do Shiki (tema VS Code); a cor abaixo so pinta o prefixo +/- e o modo sem-highlight. */
  .git-diff .add { color: var(--success); background: color-mix(in srgb, var(--success) 10%, transparent); }
  .git-diff .del { color: var(--error); background: color-mix(in srgb, var(--error) 10%, transparent); }
  .git-diff .hunk { color: var(--accent); }
  .git-diff .meta { color: var(--text-muted); }
  .git-diff .diff-prefix { opacity: 0.7; user-select: none; }
</style>
