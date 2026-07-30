<script lang="ts">
  // Aba Branches: a lista mais o campo de filtro. O filtro so existia no celular e ainda
  // condicionado a "mais de 6 locais ou alguma remota" — o desktop passava
  // filter="" e ficava sem. Numa aba dedicada ele e incondicional: e a unica coisa que se faz aqui.
  import BranchList from './BranchList.svelte';
  import type { GitStore } from '../../lib/gitStore.svelte';

  interface Props { git: GitStore }
  let { git }: Props = $props();

  // O BranchList exige `filter` (obrigatoria, sem default) e so LE o valor — quem guarda o estado e
  // quem desenha o campo e sempre o dono da tela.
  let filter = $state('');
</script>

<div class="bt">
  <input
    class="bt-search"
    type="search"
    placeholder="filtrar branch…"
    bind:value={filter}
    autocapitalize="off"
    autocorrect="off"
    spellcheck="false"
  />
  <div class="bt-list">
    <!-- Vazio NAO ganha texto novo aqui: o BranchList ja diz "nenhuma branch local" e tem a variante
         com filtro. Dois textos concorrentes na mesma tela e pior que um. -->
    <BranchList {git} {filter} />
  </div>
</div>

<style>
  .bt { display: flex; flex-direction: column; gap: var(--space-2); min-height: 0; height: 100%; }
  .bt-list { min-height: 0; overflow: auto; }
  .bt-search {
    width: 100%; padding: var(--space-2) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-default); background: var(--bg-base); color: var(--text-primary);
    font-size: var(--text-sm);
  }
</style>
