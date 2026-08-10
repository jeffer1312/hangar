<script lang="ts">
  // Stub de teste (vitest __mocks__): expõe o onScan — a porta pro handleScan do pai, pra provar o
  // guard de addBusy no callback QR (segunda chamada enquanto pendente é ignorada).
  let { onScan }: { onScan?: (text: string) => void } = $props();
</script>

<button data-testid="stub-scan" onclick={() => onScan?.('https://host:8765/?token=abc')}>escanear (stub)</button>
