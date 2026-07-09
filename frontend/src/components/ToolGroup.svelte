<script lang="ts">
  import type { ChatEvent } from '../lib/types';
  import ToolCard from './ToolCard.svelte';

  interface Props {
    tools: ChatEvent[];
    // mesmo wrapper de toolResults do MessageList (Map incremental): tool_use_id -> tool_result.
    toolResults: { get: (id: string) => ChatEvent | undefined };
    sessionName: string;
    animate?: boolean;   // false = grupo de HISTORICO remontado (paginacao/janela): sem fade
  }
  let { tools, toolResults, sessionName, animate = true }: Props = $props();

  // Colapsado por padrao: um burst de tool calls vira UMA linha muda (o que o usuario pediu, pra nao
  // encher a lista). Tap expande e mostra os ToolCards individuais (cada um ainda expande sozinho).
  let expanded = $state(false);

  const resultOf = (t: ChatEvent) => toolResults.get(t.tool_use_id ?? '') ?? null;

  // Ha algo ainda rodando no grupo? (result ausente = pending, mesma regra do ToolCard). Se sim, o
  // cabecalho mostra "Executando" + spinner, senao "Executou".
  const anyPending = $derived(tools.some((t) => resultOf(t) === null));
  const anyError = $derived(tools.some((t) => resultOf(t)?.is_error === true));

  // Resumo dos nomes de ferramenta com contagem: "Read ×6, Bash ×3, Grep". Cap em 4 tipos + "…".
  const names = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const t of tools) {
      const n = t.tool_name ?? 'Tool';
      counts.set(n, (counts.get(n) ?? 0) + 1);
    }
    const parts = [...counts.entries()].map(([n, c]) => (c > 1 ? `${n} ×${c}` : n));
    return parts.slice(0, 4).join(', ') + (parts.length > 4 ? '…' : '');
  });
</script>

<div class="tg" class:noanim={!animate}>
  <div
    class="tg-head"
    class:tg-head--error={anyError}
    role="button"
    tabindex="0"
    aria-expanded={expanded}
    onclick={() => (expanded = !expanded)}
    onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); expanded = !expanded; } }}
  >
    {#if anyPending}<span class="tg-spin" aria-label="Executando…">⟳</span>{/if}
    <span class="tg-label">
      {anyPending ? 'Executando' : 'Executou'} <span class="tg-count">{tools.length} ferramentas</span>
      {#if names} · <span class="tg-names">{names}</span>{/if}
    </span>
    <span class="tg-chevron" class:open={expanded} aria-hidden="true">›</span>
  </div>

  {#if expanded}
    <div class="tg-body">
      {#each tools as t (t.id)}
        <ToolCard event={t} result={resultOf(t)} {sessionName} animate={false} />
      {/each}
    </div>
  {/if}
</div>

<style>
  .tg { margin-bottom: var(--space-1); animation: bubble-in 180ms ease-out both; }
  .tg.noanim { animation: none; }

  /* Cabecalho colapsado: mesma linguagem visual do ToolCard (.tool-row), mas com um traco a esquerda
     pra ler como "grupo". */
  .tg-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    min-height: 32px;
    padding: var(--space-1) 0 var(--space-1) var(--space-2);
    border-left: 2px solid var(--border-default);
    cursor: pointer;
  }

  .tg-spin {
    flex-shrink: 0;
    color: var(--text-muted);
    display: inline-block;
    animation: spin 0.8s linear infinite;
    font-size: var(--text-xs);
  }

  .tg-label {
    flex: 1;
    min-width: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tg-count { font-family: var(--font-mono); color: var(--text-secondary); }
  .tg-names { color: var(--text-muted); }
  .tg-head--error .tg-label { color: var(--error); }

  .tg-chevron {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--text-base);
    transition: transform 180ms var(--ease-out);
  }
  .tg-chevron.open { transform: rotate(90deg); }

  /* Corpo expandido: os ToolCards individuais, recuados sob o traco do grupo. */
  .tg-body { padding-left: var(--space-3); border-left: 2px solid var(--border-subtle); margin-left: var(--space-2); }
</style>
