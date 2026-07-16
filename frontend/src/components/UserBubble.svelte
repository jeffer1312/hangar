<script lang="ts">
  interface Props {
    text: string;
    ts?: number | null;
    animate?: boolean;   // false = bubble de HISTORICO remontada (paginacao/janela): sem fade
    from?: string | null;          // recado de OUTRA sessao (cp-send): nome da sessao remetente
    onForward?: (() => void) | null; // abre o picker "encaminhar pra sessao" (long-press/hover)
    onOpenPeer?: (() => void) | null; // tap no chip "de: X" -> abre o chat da sessao remetente
  }
  let { text, ts, animate = true, from = null, onForward = null, onOpenPeer = null }: Props = $props();

  function formatTime(ts: number | null | undefined): string {
    if (!ts) return '';
    return new Date(ts * 1000).toLocaleTimeString('pt-BR', {
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  // Long-press (500ms, mesmo padrao do SessionCard) -> encaminhar. touchmove cancela (scroll).
  let pressTimer: ReturnType<typeof setTimeout> | null = null;
  function pressStart() {
    if (!onForward) return;
    pressTimer = setTimeout(() => { pressTimer = null; onForward?.(); }, 500);
  }
  function pressCancel() {
    if (pressTimer) { clearTimeout(pressTimer); pressTimer = null; }
  }
</script>

<div class="bubble-wrap" class:noanim={!animate}>
  <div
    class="bubble"
    class:peer={!!from}
    ontouchstart={pressStart}
    ontouchend={pressCancel}
    ontouchmove={pressCancel}
    oncontextmenu={(e) => { if (onForward) { e.preventDefault(); onForward(); } }}
  >
    {#if from}
      {#if onOpenPeer}
        <button class="peer-chip peer-chip--link" onclick={onOpenPeer}
                title={`Abrir o chat de ${from}`}>📟 de: {from} ›</button>
      {:else}
        <span class="peer-chip">📟 de: {from}</span>
      {/if}
    {/if}
    <p class="bubble-text">{text}</p>
    {#if onForward}
      <button class="msg-fwd" onclick={onForward} aria-label="Encaminhar pra outra sessão" title="Encaminhar pra outra sessão"></button>
    {/if}
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
    position: relative;
    background: var(--bubble-user);
    color: var(--text-primary);
    /* 80% do container, com teto de leitura: na coluna larga do desktop (ate 1400px) 80% viraria
       um balao de ~1100px com linhas ilegiveis. */
    max-width: min(80%, 46rem);
    padding: var(--space-3) var(--space-4);
    border-radius: 18px 18px 4px 18px;
    word-break: break-word;
  }

  /* Recado de OUTRA sessao Claude (cp-send): borda/fundo accent pra nao passar por msg do usuario. */
  .bubble.peer {
    background: var(--accent-dim);
    border: 1px solid var(--accent);
  }

  .peer-chip {
    display: block;
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--accent);
    margin-bottom: var(--space-1);
  }

  /* Chip clicável (navega pro chat do remetente): mesmo visual, affordance no hover/active. */
  .peer-chip--link {
    background: none; border: none; padding: 0; text-align: left; cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }
  .peer-chip--link:hover { text-decoration: underline; }
  .peer-chip--link:active { opacity: 0.7; }

  .bubble-text {
    font-size: var(--text-base);
    line-height: 1.55;
    white-space: pre-wrap;
  }

  /* Encaminhar: so desktop (hover), botão leve à ESQUERDA do balão (fora do texto), centrado
     na vertical — mesmo estilo das ações do AssistantBubble. Mobile = long-press. */
  .msg-fwd {
    position: absolute; top: 50%; left: -30px; transform: translateY(-50%);
    width: 24px; height: 24px; padding: 0;
    display: none; align-items: center; justify-content: center;
    border: none; border-radius: var(--radius-sm);
    background: transparent; color: var(--text-muted);
    opacity: 0; transition: opacity 120ms var(--ease-out), background 120ms var(--ease-out);
    cursor: pointer;
  }
  .msg-fwd::before { content: '↗'; font-size: 14px; line-height: 1; }
  @media (hover: hover) and (pointer: fine) {
    .msg-fwd { display: flex; }
    .bubble:hover .msg-fwd { opacity: 0.55; }
    .msg-fwd:hover { opacity: 1 !important; background: var(--bg-hover); color: var(--text-primary); }
  }

  .ts {
    font-size: var(--text-xs);
    color: var(--text-muted);
    margin-top: var(--space-1);
    padding-right: var(--space-1);
  }
</style>
