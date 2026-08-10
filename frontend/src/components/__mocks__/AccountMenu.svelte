<script lang="ts">
  // Stub de teste (vitest __mocks__): o AccountMenu real é pesado demais pra montar no happy-dom.
  // Expõe a porta de "Adicionar servidor" (onAddServer) — é o único caminho pro diálogo de add do
  // Sidebar, que ficou sem entrada de UI depois do Step 1 da 4c (a engrenagem abre Configurações).
  let { onAddServer }: { onAddServer?: () => void } = $props();
</script>

<button data-testid="stub-add" onclick={() => onAddServer?.()}>Adicionar servidor (stub)</button>
