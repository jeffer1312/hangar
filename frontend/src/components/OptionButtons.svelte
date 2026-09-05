<script lang="ts">
  import * as m from '../paraglide/messages';
  import { kindOf, isPermission as isPermissionFn } from '@hangar/core';
  interface Props {
    question: string;
    options: string[];
    onSelect: (index: number) => void;
    onCancel: () => void;
    /** Envia as opções já marcadas (só múltipla escolha). Ausente = sem botão de enviar. */
    onSubmit?: () => void;
  }
  let { question, options, onSelect, onCancel, onSubmit }: Props = $props();

  // MÚLTIPLA ESCOLHA: o TUI desenha a caixinha antes do rótulo ("[ ] Alfa", "[✔] Alfa"), e é só
  // por ela que dá pra saber — o pane não diz de outro jeito. Aqui marcar e enviar são ações
  // DIFERENTES: cada toque alterna uma opção e o picker continua aberto; quem envia é o botão.
  // Sem isso dava pra marcar e não dava pra enviar, e a única saída era Cancelar (relatado com
  // print, 28/08/2026).
  const CAIXA = /^\[(.?)\]\s*/;
  const multipla = $derived(options.some((o) => CAIXA.test(o)));
  const marcadas = $derived(options.filter((o) => /^\[[^\s\]]\]/.test(o)).length);
  /** Rótulo sem a caixinha — ela vira o estado visual do botão, não texto. */
  function semCaixa(o: string): string { return o.replace(CAIXA, ''); }
  function marcada(o: string): boolean { return /^\[[^\s\]]\]/.test(o); }

  const kinds = $derived(options.map(kindOf));
  const isPermission = $derived(isPermissionFn(options));
</script>

<div class="options-wrap">
  {#if isPermission}
    <span class="perm-chip">{m.permissao_pedido()}</span>
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
        aria-pressed={multipla ? marcada(opt) : undefined}
      >
        <span class="opt-num">{i + 1}.</span>
        {#if multipla}
          <!-- Decorativo: quem anuncia marcado/desmarcado e o aria-pressed do botao. -->
          <span class="opt-caixa" class:opt-caixa--on={marcada(opt)} aria-hidden="true">
            {marcada(opt) ? '✔' : ''}
          </span>
        {/if}
        <span class="opt-text">{multipla ? semCaixa(opt) : opt}</span>
      </button>
    {/each}
    {#if multipla && onSubmit}
      <!-- Marcar não envia. Sem este botão a única saída era Cancelar. -->
      <button
        class="option-btn option-btn--enviar"
        style="animation-delay: {options.length * 40}ms"
        onclick={onSubmit}
        disabled={marcadas === 0}
      >
        <span class="opt-num">➤</span>
        <span class="opt-text">{m.opcoes_enviar_marcadas({ n: marcadas })}</span>
      </button>
    {/if}
    <button
      class="option-btn option-btn--cancel"
      style="animation-delay: {(options.length + 1) * 40}ms"
      onclick={onCancel}
    >
      <span class="opt-num">✕</span>
      <span class="opt-text">{m.comum_cancelar()}</span>
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
    background: var(--surface-raised);
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
    background: var(--surface-raised);
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

  /* Enviar as marcadas: é a ação POSITIVA da múltipla escolha, então tem o peso do acento —
     Cancelar continua sendo a saída, não o caminho. Sem marcação nenhuma ele desabilita: enviar
     zero opção é o mesmo que cancelar, e oferecer duas portas pro mesmo lugar confunde. */
  .option-btn--enviar {
    border-color: var(--accent);
    color: var(--accent);
    font-weight: 600;
  }
  .option-btn--enviar:disabled {
    opacity: 0.45;
    border-color: var(--border-subtle);
    color: var(--text-muted);
    font-weight: 400;
  }
  /* Caixinha: o estado vira desenho, e o rótulo fica só com o texto. */
  .opt-caixa {
    flex-shrink: 0;
    width: 18px; height: 18px;
    display: inline-flex; align-items: center; justify-content: center;
    border: 1px solid var(--border); border-radius: var(--radius-sm);
    font-size: 11px; line-height: 1;
  }
  .opt-caixa--on {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--bg-base);
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
