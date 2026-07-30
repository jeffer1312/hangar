<script lang="ts">
  // Metade de baixo do que era o CommitDetail: os arquivos do commit. SEM max-height proprio —
  // quem limita altura e o empilhado da aba.
  import { getCommitFiles, type GitCommit, type ChangedFile } from '../../lib/api';

  interface Props {
    commit: GitCommit;
    sessionName: string;
    onOpenFile: (p: string) => void;
    onMenu?: (c: GitCommit) => void;   // opcional: reusos sem menu omitem (idem CommitList)
  }
  let { commit, sessionName, onOpenFile, onMenu }: Props = $props();

  let files = $state<ChangedFile[]>([]);
  let falhou = $state(false);

  // Recarrega ao trocar de commit; zera antes pra nao piscar a lista do commit anterior.
  // A guarda de `vez` e nova: descer a lista trocando de commit rapido deixava a resposta do commit
  // ANTERIOR chegar por ultimo e pintar os arquivos dele sob o commit selecionado agora.
  let vez = 0;
  $effect(() => {
    const h = commit.hash;
    const minha = ++vez;
    files = [];
    falhou = false;
    getCommitFiles(sessionName, h)
      .then((r) => { if (minha === vez) files = r.files; })
      .catch(() => { if (minha === vez) falhou = true; });
  });
</script>

<div class="cf">
  {#if onMenu}
    <div class="cf-head">
      <button class="git-mini" onclick={() => onMenu(commit)} aria-label="ações do commit">⋯ ações</button>
    </div>
  {/if}
  <div class="cf-list">
    {#each files as f (f.path)}
      <button class="git-file" onclick={() => onOpenFile(f.path)} title="ver diff">
        <span class="git-file-tag">{f.code}</span><span class="git-path-base">{f.path}</span>
      </button>
    {:else}
      <!-- Falha de leitura NAO pode virar "nenhum arquivo alterado": sao coisas diferentes, e o
           CommitDetail antigo mostrava a mesma frase nos dois casos (catch zerava a lista). -->
      {#if falhou}
        <p class="git-muted">não deu pra ler os arquivos deste commit</p>
      {:else}
        <p class="git-muted">nenhum arquivo alterado</p>
      {/if}
    {/each}
  </div>
</div>

<style>
  .cf { display: flex; flex-direction: column; gap: var(--space-2); min-height: 0; }
  .cf-head { display: flex; justify-content: flex-end; }
  .cf-list { display: flex; flex-direction: column; gap: 2px; }
  .git-mini {
    flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-elevated);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer;
  }
  .git-mini:disabled { opacity: 0.5; cursor: default; }
  .git-muted { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .git-file {
    display: flex; align-items: center; gap: var(--space-2); min-width: 0;
    padding: var(--space-2); border-radius: var(--radius-md);
    border: 1px solid transparent; background: transparent;
    color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  @media (hover: hover) { .git-file:hover { background: var(--bg-hover); } }
  .git-file-tag {
    flex-shrink: 0; font-size: 10px; font-family: var(--font-mono); text-transform: uppercase;
    letter-spacing: 0.03em; color: var(--text-muted); min-width: 1.6rem;
  }
  .git-path-base { flex: 0 0 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); color: var(--text-secondary); }
</style>
