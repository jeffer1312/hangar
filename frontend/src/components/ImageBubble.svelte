<script lang="ts">
  interface Props {
    caption: string;
    src: string;
  }
  let { caption, src }: Props = $props();
</script>

<div class="image-bubble">
  {#if caption}<p class="image-caption">{caption}</p>{/if}
  <img class="image-content" {src} alt="imagem enviada" loading="lazy" />
</div>

<style>
  .image-bubble {
    align-self: flex-end;
    max-width: 80%;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin: var(--space-1) 0;
    padding: var(--space-2);
    background: var(--accent-dim);
    border-radius: var(--radius-lg);
    animation: bubble-in 200ms var(--ease-out);
  }
  .image-caption {
    font-size: var(--text-sm);
    color: var(--text-primary);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .image-content {
    max-width: 100%;
    border-radius: var(--radius-md);
    display: block;
  }
</style>
