<script lang="ts">
  import { selectOption } from '../lib/api';
  import { getActiveId, selectServer } from '../lib/auth';
  import { attentionFeed } from '../lib/format';
  import type { AggSession } from '../lib/types';

  interface Props {
    rows: AggSession[];
    onOpenSession: (session: AggSession) => void;
  }

  let { rows, onOpenSession }: Props = $props();
  const feed = $derived(attentionFeed(rows));
  const first = $derived(feed[0] ?? null);
  let busy = $state(false);

  async function pick(option: number) {
    if (!first || busy) return;
    busy = true;
    const prev = getActiveId();
    selectServer(first.serverId);
    try {
      await selectOption(first.name, option);
    } catch {
      // O SSE agregado mantém o estado correto; falha de rede não pode virar rejection solta.
    } finally {
      if (prev && prev !== first.serverId) selectServer(prev);
      busy = false;
    }
  }
</script>

{#if first}
  <section class="attention-strip" aria-label={`${feed.length} sessão${feed.length === 1 ? '' : 'ões'} precisa${feed.length === 1 ? '' : 'm'} de você`}>
    <button class="attention-main" type="button" onclick={() => onOpenSession(first)}>
      <span class="pulse" aria-hidden="true"></span>
      <span class="attention-copy">
        <span class="attention-name">{first.name}</span>
        <span class="attention-question">{first.question ?? 'Aguardando sua resposta'}</span>
      </span>
      <span class="server" style="color: {first.serverColor}">{first.serverLabel}</span>
    </button>

    {#if first.options?.length}
      <div class="quick-options" aria-label="Respostas rápidas">
        {#each first.options.slice(0, 2) as option, i}
          <button type="button" disabled={busy} onclick={() => pick(i + 1)}>{option}</button>
        {/each}
      </div>
    {/if}

    {#if feed.length > 1}
      <button class="more" type="button" onclick={() => onOpenSession(first)}
              aria-label={`Mais ${feed.length - 1} sessões aguardando`}>
        +{feed.length - 1}
      </button>
    {/if}
  </section>
{/if}

<style>
  .attention-strip {
    width: min(760px, calc(100% - 32px));
    min-height: 42px;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 4px;
    border: 1px solid color-mix(in srgb, var(--warning) 28%, var(--border-subtle));
    border-radius: var(--radius-lg);
    background: color-mix(in srgb, var(--bg-elevated) 94%, var(--warning) 6%);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.2);
    pointer-events: auto;
  }

  .attention-main {
    min-width: 0;
    flex: 1;
    min-height: 34px;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: 0 var(--space-2);
    border-radius: var(--radius-md);
    text-align: left;
  }

  .attention-main:hover { background: var(--bg-hover); }

  .pulse {
    width: 8px;
    height: 8px;
    flex-shrink: 0;
    border-radius: 50%;
    background: var(--warning);
    animation: attention-pulse 1.8s var(--ease-out) infinite;
  }

  @keyframes attention-pulse { 50% { opacity: 0.35; } }

  .attention-copy {
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
  }

  .attention-name {
    flex-shrink: 0;
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-weight: 650;
  }

  .attention-question {
    overflow: hidden;
    color: var(--text-secondary);
    font-size: var(--text-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .server {
    max-width: 110px;
    overflow: hidden;
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 650;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .quick-options {
    display: flex;
    gap: 4px;
  }

  .quick-options button, .more {
    min-height: 32px;
    max-width: 150px;
    padding: 0 var(--space-3);
    overflow: hidden;
    border: 1px solid color-mix(in srgb, var(--warning) 30%, var(--border-subtle));
    border-radius: var(--radius-md);
    color: var(--warning);
    font-size: var(--text-xs);
    font-weight: 620;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .quick-options button:hover, .more:hover {
    background: color-mix(in srgb, var(--warning) 10%, transparent);
  }

  .quick-options button:disabled { opacity: 0.55; }
  .more { min-width: 34px; padding: 0 var(--space-2); }

  @media (max-width: 1120px) {
    .server, .quick-options { display: none; }
  }

  @media (prefers-reduced-motion: reduce) {
    .pulse { animation: none; }
  }
</style>
