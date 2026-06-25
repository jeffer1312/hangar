<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import type { CommandInfo } from '../lib/types';

  // Folha de comandos: busca no topo + lista agrupada (Built-ins, Suas skills, Plugins).
  // Comportamento ao tocar: destrutivos pedem confirmacao inline; model/effort abrem o
  // ModelEffortSheet (evita o TUI cego do /model); comandos com argumento preenchem o
  // textarea; o resto envia direto.
  interface Props {
    open: boolean;
    commands: CommandInfo[];
    onCommand: (cmd: string) => void; // envia "/nome"
    onFill: (name: string) => void; // preenche "/nome " no textarea
    onOpenModelEffort: () => void;
    onClose: () => void;
  }
  let { open, commands, onCommand, onFill, onOpenModelEffort, onClose }: Props = $props();

  let query = $state('');
  let confirming = $state<string | null>(null);

  // Zera busca e confirmacao toda vez que a folha abre.
  $effect(() => {
    if (open) {
      query = '';
      confirming = null;
    }
  });

  function matches(c: CommandInfo): boolean {
    const q = query.trim().toLowerCase();
    if (!q) return true;
    return (
      c.name.toLowerCase().includes(q) ||
      (c.description?.toLowerCase().includes(q) ?? false)
    );
  }

  const filtered = $derived(commands.filter(matches));
  const groups = $derived(
    (
      [
        { key: 'builtin', label: 'Built-ins' },
        { key: 'skill', label: 'Suas skills' },
        { key: 'plugin', label: 'Plugins' },
      ] as const
    )
      .map((g) => ({ ...g, items: filtered.filter((c) => c.source === g.key) }))
      .filter((g) => g.items.length > 0)
  );

  function handleTap(c: CommandInfo) {
    if (c.name === 'model' || c.name === 'effort') {
      onOpenModelEffort();
      onClose();
      return;
    }
    if (c.destructive) {
      confirming = c.name; // pede confirmacao inline antes de enviar
      return;
    }
    if (c.argumentHint) {
      onFill(c.name); // tem argumento -> preenche e deixa o usuario digitar
      onClose();
      return;
    }
    onCommand('/' + c.name); // zero-arg -> envia
    onClose();
  }

  function confirm(c: CommandInfo) {
    confirming = null;
    onCommand('/' + c.name);
    onClose();
  }

  function badge(source: CommandInfo['source']): string {
    return source === 'builtin' ? 'base' : source === 'plugin' ? 'plugin' : 'skill';
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Comandos">
  <h2 class="sheet-title">Comandos</h2>

  <input
    type="text"
    class="search"
    bind:value={query}
    placeholder="Buscar comando"
    autocomplete="off"
    autocorrect="off"
    autocapitalize="off"
    spellcheck={false}
    aria-label="Buscar comando"
  />

  {#if groups.length === 0}
    <p class="empty">Nenhum comando encontrado.</p>
  {:else}
    <div class="groups">
      {#each groups as g (g.key)}
        <section class="group">
          <h3 class="group-label">{g.label}</h3>
          <ul class="cmd-list">
            {#each g.items as c (c.source + ':' + c.name)}
              <li>
                {#if confirming === c.name}
                  <div class="cmd-row cmd-row--confirm">
                    <span class="confirm-text">Confirmar {c.display}?</span>
                    <div class="confirm-actions">
                      <button class="cbtn cbtn--no" onclick={() => (confirming = null)}>Não</button>
                      <button class="cbtn cbtn--yes" onclick={() => confirm(c)}>Sim</button>
                    </div>
                  </div>
                {:else}
                  <button class="cmd-row" onclick={() => handleTap(c)}>
                    <span class="cmd-main">
                      <span class="cmd-name">{c.display}</span>
                      {#if c.description}<span class="cmd-desc">{c.description}</span>{/if}
                    </span>
                    {#if c.argumentHint}<span class="arg">{c.argumentHint}</span>{/if}
                    <span class="badge badge--{c.source}">{badge(c.source)}</span>
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    </div>
  {/if}
</BottomSheet>

<style>
  .sheet-title {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-4);
  }

  .search {
    width: 100%;
    height: 44px;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    padding: 0 var(--space-3);
    outline: none;
    margin-bottom: var(--space-4);
    transition: border-color 180ms var(--ease-out);
  }

  .search::placeholder {
    color: var(--text-muted);
  }

  .search:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .groups {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-height: 52vh;
    overflow-y: auto;
  }

  .group-label {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-secondary);
    margin-bottom: var(--space-2);
  }

  .cmd-list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .cmd-row {
    width: 100%;
    min-height: 44px;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    text-align: left;
    background: transparent;
    transition: background 160ms var(--ease-out);
  }

  .cmd-row:active {
    background: var(--bg-hover);
  }

  .cmd-main {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }

  .cmd-name {
    font-family: var(--font-mono);
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
  }

  .cmd-desc {
    font-size: var(--text-sm);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .arg {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-secondary);
    white-space: nowrap;
    flex-shrink: 0;
  }

  .badge {
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 2px 6px;
    border-radius: var(--radius-full);
    color: var(--text-secondary);
    background: var(--bg-hover);
  }

  .badge--skill {
    color: var(--accent);
    background: var(--accent-dim);
  }

  .badge--plugin {
    color: var(--warning);
    background: rgba(255, 159, 10, 0.14);
  }

  /* ── Confirmacao inline de comando destrutivo ───────────────────────────── */
  .cmd-row--confirm {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    min-height: 44px;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    background: var(--pill-dead-bg);
  }

  .confirm-text {
    font-size: var(--text-sm);
    color: var(--text-primary);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .confirm-actions {
    display: flex;
    gap: var(--space-2);
    flex-shrink: 0;
  }

  .cbtn {
    padding: 0 var(--space-3);
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
    font-weight: 600;
  }

  .cbtn--no {
    color: var(--text-secondary);
    background: var(--bg-hover);
  }

  .cbtn--yes {
    color: #fff;
    background: var(--error);
  }

  .empty {
    font-size: var(--text-sm);
    color: var(--text-muted);
    text-align: center;
    padding: var(--space-4) 0;
  }
</style>
