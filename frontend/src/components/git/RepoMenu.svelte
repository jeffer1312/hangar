<script lang="ts">
  // Acoes de repositorio, no mesmo idioma do SessionContextMenu: backdrop que captura o clique-fora,
  // caixa flutuante, Esc fecha. Posicionamento e `absolute` no pai (o cabecalho do modal, ou o chip
  // do repo) — quem sabe onde o menu deve nascer e quem tem o botao.
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props {
    git: GitStore;
    onClose: () => void;
    // O menu abre em dois contextos: DENTRO do modal, onde a saida aparece na GitStatusBar, e a
    // partir do chip do repo com o modal FECHADO — ai nao ha faixa nenhuma e a falha sumiria calada.
    soltoNaTela?: boolean;
  }
  let { git, onClose, soltoNaTela = false }: Props = $props();

  // `push` NAO passa pelo runAction: GitAction (api.ts) e _ACTIONS (git_ops.py) nao tem 'push' — a
  // toolbar de hoje usa git.doPush(), e e esse o caminho.
  const itens: { rotulo: string; run: () => void | Promise<unknown> }[] = [
    { rotulo: 'status', run: () => git.runAction('status') },
    { rotulo: 'log', run: () => git.openLog() },
    { rotulo: 'fetch', run: () => git.runAction('fetch') },
    { rotulo: 'pull', run: () => git.runAction('pull') },
    { rotulo: 'push', run: () => git.doPush() },
    { rotulo: 'stash', run: () => git.runAction('stash') },
    { rotulo: 'pop', run: () => git.runAction('stash-pop') },
  ];

  async function acionar(item: { run: () => void | Promise<unknown> }) {
    await item.run();
    // Solto na tela o menu FICA aberto pra mostrar o resultado; dentro do modal ele fecha e quem
    // conta o que aconteceu e a faixa do rodape.
    if (!soltoNaTela) onClose();
  }
</script>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape') onClose(); }} />

<div class="menu-backdrop" onclick={onClose} oncontextmenu={(e) => { e.preventDefault(); onClose(); }} role="presentation"></div>
<div class="repo-menu" role="menu">
  {#each itens as item (item.rotulo)}
    <button type="button" role="menuitem" disabled={!!git.busy} onclick={() => acionar(item)}>{item.rotulo}</button>
  {/each}
  {#if soltoNaTela && (git.error || git.output)}
    <div class="rm-sep"></div>
    {#if git.error}<p class="rm-erro">{git.error}</p>{/if}
    {#if git.output}<pre class="rm-saida">{git.output}</pre>{/if}
  {/if}
</div>

<style>
  .menu-backdrop { position: fixed; inset: 0; z-index: 40; }
  .repo-menu {
    position: absolute; z-index: 41; right: 0; top: 100%; min-width: 168px; padding: 4px;
    display: flex; flex-direction: column;
    background: var(--surface-raised); border: 1px solid var(--border-default);
    border-radius: var(--radius-md); box-shadow: 0 8px 28px rgba(0,0,0,0.4);
  }
  .repo-menu button {
    height: 32px; padding: 0 10px; text-align: left; justify-content: flex-start;
    color: var(--text-primary); font-size: var(--text-sm); border-radius: var(--radius-sm);
    font-family: var(--font-mono);
  }
  .repo-menu button:hover { background: var(--bg-hover); }
  .repo-menu button:disabled { opacity: 0.5; cursor: default; }
  .rm-sep { height: 1px; margin: 4px 6px; background: var(--border-subtle); }
  .rm-erro {
    margin: 0; padding: 0 10px; font-size: var(--text-sm); color: var(--error);
    white-space: pre-wrap; word-break: break-word;
  }
  .rm-saida {
    margin: 4px 6px 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    white-space: pre-wrap; word-break: break-all; max-height: 200px; overflow: auto;
  }
</style>
