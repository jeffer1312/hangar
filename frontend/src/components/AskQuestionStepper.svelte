<script lang="ts">
  import { untrack } from 'svelte';
  import type { AnswerItem, AskQuestionPayload } from '../lib/types';

  // Estado interno de cada pergunta: opção(ões), texto digitado ou "conversar"
  type PickState =
    | { kind: 'option'; indices: number[] }
    | { kind: 'text'; value: string }
    | { kind: 'chat' };

  // Passo-a-passo container-agnóstico: contém TODA a lógica de escolha (opções/multi/texto/conversar/
  // revisão). Sem shell próprio — quem monta (bottom-sheet no mobile, card inline no desktop) só embrulha.
  interface Props {
    open: boolean;
    payload: AskQuestionPayload | null;
    onSubmit: (answers: AnswerItem[]) => Promise<void>;
    onClose: () => void;
  }
  let { open, payload, onSubmit, onClose }: Props = $props();

  let step = $state(0);
  let picks = $state<PickState[]>([]);
  let textOpen = $state(false);
  let textValue = $state('');
  let sending = $state(false);
  let error = $state('');

  const questions = $derived(payload?.questions ?? []);

  // Reseta tudo ao abrir; lê payload dentro de untrack p/ não reativar no mid-flow
  $effect(() => {
    if (!open) return;
    untrack(() => {
      step = 0;
      picks = (payload?.questions ?? []).map(() => ({ kind: 'option' as const, indices: [] }));
      textOpen = false;
      textValue = '';
      sending = false;
      error = '';
    });
  });

  // Avança passo e limpa o campo de texto livre
  function advance() {
    step++;
    textOpen = false;
    textValue = '';
  }

  // Volta ao passo anterior (sem apagar a escolha já feita)
  function goBack() {
    step--;
    textOpen = false;
    textValue = '';
  }

  // Toca numa opção: single-select avança direto; multi-select só alterna a marcação
  function toggleOption(i: number) {
    const cur = picks[step];
    if (!cur || cur.kind !== 'option') return;
    const q = questions[step];
    if (!q) return;
    if (!q.multiSelect) {
      picks = picks.map((p, idx) => (idx === step ? { kind: 'option', indices: [i] } : p));
      advance();
    } else {
      const has = cur.indices.includes(i);
      const next = has ? cur.indices.filter((x) => x !== i) : [...cur.indices, i];
      picks = picks.map((p, idx) => (idx === step ? { kind: 'option', indices: next } : p));
    }
  }

  // Confirma o texto digitado e avança
  function confirmText() {
    const v = textValue.trim();
    if (!v) return;
    picks = picks.map((p, idx) => (idx === step ? { kind: 'text', value: v } : p));
    advance();
  }

  // Registra "conversar" e avança
  function setChat() {
    picks = picks.map((p, idx) => (idx === step ? { kind: 'chat' } : p));
    advance();
  }

  // Monta o payload exato que o backend espera em POST /answer
  function buildAnswers(): AnswerItem[] {
    return questions.map((q, qi) => {
      const p = picks[qi];
      if (p.kind === 'text')
        return { kind: 'text', value: p.value, type_index: q.options.length, labels: [p.value] };
      if (p.kind === 'chat')
        return { kind: 'chat', chat_index: q.options.length + 1 };
      return { kind: 'option', indices: p.indices, multi: q.multiSelect, labels: p.indices.map((i) => q.options[i].label) };
    });
  }

  async function submit() {
    sending = true;
    error = '';
    try {
      await onSubmit(buildAnswers());
    } catch (e) {
      error = e instanceof Error ? e.message : 'Erro ao enviar respostas';
    } finally {
      sending = false;
    }
  }

  // Texto legível da resposta escolhida numa pergunta (para a tela de revisão)
  function pickLabel(qi: number): string {
    const p = picks[qi];
    if (!p) return '—';
    if (p.kind === 'text') return p.value;
    if (p.kind === 'chat') return 'Conversar';
    const q = questions[qi];
    if (!q) return '—';
    return p.indices.map((i) => q.options[i]?.label ?? '').filter(Boolean).join(', ') || '—';
  }

  // Índices selecionados no passo atual (para realçar botões de multi-select)
  const currentPick = $derived(picks[step] as PickState | undefined);
  const selectedIndices = $derived(currentPick?.kind === 'option' ? currentPick.indices : []);
</script>

{#if step < questions.length}
  {@const q = questions[step]}

  <div class="step-nav">
    {#if step > 0}
      <button class="back-link" onclick={goBack}>‹ Voltar</button>
    {:else}
      <span></span>
    {/if}
    <span class="step-counter">{step + 1} / {questions.length}</span>
  </div>

  <h2 class="sheet-title">{q.header}</h2>
  <p class="question-text">{q.question}</p>

  <div class="options-list">
    {#each q.options as opt, i}
      <button
        class="option-btn"
        class:selected={selectedIndices.includes(i)}
        onclick={() => toggleOption(i)}
      >
        {#if q.multiSelect}
          <span class="check-box" aria-hidden="true">{selectedIndices.includes(i) ? '✓' : ''}</span>
        {/if}
        <span class="opt-content">
          <span class="opt-label">{opt.label}</span>
          {#if opt.description}
            <span class="opt-desc">{opt.description}</span>
          {/if}
          {#if opt.preview}
            <!-- span com white-space:pre (nao <pre>: button so aceita phrasing content). Box
                 monospace rolavel — codigo/mockup nunca estoura a largura da opcao. -->
            <span class="opt-preview">{opt.preview}</span>
          {/if}
        </span>
      </button>
    {/each}
  </div>

  {#if q.multiSelect && !textOpen}
    <button class="primary-btn" onclick={advance} disabled={selectedIndices.length === 0}>
      Próximo
    </button>
  {/if}

  {#if !textOpen}
    <div class="escapes">
      <button class="ghost-btn" onclick={() => (textOpen = true)}>✎ Digitar resposta</button>
      <button class="ghost-btn" onclick={setChat}>💬 Conversar sobre isso</button>
    </div>
  {:else}
    <div class="text-escape">
      <!-- svelte-ignore a11y_autofocus -->
      <input
        type="text"
        class="field-input"
        bind:value={textValue}
        placeholder="Sua resposta…"
        autofocus
      />
      <div class="text-actions">
        <button class="primary-btn" onclick={confirmText} disabled={!textValue.trim()}>Confirmar</button>
        <button class="ghost-btn" onclick={() => { textOpen = false; textValue = ''; }}>Cancelar</button>
      </div>
    </div>
  {/if}

{:else if questions.length > 0}
  <!-- Revisão das respostas antes de enviar -->
  <h2 class="sheet-title">Revisar respostas</h2>

  <div class="review-list">
    {#each questions as q, qi}
      <div class="review-item">
        <span class="review-q">{q.header}</span>
        <span class="review-a">{pickLabel(qi)}</span>
      </div>
    {/each}
  </div>

  {#if error}
    <p class="error-msg" role="alert">{error}</p>
  {/if}

  <button class="primary-btn" onclick={submit} disabled={sending}>
    {sending ? 'Enviando…' : 'Enviar'}
  </button>
  <button class="ghost-btn" onclick={onClose}>Cancelar</button>
{/if}

<style>
  .step-nav {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: var(--space-3);
  }

  .back-link {
    color: var(--accent);
    font-size: var(--text-sm);
    font-weight: 500;
    padding: var(--space-1) 0;
    min-height: 44px;
    display: flex;
    align-items: center;
  }

  .step-counter {
    font-size: var(--text-sm);
    color: var(--text-muted);
  }

  .sheet-title {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-2);
  }

  .question-text {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    margin-bottom: var(--space-4);
    line-height: 1.5;
  }

  /* Lista de opções */
  .options-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-bottom: var(--space-4);
  }

  .option-btn {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    width: 100%;
    min-height: 52px;
    padding: var(--space-3);
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    text-align: left;
    transition: border-color 160ms var(--ease-out), background 160ms var(--ease-out);
  }

  .option-btn.selected {
    border-color: var(--accent);
    background: var(--accent-dim);
  }

  .option-btn:active:not(:disabled) {
    background: var(--bg-hover);
  }

  .check-box {
    flex-shrink: 0;
    width: 22px;
    height: 22px;
    border: 1.5px solid var(--border-strong);
    border-radius: var(--radius-sm);
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: var(--text-sm);
    font-weight: 700;
    color: var(--accent);
    margin-top: 1px;
  }

  .selected .check-box {
    border-color: var(--accent);
    background: var(--accent);
    color: #fff;
  }

  .opt-content {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }

  .opt-label {
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
  }

  .opt-desc {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    line-height: 1.4;
  }

  /* Preview da opção (código/mockup do AskUserQuestion): box mono rolável dentro do botão.
     display:block + white-space:pre preservam as quebras; overflow próprio segura linha longa
     (a página nunca rola na horizontal — guarda do message-list). */
  .opt-preview {
    display: block;
    margin-top: var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.45;
    color: var(--text-secondary);
    white-space: pre;
    overflow-x: auto;
    max-height: 14em;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  /* Escape hatches */
  .escapes {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin-top: var(--space-2);
    border-top: 1px solid var(--border-subtle);
    padding-top: var(--space-3);
  }

  /* Input de texto livre */
  .text-escape {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    margin-top: var(--space-3);
    border-top: 1px solid var(--border-subtle);
    padding-top: var(--space-3);
  }

  .text-actions {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  /* Campo de texto */
  .field-input {
    height: 44px;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px;
    padding: 0 var(--space-3);
    outline: none;
    transition: border-color 180ms var(--ease-out);
    width: 100%;
  }

  .field-input::placeholder {
    color: var(--text-muted);
  }

  .field-input:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  /* Revisão */
  .review-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-bottom: var(--space-4);
  }

  .review-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-3);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }

  .review-q {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    font-weight: 500;
  }

  .review-a {
    font-size: var(--text-base);
    color: var(--text-primary);
    font-weight: 600;
  }

  /* Botões (espelham CreateSessionSheet) */
  .primary-btn {
    width: 100%;
    height: 50px;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    font-size: var(--text-base);
    font-weight: 600;
    transition: background 180ms var(--ease-out);
  }

  .primary-btn:active:not(:disabled) {
    background: var(--accent-press);
  }

  .primary-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .ghost-btn {
    width: 100%;
    height: 44px;
    margin-top: var(--space-2);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    border-radius: var(--radius-md);
  }

  .ghost-btn:active {
    background: var(--bg-hover);
  }

  .error-msg {
    font-size: var(--text-sm);
    color: var(--error);
    margin-bottom: var(--space-3);
  }
</style>
