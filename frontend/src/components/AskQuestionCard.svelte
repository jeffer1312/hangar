<script lang="ts">
  import AskQuestionStepper from './AskQuestionStepper.svelte';
  import type { AskQuestionPayload, AnswerItem } from '../lib/types';

  // Container desktop: card inline no fluxo do chat (sem backdrop/modal) — o contexto acima
  // (mensagem/tabela do assistente) fica visível enquanto se escolhe. Roda o mesmo stepper do sheet.
  interface Props {
    open: boolean;
    payload: AskQuestionPayload | null;
    onSubmit: (answers: AnswerItem[]) => Promise<void>;
    onClose: () => void;
  }
  let { open, payload, onSubmit, onClose }: Props = $props();
</script>

{#if open}
  <div class="ask-card" role="group" aria-label="Perguntas">
    <AskQuestionStepper {open} {payload} {onSubmit} {onClose} />
  </div>
{/if}

<style>
  .ask-card {
    align-self: stretch;
    max-width: 600px;
    margin: var(--space-2) 0;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    padding: var(--space-4) var(--space-5);
    animation: card-in 220ms var(--ease-out) both;
  }

  @keyframes card-in {
    from { opacity: 0; transform: translateY(6px); }
    to   { opacity: 1; transform: translateY(0); }
  }
</style>
