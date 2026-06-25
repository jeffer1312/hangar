<script lang="ts">
  interface Props {
    question: string;
    options: string[];
    onSelect: (index: number) => void;
    onCancel: () => void;
  }
  let { question, options, onSelect, onCancel }: Props = $props();
</script>

<div class="options-wrap">
  <p class="question">{question}</p>
  <div class="options-list">
    {#each options as opt, i}
      <button
        class="option-btn"
        style="animation-delay: {i * 40}ms"
        onclick={() => onSelect(i + 1)}
      >
        <span class="opt-num">{i + 1}.</span>
        <span class="opt-text">{opt}</span>
      </button>
    {/each}
    <button
      class="option-btn option-btn--cancel"
      style="animation-delay: {options.length * 40}ms"
      onclick={onCancel}
    >
      <span class="opt-num">✕</span>
      <span class="opt-text">Cancelar</span>
    </button>
  </div>
</div>

<style>
  .options-wrap {
    padding: var(--space-4) var(--space-4) var(--space-6);
  }

  .question {
    font-size: var(--text-lg);
    color: var(--text-primary);
    font-weight: 500;
    margin-bottom: var(--space-4);
    line-height: 1.4;
  }

  .options-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .option-btn {
    width: 100%;
    min-height: 52px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 0 var(--space-4);
    text-align: left;
    cursor: pointer;
    transition: background 180ms ease-out;
    animation: option-in 220ms ease-out both;
  }

  .option-btn:active {
    background: var(--bg-hover);
  }

  .option-btn--cancel {
    border-color: var(--error);
    color: var(--error);
  }

  .opt-num {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    color: var(--text-secondary);
    flex-shrink: 0;
    min-width: 20px;
  }

  .option-btn--cancel .opt-num {
    color: var(--error);
  }

  .opt-text {
    font-size: var(--text-base);
    color: var(--text-primary);
  }

  .option-btn--cancel .opt-text {
    color: var(--error);
  }
</style>
