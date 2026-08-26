<script lang="ts">
  import { svgIcone } from '../../lib/fileIcons';
  interface Props { nome: string; isDir?: boolean; aberta?: boolean; }
  let { nome, isDir = false, aberta = false }: Props = $props();
  // {@html} só de string GERADA no build (fileIcons.generated.ts, filtrada contra script/on*=),
  // nunca de dado do servidor.
  const svg = $derived(svgIcone(nome, isDir, aberta));
</script>

<span class="fi" aria-hidden="true">{@html svg}</span>

<style>
  .fi { display: inline-flex; width: 16px; height: 16px; flex: none; }
  .fi :global(svg) { width: 100%; height: 100%; }
</style>
