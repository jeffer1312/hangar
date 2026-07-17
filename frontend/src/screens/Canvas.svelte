<script lang="ts">
  // Placeholder do canvas livre — a Task 10 substitui pela tela real. A interface Props já fixa o
  // contrato que a Task 10 vai cumprir (abrir um card = navegar pro overlay do chat, via App).
  interface Props {
    onOpenSession: (name: string, serverId: string) => void;
  }
  // eslint-disable-next-line @typescript-eslint/no-unused-vars -- consumido pela Task 10 (tela real)
  let { onOpenSession }: Props = $props();
</script>

<p>canvas</p>
