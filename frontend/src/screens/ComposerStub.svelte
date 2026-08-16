<script module lang="ts">
  // Contador de chamadas do focus: o teste do resize (B5) prova que o Chat devolve o foco ao
  // Composer lendo este número, porque o stub nao tem textarea real pra focar.
  export let chamadasFocus = 0;
</script>

<script lang="ts">
  // Stub do Composer para o Chat.test.ts: o Chat chama composerRef.focus() ao encolher a
  // janela com o visor aberto — o stub generico (createRawSnippet) nao expoe o metodo.
  export function focus() {
    chamadasFocus += 1;
  }
</script>
