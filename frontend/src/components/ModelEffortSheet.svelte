<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import type { ModelEffortBody } from '../lib/api';

  // Sheet pra trocar modelo e esforco de raciocinio. NADA e enviado ao mexer: a selecao e
  // local. So o botao "Aplicar nesta sessao" (ou "Salvar como padrao") chama o backend, que
  // dirige o picker interativo do /model -- aplicando SO na sessao (sem virar o default).
  interface Props {
    open: boolean;
    currentModel?: string | null;
    currentEffort?: string | null;
    onApply: (body: ModelEffortBody) => Promise<void> | void;
    onClose: () => void;
  }
  let { open, currentModel = null, currentEffort = null, onApply, onClose }: Props = $props();

  // arg = forma lowercase aceita pelo backend (MODEL_ORDER). meta = descricao curta (sem em-dash).
  const MODELS: { arg: string; label: string; meta: string }[] = [
    { arg: 'default', label: 'Default', meta: 'recomendado' },
    { arg: 'opus',    label: 'Opus',    meta: 'mais capaz' },
    { arg: 'sonnet',  label: 'Sonnet',  meta: 'equilibrado' },
    { arg: 'haiku',   label: 'Haiku',   meta: 'mais rápido' },
  ];

  // Niveis reais do /effort do Claude Code, ordenados Faster -> Smarter (6 paradas).
  // 'ultracode' e o topo. O picker do Opus expoe os 6; modelos menores expoem um subconjunto
  // (o backend acomoda) e o Haiku nao usa esforco.
  const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max', 'ultracode'];
  const EFFORT_DEFAULT = 3; // xhigh: parada neutra quando o nivel atual e desconhecido

  // Casa o modelo atual (read-back 'Opus4.8') com o arg ('opus') por substring.
  function modelArgFromCurrent(cur: string | null | undefined): string {
    const c = cur?.toLowerCase() ?? '';
    return MODELS.find((m) => m.arg !== 'default' && c.includes(m.arg))?.arg ?? 'default';
  }

  // Mapeia o esforco atual pra parada do slider. Exato primeiro; senao prefixo (cobre
  // abreviacoes do statusline: 'med' -> medium, 'ultra' -> ultracode).
  function effortIndex(cur: string | null | undefined): number {
    if (!cur) return EFFORT_DEFAULT;
    const c = cur.trim().toLowerCase();
    const exact = EFFORTS.indexOf(c);
    if (exact >= 0) return exact;
    const pref = EFFORTS.findIndex((l) => l.startsWith(c));
    return pref >= 0 ? pref : EFFORT_DEFAULT;
  }

  // Selecao LOCAL: re-sincroniza com o estado atual toda vez que a folha abre.
  let selectedModel = $state('default');
  let effortIdx = $state(EFFORT_DEFAULT);
  let applying = $state(false);

  $effect(() => {
    if (open) {
      selectedModel = modelArgFromCurrent(currentModel);
      effortIdx = effortIndex(currentEffort);
      applying = false;
    }
  });

  const effortLevel = $derived(EFFORTS[effortIdx]);
  const effortFill = $derived((effortIdx / (EFFORTS.length - 1)) * 100);
  const isHaiku = $derived(selectedModel === 'haiku'); // Haiku nao usa esforco de raciocinio

  function pickModel(arg: string) {
    selectedModel = arg; // so atualiza a selecao local; nada e enviado
  }

  function onEffortSlide(e: Event) {
    effortIdx = Number((e.currentTarget as HTMLInputElement).value); // local only
  }

  async function apply(scope: 'session' | 'default') {
    if (applying) return;
    applying = true;
    try {
      await onApply({ model: selectedModel, effort: EFFORTS[effortIdx], scope });
    } catch {
      // Falha real (rede/picker): nao deixa a folha aberta. Se a troca de effort dispara o
      // follow-up "Change effort level?", ele vira OptionButtons ATRAS da folha -- folha
      // aberta bloquearia o toque. Sempre fechar; o estado/pill refletem o resultado real.
    } finally {
      onClose();
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Modelo e esforço de raciocínio">
  <h2 class="sheet-title">Modelo</h2>

  <ul class="model-list">
    {#each MODELS as m (m.arg)}
      <li>
        <button
          class="model-row"
          class:active={selectedModel === m.arg}
          aria-pressed={selectedModel === m.arg}
          onclick={() => pickModel(m.arg)}
        >
          <span class="model-text">
            <span class="model-name">{m.label}</span>
            <span class="model-meta">{m.meta}</span>
          </span>
          {#if selectedModel === m.arg}
            <svg
              class="check"
              width="18"
              height="18"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <polyline points="20 6 9 17 4 12" />
            </svg>
          {/if}
        </button>
      </li>
    {/each}
  </ul>

  <div class="effort-head">
    <h3 class="section-label">Esforço de raciocínio</h3>
    <span class="effort-current">{isHaiku ? 'n/d' : effortLevel}</span>
  </div>
  <input
    class="range"
    type="range"
    min="0"
    max={EFFORTS.length - 1}
    step="1"
    value={effortIdx}
    style="--fill: {effortFill}%"
    oninput={onEffortSlide}
    disabled={isHaiku}
    aria-label="Esforço de raciocínio"
    aria-valuetext={effortLevel}
  />
  <div class="ends" aria-hidden="true">
    <span>Mais rápido</span>
    <span>Mais inteligente</span>
  </div>
  {#if isHaiku}
    <p class="effort-note">O Haiku não usa esforço de raciocínio.</p>
  {/if}

  <div class="actions">
    <button class="btn btn--primary" onclick={() => apply('session')} disabled={applying}>
      {applying ? 'Aplicando…' : 'Aplicar nesta sessão'}
    </button>
    <button class="btn btn--ghost" onclick={() => apply('default')} disabled={applying}>
      Salvar como padrão
    </button>
  </div>
</BottomSheet>

<style>
  .sheet-title {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-4);
  }

  /* ── Lista de modelos: rows grandes, tappaveis ─────────────────────────── */
  .model-list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin-bottom: var(--space-5);
  }

  .model-row {
    width: 100%;
    min-height: 44px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    text-align: left;
    background: transparent;
    transition: background 160ms var(--ease-out);
  }

  .model-row:active {
    background: var(--bg-hover);
  }

  .model-row.active {
    background: var(--accent-dim);
  }

  .model-text {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    min-width: 0;
  }

  .model-name {
    font-size: var(--text-base);
    font-weight: 500;
    line-height: 1.3;
    color: var(--text-primary);
  }

  .model-meta {
    font-size: var(--text-sm);
    line-height: 1.3;
    color: var(--text-muted);
  }

  .check {
    color: var(--accent);
    flex-shrink: 0;
  }

  /* ── Esforco: slider Faster -> Smarter, 6 paradas (espelha o /effort do Claude) ── */
  .section-label {
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-secondary);
  }

  .effort-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-3);
    margin-bottom: var(--space-3);
  }

  .effort-current {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }

  /* Slider nativo estilizado: alvo de toque de 44px, trilho de 4px, polegar accent.
     --fill (inline) pinta a parte preenchida; o step=1 garante o snap nas 6 paradas. */
  .range {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    height: 44px;
    min-height: 44px;
    background: transparent;
    cursor: pointer;
    display: block;
  }

  .range:focus {
    outline: none;
  }

  .range:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .range::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: var(--radius-full);
    background: linear-gradient(
      to right,
      var(--accent) var(--fill, 0%),
      var(--border-default) var(--fill, 0%)
    );
  }

  .range::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 22px;
    height: 22px;
    margin-top: -9px; /* centraliza no trilho de 4px */
    border-radius: var(--radius-full);
    background: var(--accent);
    border: 2px solid var(--bg-elevated);
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.45);
  }

  .range::-moz-range-track {
    height: 4px;
    border-radius: var(--radius-full);
    background: var(--border-default);
  }

  .range::-moz-range-progress {
    height: 4px;
    border-radius: var(--radius-full);
    background: var(--accent);
  }

  .range::-moz-range-thumb {
    width: 22px;
    height: 22px;
    border: 2px solid var(--bg-elevated);
    border-radius: var(--radius-full);
    background: var(--accent);
  }

  .range:focus-visible::-webkit-slider-thumb {
    box-shadow: 0 0 0 4px var(--accent-dim);
  }

  .range:focus-visible::-moz-range-thumb {
    box-shadow: 0 0 0 4px var(--accent-dim);
  }

  .ends {
    display: flex;
    justify-content: space-between;
    margin-top: var(--space-1);
  }

  .ends span {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }

  .effort-note {
    font-size: var(--text-xs);
    color: var(--text-muted);
    margin-top: var(--space-2);
  }

  /* ── Acoes: aplicar so na sessao (primario) ou salvar como padrao (secundario) ── */
  .actions {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-top: var(--space-5);
  }

  .btn {
    width: 100%;
    min-height: 48px;
    border-radius: var(--radius-md);
    font-size: var(--text-base);
    font-weight: 600;
    transition: background 180ms var(--ease-out), opacity 180ms var(--ease-out);
  }

  .btn:disabled {
    opacity: 0.55;
    cursor: default;
  }

  .btn--primary {
    background: var(--accent);
    color: #fff;
  }

  .btn--primary:active:not(:disabled) {
    background: var(--accent-press);
  }

  .btn--ghost {
    background: transparent;
    border: 1px solid var(--border-default);
    color: var(--text-secondary);
  }

  .btn--ghost:active:not(:disabled) {
    background: var(--bg-hover);
  }
</style>
