<script lang="ts">
  import type { CommandInfo } from '../lib/types';

  // Tira inline de autocomplete acima do textarea. Aparece so quando o texto comeca com '/'
  // (primeiro caractere nao-branco) e ainda nao tem argumento (sem espaco depois do nome).
  // Renderiza no fluxo normal, acima do input, pra nunca ficar atras do teclado.
  interface Props {
    commands: CommandInfo[];
    query: string;
    onPick: (cmd: CommandInfo) => void;
  }
  let { commands, query, onPick }: Props = $props();

  const MAX = 8;

  const trimmed = $derived(query.replace(/^\s+/, ''));
  const active = $derived(trimmed.startsWith('/'));
  // prefixo digitado apos a '/', ate o primeiro espaco
  const token = $derived(active ? trimmed.slice(1).split(/\s/)[0].toLowerCase() : '');
  // ja entrou argumento (tem espaco depois do nome) -> some com as sugestoes
  const typingArgs = $derived(active && /\s/.test(trimmed.slice(1)));

  function rank(c: CommandInfo): number {
    const n = c.name.toLowerCase();
    if (token === '') return 1;
    if (n.startsWith(token)) return 0; // melhor: casa o prefixo
    if (n.includes(token)) return 1; // fuzzy leve: substring
    return -1; // sem match
  }

  const matches = $derived(
    !active || typingArgs
      ? []
      : commands
          .map((c) => ({ c, r: rank(c) }))
          .filter((x) => x.r >= 0)
          .sort((a, b) => a.r - b.r)
          .slice(0, MAX)
          .map((x) => x.c)
  );

  function badge(source: CommandInfo['source']): string {
    return source === 'builtin' ? 'base' : source === 'plugin' ? 'plugin' : 'skill';
  }
</script>

{#if matches.length > 0}
  <div class="suggest" role="listbox" aria-label="Sugestões de comando">
    {#each matches as c (c.name)}
      <button class="row" role="option" aria-selected="false" onclick={() => onPick(c)}>
        <span class="name">{c.display}</span>
        {#if c.description}<span class="desc">{c.description}</span>{/if}
        <span class="badge badge--{c.source}">{badge(c.source)}</span>
      </button>
    {/each}
  </div>
{/if}

<style>
  .suggest {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 168px; /* ~3-4 linhas; rola se passar */
    overflow-y: auto;
    margin-bottom: var(--space-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: var(--bg-surface);
    padding: var(--space-1);
  }

  .row {
    width: 100%;
    min-height: 44px;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 0 var(--space-2);
    border-radius: var(--radius-sm);
    text-align: left;
    background: transparent;
    transition: background 160ms var(--ease-out);
  }

  .row:active {
    background: var(--bg-hover);
  }

  .name {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
    flex-shrink: 0;
  }

  .desc {
    font-size: var(--text-xs);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
    min-width: 0;
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
</style>
