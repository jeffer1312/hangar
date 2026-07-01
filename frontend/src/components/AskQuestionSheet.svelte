<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import AskQuestionStepper from './AskQuestionStepper.svelte';
  import type { AskQuestionPayload, AnswerItem } from '../lib/types';

  // Container mobile: bottom-sheet embrulhando o stepper compartilhado. No desktop usa-se
  // AskQuestionCard (inline no chat) — ambos rodam o mesmo AskQuestionStepper.
  interface Props {
    open: boolean;
    payload: AskQuestionPayload | null;
    onSubmit: (answers: AnswerItem[]) => Promise<void>;
    onClose: () => void;
    onFallback: () => void; // abre o espelho TUI se algo falhar (reservado)
  }
  let { open, payload, onSubmit, onClose }: Props = $props();
</script>

<BottomSheet {open} {onClose} ariaLabel="Perguntas">
  <AskQuestionStepper {open} {payload} {onSubmit} {onClose} />
</BottomSheet>
