<script lang="ts">
  // Faixa do rodape do modal: conflito de sequenciador, erro e saida do ultimo comando.
  import type { GitStore } from '../../lib/gitStore.svelte';
  interface Props { git: GitStore; menuAberto: boolean }
  let { git, menuAberto }: Props = $props();

  // Confirm em 2 passos, e o reset SO no sucesso: um abort que o git recusou nao pode voltar o
  // botao pro estado inicial, senao o proximo conflito ja aparece em "confirmar" (regra que hoje
  // vinha da GitToolbar, apagada).
  let confirmar = $state(false);
  async function doAbort() {
    if (await git.abortOp()) confirmar = false;
  }
</script>

{#if git.pendingAbort}
  <div class="gsb-conflito" role="status">
    <span>⚠ {git.pendingAbort === 'revert-abort' ? 'revert' : 'cherry-pick'} em conflito</span>
    {#if confirmar}
      <button class="git-mini danger" disabled={!!git.busy} onclick={doAbort}>confirmar abort</button>
      <button class="git-mini" onclick={() => (confirmar = false)}>não</button>
    {:else}
      <button class="git-mini danger" onclick={() => (confirmar = true)}>abortar…</button>
    {/if}
  </div>
{/if}
<!-- Com o CommitMenu aberto quem mostra o erro e o proprio menu (ele fica por cima): repetir aqui
     poria a mesma frase duas vezes na tela. Mesmo padrao que a folha e o painel antigos usavam. -->
{#if git.error && !menuAberto}<p class="gsb-erro">{git.error}</p>{/if}
{#if git.output}<pre class="gsb-saida">{git.output}</pre>{/if}

<style>
  .gsb-conflito {
    display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2);
    padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    background: color-mix(in srgb, var(--warning, #d9a441) 14%, transparent);
    border: 1px solid color-mix(in srgb, var(--warning, #d9a441) 40%, transparent);
    color: var(--text-secondary); font-size: var(--text-xs); line-height: 1.4;
    flex-shrink: 0;
  }
  .gsb-erro {
    margin: 0; font-size: var(--text-sm); color: var(--error);
    white-space: pre-wrap; word-break: break-word; flex-shrink: 0;
  }
  /* Teto de 200px, herdado do <pre> da folha antiga: sem ele um `git status` de repo
     sujo empurra o conteudo do modal pra fora da tela. */
  .gsb-saida {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--surface-raised); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: pre-wrap; word-break: break-all; max-height: 200px; overflow: auto; flex-shrink: 0;
  }
  .git-mini {
    flex-shrink: 0; padding: var(--space-1) var(--space-2); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--surface-raised);
    color: var(--text-muted); font-size: var(--text-xs); cursor: pointer;
  }
  .git-mini:disabled { opacity: 0.5; cursor: default; }
  .git-mini.danger { color: var(--error); border-color: color-mix(in srgb, var(--error) 50%, transparent); }
</style>
