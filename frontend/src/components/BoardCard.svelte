<script lang="ts">
  // STUB: o corpo real (estado vivo, cauda do histórico, composer inline) chega na Task 5. Aqui só o
  // contrato de props + abrir a sessão, pra o Board compilar e rodar sozinho.
  import type { BoardRow } from '../screens/Board.svelte';
  import type { Server } from '../lib/auth';
  interface Props {
    session: BoardRow; server: Server; color: string;
    draft: string; onDraftChange: (t: string) => void; onOpen: () => void;
  }
  let { session, onOpen }: Props = $props();
</script>

<button class="bc-stub" onclick={onOpen}>{session.name}</button>

<style>.bc-stub { text-align: left; padding: var(--space-2); }</style>
