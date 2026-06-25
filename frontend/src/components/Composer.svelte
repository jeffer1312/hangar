<script lang="ts">
  import { onDestroy } from 'svelte';
  import IconSend from './icons/IconSend.svelte';
  import IconInterrupt from './icons/IconInterrupt.svelte';
  import ContextRing from './ContextRing.svelte';
  import LiveMetrics from './LiveMetrics.svelte';
  import ModelEffortSheet from './ModelEffortSheet.svelte';
  import type { State } from '../lib/types';
  import type { StatusFields } from '../lib/statusline';

  interface Props {
    sessionState: State;
    status: StatusFields | null;
    label?: string | null;
    onSend: (text: string) => void;
    onInterrupt: () => void;
    onCommand: (cmd: string) => void;
  }
  let { sessionState, status, label = null, onSend, onInterrupt, onCommand }: Props = $props();

  let inputText = $state('');
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  const isWorking = $derived(sessionState === 'working');
  const canSend = $derived(inputText.trim().length > 0 && !isWorking);

  // ── Status row: rotulo por estado (espelha a ideia do StatusPill) ──────────
  const stateLabels: Record<State, string> = {
    working: '',
    idle: 'Pronto',
    awaiting_input: 'Aguardando resposta',
    dead: 'Sessão encerrada',
  };
  const stateLabel = $derived(
    sessionState === 'working' ? (label ?? 'Trabalhando…') : stateLabels[sessionState]
  );

  // ── Cronometro local: conta a partir do instante em que entra em "working" ──
  // Aproximacao client-side (reseta no reconnect do SSE) — e o ancora de liveness,
  // nao um tempo autoritativo. Fora de "working" cai pro sessionTime da statusline.
  let elapsedLabel = $state<string | null>(null);
  let timer: ReturnType<typeof setInterval> | null = null;
  let startedAt = 0;

  function fmtElapsed(ms: number): string {
    const total = Math.max(0, Math.floor(ms / 1000));
    const mm = Math.floor(total / 60);
    const ss = total % 60;
    return String(mm).padStart(2, '0') + ':' + String(ss).padStart(2, '0');
  }

  function stopTimer() {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  }

  $effect(() => {
    if (sessionState !== 'working') {
      stopTimer();
      elapsedLabel = null;
      return;
    }
    startedAt = Date.now();
    elapsedLabel = fmtElapsed(0);
    timer = setInterval(() => {
      elapsedLabel = fmtElapsed(Date.now() - startedAt);
    }, 1000);
    return () => stopTimer();
  });

  onDestroy(() => stopTimer());

  const timeLabel = $derived(
    sessionState === 'working' ? elapsedLabel : (status?.sessionTime ?? null)
  );

  // Linha de status colapsa por completo quando nao ha rotulo nem metricas.
  const hasMetrics = $derived(
    (typeof timeLabel === 'string' && timeLabel.length > 0) ||
    (typeof status?.costUsd === 'number' && isFinite(status.costUsd))
  );
  const showStatusRow = $derived(stateLabel.length > 0 || hasMetrics);

  // ── Pill de modelo + esforco: abre o ModelEffortSheet e envia slash commands ──
  // Display otimista: a escolha local aparece na hora; o status (read-back real do
  // statusline) reconcilia o modelo quando confirma. Esforco e write-only (sem
  // read-back confiavel) -> a escolha local persiste.
  let sheetOpen = $state(false);
  let chosenModel = $state<string | null>(null);   // rotulo otimista: 'Opus' | 'Sonnet' | ...
  let chosenEffort = $state<string | null>(null);   // 'low' | 'medium' | 'high' | 'max'

  const pillModel = $derived(chosenModel ?? status?.model ?? null);
  const pillEffort = $derived(chosenEffort ?? status?.effort ?? null);
  const pillText = $derived(
    pillModel ? pillModel + (pillEffort ? ' · ' + pillEffort : '') : 'Modelo'
  );

  // Reconciliacao do modelo: quando o statusline confirma a escolha (substring match),
  // solta a escolha otimista pra que mudancas feitas direto no terminal reaparecam.
  $effect(() => {
    const m = status?.model?.toLowerCase();
    if (chosenModel && m && m.includes(chosenModel.toLowerCase())) {
      chosenModel = null;
    }
  });

  function handleSelectModel(arg: string) {
    // exibe o rotulo capitalizado na hora; arg lowercase vai pro comando
    chosenModel = arg.charAt(0).toUpperCase() + arg.slice(1);
    onCommand('/model ' + arg);
  }

  function handleSelectEffort(level: string) {
    chosenEffort = level;
    onCommand('/effort ' + level);
  }

  // ── Textarea: auto-grow ate 120px ──────────────────────────────────────────
  function autoGrow() {
    if (!textareaEl) return;
    textareaEl.style.height = 'auto';
    textareaEl.style.height = Math.min(textareaEl.scrollHeight, 120) + 'px';
  }

  function handleInput() {
    autoGrow();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      submit();
    }
  }

  function submit() {
    if (!canSend) return;
    const msg = inputText.trim();
    inputText = '';
    if (textareaEl) {
      textareaEl.style.height = 'auto';
    }
    onSend(msg);
  }

  // Auto-focus quando ocioso
  $effect(() => {
    if (sessionState === 'idle' && textareaEl) {
      setTimeout(() => textareaEl?.focus(), 100);
    }
  });
</script>

<footer class="composer">
  <div class="composer-card">
    {#if showStatusRow}
      <div class="status-row">
        <div class="status-left">
          <span class="dot dot--{sessionState}" aria-hidden="true"></span>
          {#if stateLabel}
            <span class="state-label" role="status" aria-live="polite">{stateLabel}</span>
          {/if}
        </div>
        <LiveMetrics {timeLabel} costUsd={status?.costUsd} />
      </div>
    {/if}

    <textarea
      bind:this={textareaEl}
      bind:value={inputText}
      class="composer-textarea"
      placeholder="Mensagem para Claude…"
      rows={1}
      disabled={isWorking}
      oninput={handleInput}
      onkeydown={handleKeydown}
      aria-label="Mensagem"
    ></textarea>

    <div class="control-row">
      <div class="control-left">
        <ContextRing pct={status?.ctxPct ?? null} />
        <button
          class="model-pill"
          onclick={() => (sheetOpen = true)}
          aria-label="Modelo e esforço de raciocínio"
        >
          {pillText}
        </button>
      </div>

      <div class="control-right">
        {#if isWorking}
          <button class="stop-btn" onclick={onInterrupt} aria-label="Interromper Claude">
            <IconInterrupt size={16} />
          </button>
        {:else}
          <button
            class="send-btn"
            class:send-btn--disabled={!canSend}
            onclick={submit}
            disabled={!canSend}
            aria-label="Enviar mensagem"
          >
            <IconSend size={18} />
          </button>
        {/if}
      </div>
    </div>
  </div>

  <ModelEffortSheet
    open={sheetOpen}
    currentModel={pillModel}
    currentEffort={pillEffort}
    onSelectModel={handleSelectModel}
    onSelectEffort={handleSelectEffort}
    onClose={() => (sheetOpen = false)}
  />
</footer>

<style>
  .composer {
    background: var(--bg-base);
    padding: var(--space-2) var(--space-3) var(--space-3);
  }

  /* Card unico que reune status, textarea e controles. */
  .composer-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-width: 600px;
    margin: 0 auto;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    padding: var(--space-3);
  }

  /* ── Status row ─────────────────────────────────────────────────────────── */
  .status-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    min-height: 24px;
  }

  .status-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .state-label {
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-full);
    flex-shrink: 0;
  }

  .dot--working {
    background: var(--pill-working-fg);
    animation: pulse-scale 1.4s ease-in-out infinite;
  }

  .dot--idle {
    background: var(--success);
  }

  .dot--awaiting_input {
    background: var(--warning);
    animation: pulse-scale 1s ease-in-out infinite;
  }

  .dot--dead {
    background: var(--error);
  }

  /* ── Textarea (transparente dentro do card) ─────────────────────────────── */
  .composer-textarea {
    width: 100%;
    min-height: 24px;
    max-height: 120px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    line-height: 1.55;
    padding: var(--space-1) 0;
    resize: none;
    outline: none;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .composer-textarea::placeholder {
    color: var(--text-muted);
  }

  .composer-textarea:disabled {
    opacity: 0.4;
  }

  /* ── Control row ────────────────────────────────────────────────────────── */
  .control-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    min-height: 44px;
  }

  .control-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  /* Chip de modelo/esforco: botao que abre o ModelEffortSheet (mantem o visual do chip).
     min-height:0 sobrescreve o alvo global de 44px pra preservar o pill compacto de 28px
     (o tap fica confortavel dentro da control-row de 44px). :active scale vem do global. */
  .model-pill {
    display: inline-flex;
    align-items: center;
    height: 28px;
    min-height: 0;
    padding: 0 var(--space-3);
    background: var(--accent-dim);
    border-radius: var(--radius-md);
    font-size: var(--text-xs);
    font-weight: 500;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 160px;
    font-variant-numeric: tabular-nums;
  }

  .control-right {
    flex-shrink: 0;
  }

  .send-btn {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    transition: background 180ms var(--ease-out);
  }

  .send-btn:active:not(:disabled) {
    background: var(--accent-press);
  }

  .send-btn--disabled {
    background: var(--bg-hover);
    color: var(--text-muted);
    cursor: default;
  }

  /* Stop: substitui o antigo botao "Interromper" de largura total. */
  .stop-btn {
    width: 44px;
    height: 44px;
    flex-shrink: 0;
    background: transparent;
    border: 1px solid var(--error);
    border-radius: var(--radius-md);
    color: var(--error);
    transition: background 180ms var(--ease-out);
  }

  .stop-btn:active {
    background: rgba(255, 69, 58, 0.08);
  }
</style>
