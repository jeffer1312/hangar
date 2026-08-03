<script lang="ts">
  import type { ChatEvent } from '../lib/types';
  import { summarizeToolInput, toolGroupCounts, toolGroupLabel, toolPhase } from '../lib/format';
  import ToolCard from './ToolCard.svelte';

  interface Props {
    tools: ChatEvent[];
    // mesmo wrapper de toolResults do MessageList (Map incremental): tool_use_id -> tool_result.
    toolResults: { get: (id: string) => ChatEvent | undefined };
    sessionName: string;
    animate?: boolean;   // false = grupo de HISTORICO remontado (paginacao/janela): sem fade
  }
  let { tools, toolResults, sessionName, animate = true }: Props = $props();

  // Colapsado por padrao: o burst vira UMA linha — cabecalho com a contagem. A arvore de uma linha
  // por chamada saiu: 22 Bash viravam 22 linhas de scroll, e o argumento que ela mostrava e o mesmo
  // que o ToolCard ja poe na linha 1. Tap abre os ToolCards completos (cada um com o proprio tap
  // pra saida). Enquanto o burst roda, a chamada VIVA aparece sob o cabecalho — sem ela o painel
  // diria "3 concluidos" e esconderia o que esta acontecendo agora.
  let expanded = $state(false);

  const resultOf = (t: ChatEvent) => toolResults.get(t.tool_use_id ?? '') ?? null;

  const phases = $derived(tools.map((t) => toolPhase(resultOf(t))));
  const label = $derived(toolGroupLabel(tools.map((t) => t.tool_name)));
  const counts = $derived(toolGroupCounts(phases));
  const anyError = $derived(phases.includes('error'));
  // Todas do mesmo tipo -> o nome ja esta no cabecalho e some de cada filho (o que a arvore do Pi
  // faz); misturadas -> cada filho carrega o proprio nome, senao a linha vira um path sem dono.
  const mixed = $derived(label === 'Ferramentas');

  // A chamada viva: a ULTIMA pendente (a mais nova), como o "$ …" que o Claude mostra sob o resumo.
  const running = $derived.by(() => {
    for (let i = tools.length - 1; i >= 0; i--) if (phases[i] === 'pending') return { t: tools[i], i };
    return null;
  });
</script>

<!-- Uma ferramenta so nao e grupo: "Executou 1 ferramenta ›" esconderia a query atras de um tap a
     mais. Desenha o bloco do ToolCard direto (a regra de agrupar vive no MessageList, mas o guarda
     fica aqui pra valer pra qualquer chamador). -->
{#if tools.length === 1}
  <ToolCard event={tools[0]} result={resultOf(tools[0])} {sessionName} {animate} />
{:else}
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
    <span class="tg-dot" class:pending={phases.includes('pending')}
          data-phase={anyError ? 'error' : phases.includes('pending') ? 'pending' : 'done'} aria-hidden="true"></span>
    <span class="tg-label">{label}:</span>
    <span class="tg-counts">{counts}</span>
    <span class="tg-hint">
      <span class="sep" aria-hidden="true">•</span>
      <span class="coarse">{expanded ? 'toque para ocultar' : 'toque para ver'}</span><span
            class="fine">{expanded ? 'clique para ocultar' : 'clique para ver'}</span>
    </span>
  </div>

  {#if expanded}
    <div class="tg-body">
      {#each tools as t (t.id)}
        <ToolCard event={t} result={resultOf(t)} {sessionName} animate={false} />
      {/each}
    </div>
  {:else if running}
    <!-- Uma linha so: a chamada em curso. O "└" e CSS (tronco + bracinho), nao box-drawing — em
         fonte de sistema o glifo cai em fallback e desalinha da bolinha. -->
    <div class="tg-tree">
      <div class="tg-child">
        <span class="tg-dot tg-dot--child pending" data-phase="pending" aria-hidden="true"></span>
        {#if mixed}<span class="tg-cname">{running.t.tool_name ?? 'Tool'}</span>{/if}
        <span class="tg-arg">{summarizeToolInput(running.t.tool_name, running.t.tool_input)}</span>
      </div>
    </div>
  {/if}
</div>
{/if}

<style>
  .tg { margin-bottom: var(--space-1); animation: bubble-in 180ms ease-out both; }
  .tg.noanim { animation: none; }

  .tg-head {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
    padding: var(--space-1) 0;
    font-size: var(--text-xs);
    line-height: 1.5;
    color: var(--text-muted);
    cursor: pointer;
  }

  /* Mesma bolinha do ToolCard (mesmas cores de estado) — no cabecalho ela resume o grupo. */
  .tg-dot {
    flex-shrink: 0;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    align-self: center;
    background: var(--success);
  }
  .tg-dot[data-phase='pending'] { background: var(--accent); }
  .tg-dot[data-phase='error']   { background: var(--error); }
  .tg-dot.pending { animation: pulse-scale 1.2s ease-in-out infinite; }

  .tg-label { flex-shrink: 0; font-weight: 600; color: var(--text-secondary); }
  .tg-counts { flex-shrink: 0; }
  .tg-head--error .tg-counts { color: var(--error); }

  .tg-hint {
    flex-shrink: 1000;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    opacity: 0.7;
  }
  .tg-hint .sep { margin-right: 4px; }
  .fine { display: inline; }
  .coarse { display: none; }
  @media (pointer: coarse) {
    .fine { display: none; }
    .coarse { display: inline; }
  }

  /* Arvore colapsada. */
  .tg-tree { padding-bottom: var(--space-1); }

  .tg-child {
    position: relative;
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
    padding-left: 14px;
    font-size: var(--text-xs);
    line-height: 1.6;
  }
  /* Tronco vertical que para na metade e vira pra direita = "└". */
  .tg-child::before {
    content: '';
    position: absolute;
    left: 2px;
    top: 0;
    bottom: 50%;
    width: 6px;
    border-left: 1px solid var(--border-default);
    border-bottom: 1px solid var(--border-default);
    border-bottom-left-radius: 3px;
  }

  .tg-dot--child { width: 5px; height: 5px; }

  .tg-cname { flex-shrink: 0; font-weight: 600; color: var(--text-secondary); }

  .tg-arg {
    min-width: 0;
    font-family: var(--font-mono);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Corpo expandido: os ToolCards individuais, recuados sob o tronco do grupo. */
  .tg-body { padding-left: var(--space-3); border-left: 1px solid var(--border-subtle); margin-left: 2px; }
</style>
