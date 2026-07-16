<script lang="ts">
  // Quadro kanban de sessões (visualização irmã da lista+chat). Corpo real na Task 4.
  // onOpenSession ainda nao e consumido: o contrato ja fica de pe pro DesktopShell, os cards que o
  // chamam chegam na Task 4.
  interface Props { onOpenSession: (name: string, serverId: string) => void }
  let { onOpenSession }: Props = $props();
</script>

<div class="board"><p class="board-empty">Quadro — em construção</p></div>

<style>
  .board { height: 100%; overflow: auto; padding: var(--space-4); }
  .board-empty { color: var(--text-muted); }
</style>
