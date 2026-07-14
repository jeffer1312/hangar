<script lang="ts">
  // Task C: modelo + reasoning effort pra sessoes Codex (equivalente do ModelEffortSheet do
  // Claude). Auto-contido igual ao CodexLimitsSheet (Task B): busca a lista + escolha atual ao
  // abrir (GET /models) e aplica na hora (POST /model) -- sem estado intermediario no Composer,
  // que so recebe o resultado via onApplied pra atualizar o pill otimista.
  import BottomSheet from './BottomSheet.svelte';
  import { getCodexModels, setCodexModel } from '../lib/api';
  import type { CodexModel } from '../lib/types';

  interface Props {
    open: boolean;
    sessionName: string;
    onApplied: (model: string, effort: string | null) => void;
    onClose: () => void;
  }
  let { open, sessionName, onApplied, onClose }: Props = $props();

  let models = $state<CodexModel[]>([]);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let applying = $state(false);
  let selectedModel = $state<string | null>(null);
  let selectedEffort = $state<string | null>(null);

  async function load() {
    err = null;
    loading = true;
    try {
      const res = await getCodexModels(sessionName);
      models = res.models;
      selectedModel = res.current.model ?? models[0]?.model ?? null;
      const m = models.find((x) => x.model === selectedModel);
      selectedEffort = res.current.effort ?? m?.defaultEffort ?? m?.efforts[0]?.value ?? null;
    } catch (e) {
      err = e instanceof Error ? e.message : 'Falha ao carregar modelos';
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (open) load();
  });

  const selected = $derived(models.find((m) => m.model === selectedModel) ?? null);

  function pickModel(m: CodexModel) {
    selectedModel = m.model;
    // troca de modelo -> resolve pro effort default do NOVO modelo (senao ficaria mostrando um
    // esforco que ele nem suporta).
    selectedEffort = m.defaultEffort ?? m.efforts[0]?.value ?? null;
  }

  async function apply() {
    if (!selectedModel || applying) return;
    applying = true;
    err = null;
    try {
      await setCodexModel(sessionName, selectedModel, selectedEffort);
      onApplied(selectedModel, selectedEffort);
    } catch (e) {
      err = e instanceof Error ? e.message : 'Falha ao aplicar';
      applying = false;
      return; // mantem a folha aberta pra tentar de novo
    }
    applying = false;
    onClose();
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Modelo do Codex">
  <h2 class="sheet-title">Modelo</h2>

  {#if err}
    <p class="err">{err}</p>
  {/if}

  {#if loading && !models.length}
    <p class="empty">Carregando…</p>
  {:else if !models.length}
    {#if !err}<p class="empty">Nenhum modelo disponível.</p>{/if}
  {:else}
    <ul class="model-list">
      {#each models as m (m.model)}
        <li>
          <button
            class="model-row"
            class:active={selectedModel === m.model}
            aria-pressed={selectedModel === m.model}
            onclick={() => pickModel(m)}
          >
            <span class="model-text">
              <span class="model-name">{m.displayName ?? m.model}</span>
              {#if m.description}<span class="model-meta">{m.description}</span>{/if}
            </span>
            {#if selectedModel === m.model}
              <svg class="check" width="18" height="18" viewBox="0 0 24 24" fill="none"
                stroke="currentColor" stroke-width="2.5" stroke-linecap="round"
                stroke-linejoin="round" aria-hidden="true">
                <polyline points="20 6 9 17 4 12" />
              </svg>
            {/if}
          </button>
        </li>
      {/each}
    </ul>

    {#if selected && selected.efforts.length}
      <h3 class="section-label">Esforço de raciocínio</h3>
      <ul class="effort-list">
        {#each selected.efforts as e (e.value)}
          <li>
            <button
              class="effort-row"
              class:active={selectedEffort === e.value}
              aria-pressed={selectedEffort === e.value}
              onclick={() => (selectedEffort = e.value)}
            >
              <span class="effort-name">{e.value}</span>
              {#if e.description}<span class="effort-meta">{e.description}</span>{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    <div class="actions">
      <button class="btn btn--primary" onclick={apply} disabled={applying || !selectedModel}>
        {applying ? 'Aplicando…' : 'Aplicar'}
      </button>
    </div>
    <p class="hint">Vale a partir da próxima mensagem enviada.</p>
  {/if}
</BottomSheet>

<style>
  .sheet-title { font-size: var(--text-xl); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-4); }
  .err { color: var(--error); font-size: var(--text-sm); margin-bottom: var(--space-3); }
  .empty { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: var(--space-4) 0; }

  .model-list, .effort-list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin-bottom: var(--space-4);
  }

  .model-row, .effort-row {
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

  .model-row:active, .effort-row:active { background: var(--bg-hover); }
  .model-row.active, .effort-row.active { background: var(--accent-dim); }

  .model-text, .effort-row {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    min-width: 0;
  }

  .model-name, .effort-name {
    font-size: var(--text-base);
    font-weight: 500;
    line-height: 1.3;
    color: var(--text-primary);
  }
  .effort-name { text-transform: capitalize; }

  .model-meta, .effort-meta {
    font-size: var(--text-sm);
    line-height: 1.3;
    color: var(--text-muted);
  }

  .check { color: var(--accent); flex-shrink: 0; }

  .section-label {
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-secondary);
    margin-bottom: var(--space-2);
  }

  .actions { margin-top: var(--space-3); }

  .btn {
    width: 100%;
    min-height: 48px;
    border-radius: var(--radius-md);
    font-size: var(--text-base);
    font-weight: 600;
    transition: background 180ms var(--ease-out), opacity 180ms var(--ease-out);
  }
  .btn:disabled { opacity: 0.55; cursor: default; }
  .btn--primary { background: var(--accent); color: #fff; }
  .btn--primary:active:not(:disabled) { background: var(--accent-press); }

  .hint {
    margin-top: var(--space-2);
    font-size: var(--text-xs);
    color: var(--text-muted);
    text-align: center;
  }
</style>
