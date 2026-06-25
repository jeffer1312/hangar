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

  const EFFORTS = ['low', 'medium', 'high', 'max'];

  // Match por substring case-insensitive: o read-back ('Opus4.8') contem o arg ('opus').
  function isModelActive(arg: string): boolean {
    const cur = currentModel?.toLowerCase();
    return !!cur && cur.includes(arg);
  }

  // Esforco aceita match nos dois sentidos ('med' do statusline x 'medium' do segmento).
  function isEffortActive(level: string): boolean {
    const cur = currentEffort?.toLowerCase();
    if (!cur) return false;
    return level.includes(cur) || cur.includes(level);
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

  <h3 class="section-label">Esforço de raciocínio</h3>
  <div class="effort" role="radiogroup" aria-label="Esforço de raciocínio">
    {#each EFFORTS as level (level)}
      <button
        class="effort-seg"
        class:active={isEffortActive(level)}
        role="radio"
        aria-checked={isEffortActive(level)}
        onclick={() => onSelectEffort(level)}
      >
        {level}
      </button>
    {/each}
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

  /* ── Esforco: controle segmentado ──────────────────────────────────────── */
  .section-label {
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-secondary);
    margin-bottom: var(--space-2);
  }

  .effort {
    display: flex;
    gap: var(--space-1);
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    padding: var(--space-1);
  }

  .effort-seg {
    flex: 1;
    min-width: 0;
    min-height: 44px;
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-secondary);
    background: transparent;
    font-variant-numeric: tabular-nums;
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out);
  }

  .effort-seg.active {
    background: var(--accent-dim);
    color: var(--accent);
  }
</style>
