<script lang="ts">
  // Harness de teste do GitTabs (GitTabs.test.ts, B3 rodada 4): o $set do Svelte 4 nao existe
  // mais e a prop de um mount() e imutavel — o teste precisa alternar filesInContext (o ciclo
  // de resize 1279<->1280 com o modal aberto) e o harness expoe o metodo.
  import GitTabs from './GitTabs.svelte';
  import type { GitStore } from '../../lib/gitStore.svelte';

  let { git, desktop, onClose }: { git: GitStore; desktop: boolean; onClose: () => void } = $props();
  let filesInContext = $state(false);

  export function setFilesInContext(v: boolean) {
    filesInContext = v;
  }
</script>

<GitTabs {git} {desktop} filesInContext={filesInContext} {onClose} />
