<script lang="ts">
  // Harness do FileSearchBar: segura q em estado local para permitir re-render com prop
  // nova (o $set do mount é bloqueado em DEV no Svelte 5 — component_api_changed).
  import FileSearchBar, { type ModoBusca } from './FileSearchBar.svelte';

  interface Props {
    q: string;
    mode: ModoBusca;
    onBusca: (q: string, mode: ModoBusca) => void;
  }

  let { q, mode, onBusca }: Props = $props();
  // svelte-ignore state_referenced_locally — captura intencional do valor inicial; o
  // harness só recebe as props uma vez, do teste.
  let qAtual = $state(q);
  // svelte-ignore state_referenced_locally — idem.
  let modeAtual = $state(mode);
</script>

<button type="button" class="h-ecoar" onclick={() => (qAtual = 'novo')}>ecoar q=novo</button>
<FileSearchBar q={qAtual} mode={modeAtual} onBusca={onBusca} />
