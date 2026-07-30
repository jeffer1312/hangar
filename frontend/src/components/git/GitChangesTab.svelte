<script lang="ts">
  // Aba Mudancas: UMA lista de arquivos alterados. Antes eram duas — o ChangedFiles (com o ⟲
  // descartar) e a lista do CommitBox (com checkbox). Numa aba so, viravam a mesma lista duas vezes.
  // Cada linha carrega as tres acoes: marcar pro commit, abrir o diff, descartar.
  import DiffView from './DiffView.svelte';
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props {
    git: GitStore;
    desktop: boolean;
    // Nivel do drill-down no celular: 0 = lista, 1 = diff. No desktop nao vale (tudo lado a lado).
    level: number;
    onPush: () => void;
    onPop: () => void;
  }
  let { git, desktop, level, onPush, onPop }: Props = $props();

  // Selecao migrada do CommitBox: todos marcados por padrao. `selectionInitialized` existe pra que
  // um desmarque manual NAO seja refeito no proximo refresh() — sem ele o poll sobrescreve a escolha
  // do usuario a cada volta.
  let sel = $state<Set<string>>(new Set());
  let selectionInitialized = $state(false);
  $effect(() => {
    if (!selectionInitialized && git.files.length) {
      sel = new Set(git.files.map((f) => f.path));
      selectionInitialized = true;
    }
  });
  const toggle = (p: string) => { sel.has(p) ? sel.delete(p) : sel.add(p); sel = new Set(sel); };
  const chosen = $derived(git.files.filter((f) => sel.has(f.path)).map((f) => f.path));

  let confirmDiscard = $state('');   // path aguardando confirmacao de descarte
  let message = $state('');
  const canCommit = $derived(!!message.trim() && chosen.length > 0 && !git.busy);

  // Rotulo curto do status XY do porcelain (M/A/D/R/? -> palavra).
  function fileTag(code: string): string {
    const c = code.trim()[0] ?? '';
    return { M: 'mod', A: 'novo', D: 'del', R: 'ren', C: 'copia', U: 'conflito', '?': 'novo' }[c] ?? c;
  }

  async function doDiscard(path: string) {
    if (await git.discard(path)) confirmDiscard = '';
  }
  // Falhou = nao desce de nivel: o boolean do store existe justamente pra isso.
  async function abrirDiff(path: string) {
    if (await git.openFileDiff(path)) onPush();
  }
  function voltar() {
    git.closeDiff();
    onPop();
  }
  async function commitar() {
    if (!canCommit) return;
    if (await git.doCommit(message, chosen)) message = '';
  }
</script>

{#snippet lista()}
  <!-- O aviso de tree suja vinha do ChangedFiles, que morre na Task 9. Ele fala da working tree,
       entao e aqui que ele mora agora. -->
  {#if git.dirty && git.files.length}
    <div class="git-warn">working tree suja — troque de branch só depois de commit ou stash</div>
  {/if}
  {#if !git.files.length}
    <!-- Obrigatorio: o ChangedFiles nao renderizava NADA com o repo limpo, e a aba nasceria em
         branco, sem dizer que esta tudo certo. -->
    <p class="git-muted">nada alterado — a working tree está limpa</p>
  {:else}
    <div class="git-section-row">
      <p class="git-section">{git.files.length} arquivo{git.files.length > 1 ? 's' : ''} alterado{git.files.length > 1 ? 's' : ''}</p>
      <div class="ct-sel-row">
        <button class="git-mini" onclick={() => (sel = new Set(git.files.map((f) => f.path)))}>todos</button>
        <button class="git-mini" onclick={() => (sel = new Set())}>nenhum</button>
      </div>
    </div>
    <div class="git-files">
      {#each git.files as f (f.path)}
        {@const slash = f.path.lastIndexOf('/')}
        <div class="git-file-row" class:danger={confirmDiscard === f.path}>
          <input class="ct-check" type="checkbox" checked={sel.has(f.path)}
            onchange={() => toggle(f.path)} aria-label={`incluir ${f.path} no commit`} />
          <button class="git-file" disabled={!!git.busy} onclick={() => abrirDiff(f.path)} title="ver diff">
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
{/snippet}

{#snippet caixaDeCommit()}
  <!-- Caixa minima de proposito: o CommitBox completo (recentes, amend, branch nova) so entra na
       Task 9, quando ele perde a lista propria. Ate la ele segue intacto e em uso pelo GitSheet —
       montar os dois agora poria duas listas de arquivos na mesma tela, o que esta aba veio matar. -->
  <div class="ct-commit">
    <textarea class="ct-msg" bind:value={message} placeholder="mensagem do commit…" rows="3"
      autocapitalize="off" spellcheck="false"></textarea>
    <button class="ct-btn" disabled={!canCommit} onclick={commitar}>
      Commit{chosen.length ? ` (${chosen.length})` : ''}
    </button>
  </div>
{/snippet}

{#if desktop}
  <div class="ct-cols">
    <section class="ct-col ct-col-list">{@render lista()}</section>
    <section class="ct-col ct-col-diff">
      {#if git.diffPath}
        <DiffView path={git.diffPath} rows={git.diffRows} loading={git.diffLoading} truncated={git.diffTruncated} />
      {:else}
        <p class="git-muted">selecione um arquivo</p>
      {/if}
    </section>
    <section class="ct-col ct-col-commit">{@render caixaDeCommit()}</section>
  </div>
{:else if level >= 1}
  <button class="git-back" onclick={voltar}>‹ arquivos</button>
  <DiffView path={git.diffPath} rows={git.diffRows} loading={git.diffLoading} truncated={git.diffTruncated} />
{:else}
  <div class="ct-stack">
    {@render lista()}
    {@render caixaDeCommit()}
  </div>
{/if}

<style>
  /* Desktop: tres colunas com proporcao fixa, cada uma rolando por conta propria — a lista nao pode
     empurrar o diff pra fora da tela. */
  .ct-cols { display: flex; gap: var(--space-3); min-height: 0; height: 100%; }
  .ct-col { min-width: 0; min-height: 0; overflow: auto; display: flex; flex-direction: column; gap: var(--space-2); }
  .ct-col-list { flex: 3 1 0; }
  .ct-col-diff { flex: 5 1 0; }
  .ct-col-commit { flex: 3 1 0; }
  .ct-stack { display: flex; flex-direction: column; gap: var(--space-3); }

  .ct-sel-row { display: flex; gap: var(--space-2); }
  .ct-check { flex-shrink: 0; margin: 0; }

  .ct-commit { display: flex; flex-direction: column; gap: var(--space-2); }
  .ct-msg {
    width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); resize: vertical;
  }
  .ct-btn {
    padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-primary); font-size: var(--text-sm); cursor: pointer;
  }
  .ct-btn:disabled { opacity: 0.5; cursor: default; }

  .git-back {
    align-self: flex-start; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent; color: var(--text-muted);
    font-size: var(--text-sm); cursor: pointer;
  }

  .git-warn {
    padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--warning, #d9a441) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--warning, #d9a441) 40%, transparent);
    color: var(--text-secondary); font-size: var(--text-xs); line-height: 1.4;
  }
  .git-section-row { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); }
  .git-section {
    margin: 0; font-size: var(--text-xs); color: var(--text-muted);
    text-transform: uppercase; letter-spacing: 0.05em;
  }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }

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
