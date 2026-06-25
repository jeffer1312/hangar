<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';

  // Sheet pra trocar modelo e esforco de raciocinio. As selecoes viram slash commands
  // de argumento completo (/model <arg>, /effort <level>) enviados pela sessao viva.
  interface Props {
    open: boolean;
    currentModel?: string | null;
    currentEffort?: string | null;
    onSelectModel: (id: string) => void;
    onSelectEffort: (level: string) => void;
    onClose: () => void;
  }
  let {
    open,
    currentModel = null,
    currentEffort = null,
    onSelectModel,
    onSelectEffort,
    onClose,
  }: Props = $props();

  // arg = forma lowercase passada pro /model. meta = descricao curta (sem em-dash).
  const MODELS: { arg: string; label: string; meta: string }[] = [
    { arg: 'default', label: 'Default', meta: 'escolha do projeto' },
    { arg: 'opus',    label: 'Opus',    meta: 'mais capaz' },
    { arg: 'sonnet',  label: 'Sonnet',  meta: 'equilibrado' },
    { arg: 'haiku',   label: 'Haiku',   meta: 'mais rápido' },
  ];

  // Niveis reais do /effort do Claude Code, ordenados Faster -> Smarter (6 paradas).
  // 'ultracode' e o topo (= 'xhigh + workflows'). /effort <level> aplica direto.
  const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max', 'ultracode'];
  const EFFORT_DEFAULT = 1; // medium: parada neutra quando o nivel atual e desconhecido

  // Match por substring case-insensitive: o read-back ('Opus4.8') contem o arg ('opus').
  function isModelActive(arg: string): boolean {
    const cur = currentModel?.toLowerCase();
    return !!cur && cur.includes(arg);
  }

  // Mapeia o esforco atual pra parada do slider. Exato primeiro; senao prefixo (cobre
  // abreviacoes do statusline: 'med' -> medium, 'ultra' -> ultracode). -1 = desconhecido.
  function effortIndex(cur: string | null | undefined): number {
    if (!cur) return -1;
    const c = cur.trim().toLowerCase();
    const exact = EFFORTS.indexOf(c);
    if (exact >= 0) return exact;
    return EFFORTS.findIndex((l) => l.startsWith(c));
  }

  // Parada local do slider: responde ao toque na hora e re-sincroniza com o read-back
  // sempre que ele muda e e reconhecivel (ex: /effort rodado direto no terminal).
  let effortIdx = $state(EFFORT_DEFAULT);
  $effect(() => {
    const i = effortIndex(currentEffort);
    if (i >= 0) effortIdx = i;
  });

  const effortLevel = $derived(EFFORTS[effortIdx]);
  const effortFill = $derived((effortIdx / (EFFORTS.length - 1)) * 100);

  function onEffortSlide(e: Event) {
    const t = e.currentTarget as HTMLInputElement;
    effortIdx = Number(t.value);
    onSelectEffort(EFFORTS[effortIdx]);
  }

  function pickModel(arg: string) {
    onSelectModel(arg);
    onClose();
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Modelo e esforço de raciocínio">
  <h2 class="sheet-title">Modelo</h2>

  <ul class="model-list">
    {#each MODELS as m (m.arg)}
      <li>
        <button
          class="model-row"
          class:active={isModelActive(m.arg)}
          onclick={() => pickModel(m.arg)}
        >
          <span class="model-text">
            <span class="model-name">{m.label}</span>
            <span class="model-meta">{m.meta}</span>
          </span>
          {#if isModelActive(m.arg)}
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
    <span class="effort-current">{effortLevel}</span>
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
    aria-label="Esforço de raciocínio"
    aria-valuetext={effortLevel}
  />
  <div class="ends" aria-hidden="true">
    <span>Mais rápido</span>
    <span>Mais inteligente</span>
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
</style>
