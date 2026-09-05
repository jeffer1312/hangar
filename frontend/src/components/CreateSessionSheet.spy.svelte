<script lang="ts">
  // Dublê do CreateSessionSheet nos testes da Sidebar. O stub genérico (`<div />`) não deixa
  // afirmar NADA sobre o que a Sidebar mandou pra folha, e é exatamente isso que o modo bastão
  // precisa travar: "+ Nova" tem que abrir em modo normal mesmo depois de uma passagem, e o item
  // do menu tem que abrir com a origem certa. Só publica as props no DOM — sem comportamento.
  let { open = false, bastao = null }: {
    open?: boolean;
    bastao?: { name: string; cwd: string; serverId: string } | null;
  } = $props();
</script>

<div data-testid="create-sheet" data-open={open ? '1' : '0'} data-bastao={bastao?.name ?? ''}></div>
