<script lang="ts">
  // Spy do TerminalPanel para o teste da rota legada (REV G1): em vez de montar o xterm, observa
  // o proprio `open` e grava em globalThis — o efeito de fechamento do DesktopShell fecha o painel
  // no MESMO flush em que ele abre, e o spy e a unica forma de ver o `open` final do componente.
  let { open, connKey }: { open: boolean; connKey: string } = $props();
  const g = globalThis as unknown as Record<string, unknown>;
  $effect(() => {
    g.__tpOpen = open;
    g.__tpKey = connKey;
  });
</script>

<div data-tp-spy></div>
