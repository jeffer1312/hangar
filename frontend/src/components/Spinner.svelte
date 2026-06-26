<script lang="ts">
  interface Props {
    label?: string | null;
  }
  let { label = null }: Props = $props();

  // So o que o Claude manda (label do spinner real, ex "Computing… (4m 44s)"). O cronometro local
  // foi removido: era falso (contava do mount no front), e numa conexao stale tiquetaqueava sozinho
  // enquanto o estado real estava congelado -> enganava. Tempo autoritativo vive no UsageSheet.
  const stateLabel = $derived(label ?? 'Trabalhando…');
</script>

<div class="spinner" role="status" aria-live="polite">
  <span class="dot" aria-hidden="true"></span>
  <span class="spinner-label">{stateLabel}</span>
</div>

<style>
  /* Linha slim, sem bubble/card — estilo da linha de spinner do Claude Code. */
  .spinner { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-1); animation: bubble-in 200ms var(--ease-out); }
  .dot { width: 7px; height: 7px; border-radius: var(--radius-full); flex-shrink: 0; background: var(--pill-working-fg); animation: pulse-scale 1.4s ease-in-out infinite; }
  .spinner-label { font-size: var(--text-sm); color: var(--text-secondary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
