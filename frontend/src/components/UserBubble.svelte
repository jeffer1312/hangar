<script lang="ts">
  interface Props {
    text: string;
    ts?: number | null;
    animate?: boolean;   // false = bubble de HISTORICO remontada (paginacao/janela): sem fade
  }
  let { text, ts, animate = true }: Props = $props();

  function formatTime(ts: number | null | undefined): string {
    if (!ts) return '';
    return new Date(ts * 1000).toLocaleTimeString('pt-BR', {
      hour: '2-digit',
      minute: '2-digit',
    });
  }
</script>

<div class="bubble-wrap" class:noanim={!animate}>
  <div class="bubble">
    <p class="bubble-text">{text}</p>
  </div>
  {#if ts}
    <span class="ts">{formatTime(ts)}</span>
  {/if}
</div>

<style>
  .bubble-wrap {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    animation: bubble-in 220ms var(--ease-out) both;
    margin-bottom: var(--space-3);
  }

  /* Historico remontado (paginacao/janela): entra parado. */
  .bubble-wrap.noanim { animation: none; }

  .bubble {
    background: var(--bubble-user);
    color: var(--text-primary);
    /* 80% do container, com teto de leitura: na coluna larga do desktop (ate 1400px) 80% viraria
       um balao de ~1100px com linhas ilegiveis. */
    max-width: min(80%, 46rem);
    padding: var(--space-3) var(--space-4);
    border-radius: 18px 18px 4px 18px;
    word-break: break-word;
  }

  .bubble-text {
    font-size: var(--text-base);
    line-height: 1.55;
    white-space: pre-wrap;
  }

  .ts {
    font-size: var(--text-xs);
    color: var(--text-muted);
    margin-top: var(--space-1);
    padding-right: var(--space-1);
  }
</style>
