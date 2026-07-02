<script lang="ts">
  interface Props {
    question: string;
    options: string[];
    onSelect: (index: number) => void;
    onCancel: () => void;
  }
  let { question, options, onSelect, onCancel }: Props = $props();

  // Menu de PERMISSAO do Claude Code (tool use): "Yes…" / "Yes, and don't ask again…" / "No…".
  // Deteccao TOLERANTE por assinatura das opcoes (en/pt); nao casou -> lista generica de sempre.
  // ponytail: heuristica textual; se o Claude Code mudar o texto do menu, degrada pro generico.
  function kindOf(o: string): 'allow' | 'always' | 'deny' | 'other' {
    const l = o.toLowerCase();
    if (/don'?t ask again|always|sempre|n[aã]o perguntar/.test(l)) return 'always';
    if (/^(yes|sim)\b/.test(l)) return 'allow';
    if (/^(no|n[aã]o)\b/.test(l)) return 'deny';
    return 'other';
  }
  const kinds = $derived(options.map(kindOf));
  const isPermission = $derived(
    options.length >= 2 && options.length <= 4 &&
    kinds.includes('allow') && kinds.includes('deny')
  );
</script>

<div class="options-wrap">
  {#if isPermission}
    <span class="perm-chip">🔐 Pedido de permissão</span>
  {/if}
  <p class="question">
    <!-- Trechos entre crases (comando/arquivo do pedido) viram <code> — legivel no celular. -->
    {#each question.split('`') as part, i}{#if i % 2 === 1}<code class="q-code">{part}</code>{:else}{part}{/if}{/each}
  </p>
  <div class="options-list">
    {#each options as opt, i}
      <button
        class="option-btn"
        class:option-btn--allow={isPermission && kinds[i] === 'allow'}
        class:option-btn--always={isPermission && kinds[i] === 'always'}
        class:option-btn--deny={isPermission && kinds[i] === 'deny'}
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

  /* Pedido de permissao: chip + botoes com semantica visual (permitir/sempre/negar). */
  .perm-chip {
    display: inline-block;
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--accent);
    background: var(--accent-dim);
    border-radius: var(--radius-full);
    padding: 2px 10px;
    margin-bottom: var(--space-2);
  }
  .q-code {
    font-family: var(--font-mono);
    font-size: 0.85em;
    background: var(--bg-elevated);
    padding: 2px 6px;
    border-radius: 4px;
    word-break: break-all;
  }
  .option-btn--allow {
    background: var(--accent);
    border-color: var(--accent);
  }
  .option-btn--allow .opt-text, .option-btn--allow .opt-num { color: #fff; }
  .option-btn--allow:active { background: var(--accent-press); }
  .option-btn--always {
    background: var(--accent-dim);
    border-color: var(--accent);
  }
  .option-btn--always .opt-text { color: var(--accent); }
  .option-btn--deny {
    border-color: var(--error);
  }
  .option-btn--deny .opt-text, .option-btn--deny .opt-num { color: var(--error); }

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
