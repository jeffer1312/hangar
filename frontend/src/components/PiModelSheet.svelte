<script lang="ts">
  // Modelo + nivel de raciocinio pra sessoes Pi — irmao do CodexModelSheet (mesmo formato:
  // auto-contido, busca a lista ao abrir e aplica na hora), com uma diferenca que vem da medicao:
  // o Pi expoe ~300 modelos, entao a lista tem CAMPO DE BUSCA e so renderiza os primeiros
  // resultados. Os niveis de raciocinio nao sao fixos: cada modelo aceita um conjunto (o backend
  // devolve `levels`), e depois de trocar de modelo eles mudam — por isso o read-back manda.
  import BottomSheet from './BottomSheet.svelte';
  import { getPiModels, setPiModel } from '../lib/api';
  import type { PiModel } from '../lib/types';

  interface Props {
    open: boolean;
    sessionName: string;
    onApplied: (model: string, effort: string | null) => void;
    onClose: () => void;
  }
  let { open, sessionName, onApplied, onClose }: Props = $props();

  const MAX_ROWS = 40;   // teto de linhas desenhadas: 300 botoes travariam o celular

  let models = $state<PiModel[]>([]);
  let levels = $state<string[]>([]);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let applying = $state(false);
  let query = $state('');
  let selected = $state<PiModel | null>(null);
  let selectedEffort = $state<string | null>(null);

  async function load() {
    err = null;
    loading = true;
    try {
      const res = await getPiModels(sessionName);
      models = res.models;
      levels = res.levels;
      selectedEffort = res.thinking;
      selected = res.current
        ? models.find((m) => m.provider === res.current!.provider && m.id === res.current!.id) ?? null
        : null;
    } catch (e) {
      err = e instanceof Error ? e.message : 'Falha ao carregar modelos';
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (open) load();
  });

  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const hit = q
      ? models.filter((m) => `${m.provider}/${m.id} ${m.name}`.toLowerCase().includes(q))
      : models;
    return hit.slice(0, MAX_ROWS);
  });
  const hiddenCount = $derived(
    Math.max(0, (query.trim() ? models.filter((m) => `${m.provider}/${m.id} ${m.name}`.toLowerCase().includes(query.trim().toLowerCase())).length : models.length) - MAX_ROWS),
  );

  function same(a: PiModel | null, b: PiModel) {
    return !!a && a.provider === b.provider && a.id === b.id;
  }

  async function apply() {
    if (applying) return;
    applying = true;
    err = null;
    try {
      const res = await setPiModel(sessionName, {
        provider: selected?.provider,
        model: selected?.id,
        effort: selectedEffort ?? undefined,
      });
      // Read-back: o Pi clampa o nivel pro que o modelo suporta.
      levels = res.levels;
      selectedEffort = res.thinking;
      if (!res.current) {
        // 200 sem modelo no read-back: nao da pra AFIRMAR que a troca pegou. Cair pro `selected`
        // aqui (como era) pintava o pill com o que foi PEDIDO e fechava a folha — o oposto do que o
        // comentario acima promete. Deixa a folha aberta com o estado nao confirmado.
        err = 'Não deu pra confirmar a troca — confira o modelo no terminal da sessão.';
        applying = false;
        return;
      }
      onApplied(res.current.name || res.current.id, res.thinking);
    } catch (e) {
      err = e instanceof Error ? e.message : 'Falha ao aplicar';
      applying = false;
      return;   // mantem a folha aberta pra tentar de novo
    }
    applying = false;
    onClose();
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Modelo do Pi">
  <h2 class="sheet-title">Modelo</h2>

  {#if err}
    <p class="err">{err}</p>
  {/if}

  {#if loading && !models.length}
    <p class="empty">Carregando…</p>
  {:else if !models.length}
    {#if !err}<p class="empty">Nenhum modelo disponível.</p>{/if}
  {:else}
    <input
      class="search"
      type="search"
      bind:value={query}
      placeholder="Buscar modelo…"
      aria-label="Buscar modelo"
    />

    <ul class="model-list">
      {#each filtered as m (m.provider + '/' + m.id)}
        <li>
          <button
            class="model-row"
            class:active={same(selected, m)}
            aria-pressed={same(selected, m)}
            onclick={() => (selected = m)}
          >
            <span class="model-text">
              <span class="model-name">{m.name}</span>
              <span class="model-meta">{m.provider}/{m.id}</span>
            </span>
            {#if same(selected, m)}
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
    {#if hiddenCount}
      <p class="more">+{hiddenCount} — refine a busca</p>
    {/if}

    {#if levels.length}
      <h3 class="section-label">Nível de raciocínio</h3>
      <ul class="effort-list">
        {#each levels as lv (lv)}
          <li>
            <button
              class="effort-row"
              class:active={selectedEffort === lv}
              aria-pressed={selectedEffort === lv}
              onclick={() => (selectedEffort = lv)}
            >
              <span class="effort-name">{lv}</span>
            </button>
          </li>
        {/each}
      </ul>
      <p class="hint hint--left">Vale para o modelo atual; trocar de modelo pode ajustar o nível.</p>
    {/if}

    <div class="actions">
      <button class="btn btn--primary" onclick={apply} disabled={applying}>
        {applying ? 'Aplicando…' : 'Aplicar'}
      </button>
    </div>
  {/if}
</BottomSheet>

<style>
  .sheet-title { font-size: var(--text-xl); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-4); }
  .err { color: var(--error); font-size: var(--text-sm); margin-bottom: var(--space-3); }
  .empty { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: var(--space-4) 0; }

  .search {
    width: 100%;
    min-height: 44px;
    padding: var(--space-2) var(--space-3);
    margin-bottom: var(--space-3);
    border-radius: var(--radius-md);
    background: var(--bg-hover);
    color: var(--text-primary);
    font-size: var(--text-base);
  }

  .model-list, .effort-list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin-bottom: var(--space-4);
  }
  /* A lista pode ter 40 linhas: rola dentro da folha em vez de empurrar o botao Aplicar pra fora. */
  .model-list { max-height: 46vh; overflow-y: auto; }

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

  .model-meta {
    font-size: var(--text-sm);
    line-height: 1.3;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  .check { color: var(--accent); flex-shrink: 0; }

  .more { font-size: var(--text-xs); color: var(--text-muted); text-align: center; margin: calc(-1 * var(--space-3)) 0 var(--space-3); }

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
  .hint--left { text-align: left; margin: calc(-1 * var(--space-3)) 0 var(--space-3); }
</style>
