<script lang="ts">
  import { computeEditDiff, extractEdits, extractFilePath, type ChatEvent } from '@hangar/core';
  import * as m from '../paraglide/messages';
  import { summarizeToolInput, toolGroupCounts, toolGroupLabel, toolPhase } from '@hangar/core';
  import { toolLook } from '../lib/toolLook.svelte';
  import ToolCard from './ToolCard.svelte';
  import HangarTrail, { type PassoHangar } from './HangarTrail.svelte';
  import { lerComandoHangar } from '../lib/hangarCmd';

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

  // Comandos do hangar deste grupo, na ordem. Só entram os que já têm resultado: sem ele não há o
  // que ler, e um passo "?" na trilha seria pior que a ausência dele.
  const trilha = $derived.by<PassoHangar[]>(() => {
    const passos: PassoHangar[] = [];
    for (const t of tools) {
      if (t.tool_name !== 'Bash') continue;
      const r = resultOf(t);
      if (!r) continue;
      const comando = String((t.tool_input as Record<string, unknown> | null)?.['command'] ?? '');
      const acao = lerComandoHangar(comando, String(r.result ?? ''), toolPhase(r) === 'error');
      if (!acao) continue;
      passos.push({
        acao,
        hora: t.ts ? new Date(t.ts * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' }) : null,
      });
    }
    return passos;
  });
  // Do início do primeiro comando ao fim do último — inclui o que rodou ENTRE eles, e é isso mesmo
  // que a trilha mede: quanto tempo a sequência levou.
  const duracaoTrilha = $derived.by(() => {
    const comandos = tools.filter((t) => t.tool_name === 'Bash' && resultOf(t));
    const primeiro = comandos[0]?.ts;
    const ultimo = comandos.length ? resultOf(comandos[comandos.length - 1])?.ts : null;
    return primeiro && ultimo ? Math.max(0, (ultimo - primeiro) * 1000) : null;
  });

  const phases = $derived(tools.map((t) => toolPhase(resultOf(t))));
  const label = $derived(toolGroupLabel(tools.map((t) => t.tool_name)));
  const counts = $derived(toolGroupCounts(phases));
  const anyError = $derived(phases.includes('error'));
  // Todas do mesmo tipo -> o nome ja esta no cabecalho e some de cada filho (o que a arvore do Pi
  // faz); misturadas -> cada filho carrega o proprio nome, senao a linha vira um path sem dono.
  const mixed = $derived(label === m.lista_ferramentas());

  // Faixa de chips de diff (pele 'chips'): o resumo dos ARQUIVOS que a rodada tocou, com +add/-del,
  // que a pele classica nao tem — hoje o diff so existe dentro de cada Edit, e uma rodada de 8
  // ferramentas nao diz em lugar nenhum "mexeu nestes 3 arquivos". Sai dos MESMOS extractEdits/
  // extractFilePath que o ToolCard ja usa; nada de dado novo. Mesmo arquivo editado 2x soma num chip
  // so (o Map junta por caminho), senao uma rodada de MultiEdit viraria uma parede de chips iguais.
  const TETO_CHIPS = 6;
  const diffs = $derived.by(() => {
    const por = new Map<string, { file: string; add: number; del: number }>();
    for (const t of tools) {
      const edits = extractEdits(t.tool_name, t.tool_input);
      if (!edits) continue;
      const caminho = extractFilePath(t.tool_input) || '?';
      const nome = caminho.split('/').pop() || caminho;
      const acc = por.get(caminho) ?? { file: nome, add: 0, del: 0 };
      // extractEdits devolve os TEXTOS; quem conta linha e o computeEditDiff (Myers, ja com teto
      // interno). E o mesmo calculo que o EditDiff faz ao abrir — aqui so o total interessa.
      for (const e of edits) {
        const d = computeEditDiff(e.oldText, e.newText);
        acc.add += d.add;
        acc.del += d.del;
      }
      por.set(caminho, acc);
    }
    return [...por.values()];
  });
  const chipsVisiveis = $derived(diffs.slice(0, TETO_CHIPS));
  const chipsOcultos = $derived(diffs.length - chipsVisiveis.length);

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
<!-- Rajada de comandos do hangar: a trilha resume a sequência ANTES do grupo, e o grupo continua
     ali com os cartões um a um. Ela só aparece com 2+ comandos lidos — com um só o cartão já conta
     a história inteira. -->
{#if trilha.length > 1}
  <HangarTrail passos={trilha} total={duracaoTrilha} />
{/if}
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
    {#if toolLook.look === 'chips'}
      <!-- Cabeçalho da pele 'chips': chevron + contagem, e mais nada — no original a linha é limpa
           e o próprio chevron ensina que abre. A bolinha de estado e a dica de expandir/ocultar da
           pele clássica sairiam de cena aqui, então o ERRO ganha a cor no lugar da bolinha. -->
      <svg class="tg-chevron" class:open={expanded} width="12" height="12" viewBox="0 0 24 24"
           fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"
           stroke-linejoin="round" aria-hidden="true"><path d="M6 9l6 6 6-6" /></svg>
      <span class="tg-resumo">{label.toLowerCase()}, {counts}</span>
    {:else}
      <span class="tg-dot" class:pending={phases.includes('pending')}
            data-phase={anyError ? 'error' : phases.includes('pending') ? 'pending' : 'done'} aria-hidden="true"></span>
      <span class="tg-label">{label}:</span>
      <span class="tg-counts">{counts}</span>
      <span class="tg-hint">
        <span class="sep" aria-hidden="true">•</span>
        <span class="coarse">{expanded ? m.tool_toque_ocultar() : m.tool_toque_ver()}</span><span
              class="fine">{expanded ? m.tool_clique_ocultar() : m.tool_clique_ver()}</span>
      </span>
    {/if}
  </div>

  {#if expanded}
    <div class="tg-body" class:tg-body--chips={toolLook.look === 'chips'}>
      {#each tools as t (t.id)}
        <ToolCard event={t} result={resultOf(t)} {sessionName} animate={false} />
      {/each}

      <!-- A faixa de arquivos vive DENTRO do expandido, como no original: fechado o grupo mostra
           só o cabeçalho. Fora daqui ela vazava pra baixo de um grupo recolhido e a linha deixava
           de ser uma linha. -->
      {#if toolLook.look === 'chips' && diffs.length}
        <div class="tg-diffs">
          {#each chipsVisiveis as d, i (d.file + i)}
            <span class="dchip" style:animation-delay="{i * 80}ms">
              <span class="dchip-file">{d.file}</span>
              <span class="dchip-add">+{d.add}</span>
              {#if d.del > 0}<span class="dchip-del">−{d.del}</span>{/if}
            </span>
          {/each}
          {#if chipsOcultos > 0}<span class="dchip-mais">+{chipsOcultos} {chipsOcultos > 1 ? m.tool_outros() : m.tool_outro_1()}</span>{/if}
        </div>
      {/if}
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

  /* Cabeçalho da pele 'chips'. */
  .tg-chevron {
    flex-shrink: 0;
    align-self: center;
    color: var(--text-muted);
    transition: transform 200ms var(--ease-out);
    transform: rotate(-90deg);
  }
  .tg-chevron.open { transform: rotate(0deg); }
  .tg-resumo {
    min-width: 0;
    font-size: 12.5px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tg-head--error .tg-resumo { color: var(--error); }

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

  /* Faixa de chips de diff (pele 'chips'): resumo dos arquivos da rodada. */
  .tg-diffs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--border-subtle);
  }

  /* Retangulo arredondado de 28px, como o original — nao pilula: ele e um BOTAO de arquivo, e a
     forma tem que conversar com o chip do argumento da linha de cima. */
  .dchip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    height: 28px;
    padding: 0 8px;
    border-radius: 6px;
    /* Mesma tinta do chip do argumento (ver --fill-subtle no app.css): degrau pra cima, translúcido,
       anda junto do tema — nao slab opaco. */
    background: var(--fill-subtle);
    /* Anel de 1px + sombra rasa: as duas medidas saem do computed style do original. */
    box-shadow: 0 0 0 1px var(--border-subtle), 0 1px 2px rgba(0, 0, 0, 0.18);
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text);
    animation: pop-in 250ms cubic-bezier(0.23, 1, 0.32, 1) both;
  }
  .dchip-file { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .dchip-add { flex-shrink: 0; color: var(--success); font-variant-numeric: tabular-nums; }
  .dchip-del { flex-shrink: 0; color: var(--error); font-variant-numeric: tabular-nums; }

  .dchip-mais {
    align-self: center;
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-muted);
  }

  /* Corpo expandido: os ToolCards individuais, recuados sob o tronco do grupo. */
  .tg-body { padding-left: var(--space-3); border-left: 1px solid var(--border-subtle); margin-left: 2px; }

  /* Na pele 'chips' o tronco lateral sai (o recuo já vem do glifo) e as linhas ganham respiro:
     4px entre uma chamada e outra, como no original. */
  .tg-body--chips {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-left: 0;
    margin-left: 0;
    border-left: none;
  }
</style>
