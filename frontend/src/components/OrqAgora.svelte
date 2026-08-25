<script lang="ts">
  import { onMount } from 'svelte';
  import * as m from '../paraglide/messages';
  import type { Server } from '../lib/auth';
  import type { OrqExecucao, OrqTask, PlanDetail } from '@hangar/core';
  import { getPlanForServer } from '@hangar/core';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { etapaAtual, type Etapa } from '../lib/orqAgora';

  interface Props {
    execucao: OrqExecucao;    // com os eventos (vem do detalhe), senão não há linha do tempo
    servidor: Server;
    onNavigateToChat?: (name: string) => void;
  }
  let { execucao, servidor, onNavigateToChat }: Props = $props();

  // O quadro/canvas já provaram a regra: uma tela que mostra várias sessões NUNCA abre um SSE por
  // sessão — o estado vivo vem do store agregado (um stream por servidor, para o app inteiro).
  // `onMount` e NÃO `$effect` (como o Board faz): aqui o componente é o PRIMEIRO a segurar o store,
  // e ligá-lo de dentro do grafo reativo, com derivados deste mesmo componente já lendo `rows`,
  // dava erro de mutação de estado — o start morria calado e a faixa ficava sem sessão e sem
  // steps. No Board o erro não aparece porque a Sidebar já ligou o store antes dele montar.
  onMount(() => {
    sessionsStore.retain();
    return () => sessionsStore.release();
  });

  // Task corrente: a última aberta que ainda não tem veredito de aprovação.
  const corrente = $derived<OrqTask | null>(
    [...execucao.tasks].reverse().find((t) => t.resultado !== 'aprova') ?? null,
  );
  const etapa = $derived<Etapa>(etapaAtual(corrente?.eventos ?? []));
  const ETAPAS: { id: Etapa; rotulo: string }[] = [
    { id: 'execucao', rotulo: m.orq_etapa_execucao() },
    { id: 'revisao', rotulo: m.orq_etapa_revisao() },
    { id: 'portao', rotulo: m.orq_etapa_portao() },
    { id: 'aprovada', rotulo: m.orq_etapa_aprovada() },
  ];
  const indiceEtapa = $derived(ETAPAS.findIndex((e) => e.id === etapa));

  // Sessões que aparecem nos eventos desta execução ∩ o que o store conhece NO MESMO servidor.
  // Uma sessão citada que não está mais viva aparece apagada, com o nome — nunca some calada.
  const citadas = $derived.by(() => {
    const nomes = new Set<string>();
    if (corrente) {
      for (const ev of corrente.eventos ?? []) {
        if (ev.executor) nomes.add(ev.executor);
        if (ev.sessao) nomes.add(ev.sessao);
      }
    }
    return [...nomes].map((nome) => {
      const viva = sessionsStore.rows.find((r) => r.serverId === servidor.id && r.name === nome);
      return { nome, estado: viva?.state ?? null };
    });
  });

  // ── Steps da task corrente ────────────────────────────────────────────────
  // O /plan forka e varre /proc a cada chamada (api.py) — nunca a cada mudança de estado. Busca
  // ao montar e SÓ quando o progresso (plan_done/plan_total) da executora muda no store agregado.
  let plano = $state<PlanDetail | null>(null);
  let semPlano = $state(false);
  const executora = $derived(corrente?.executor ?? '');
  const progressoDaExecutora = $derived.by(() => {
    const r = sessionsStore.rows.find((x) => x.serverId === servidor.id && x.name === executora);
    return r ? `${r.plan_done ?? ''}/${r.plan_total ?? ''}` : '';
  });

  $effect(() => {
    const nome = executora;
    progressoDaExecutora;   // dependência: é o gatilho de refetch
    if (!nome) { plano = null; semPlano = false; return; }
    let vivo = true;
    (async () => {
      try {
        const p = await getPlanForServer(servidor, nome);
        if (!vivo) return;
        plano = p;
        semPlano = p === null;   // 404 = sem plano ativo, caminho normal
      } catch {
        if (vivo) { plano = null; semPlano = true; }
      }
    })();
    return () => { vivo = false; };
  });

  const stepsDaTask = $derived.by(() => {
    if (!plano || !corrente) return [];
    // O plano numera as Tasks a partir de 1, na mesma ordem dos eventos.
    return plano.tasks[corrente.task - 1]?.steps ?? [];
  });

  const trocaRecente = $derived(
    (execucao.eventos_execucao ?? []).filter((e) => e.tipo === 'sessao_trocada').slice(-1)[0] ?? null,
  );
</script>

{#if corrente}
  <section class="agora">
    <div class="topo">
      <div class="task-agora">
        <span class="rot">{m.orq_agora()}</span>
        <strong>T{corrente.task} — {corrente.titulo || m.orq_sem_titulo()}</strong>
        {#if corrente.rodadas > 1}
          <span class="rodada">{corrente.rodadas}ª {m.orq_rodada()}</span>
        {/if}
      </div>
      <div class="pipe">
        {#each ETAPAS as e, i (e.id)}
          <span class="st" class:done={i < indiceEtapa} class:cur={i === indiceEtapa}>{e.rotulo}</span>
        {/each}
      </div>
    </div>

    <div class="caixas">
      <div class="caixa">
        <div class="lbl">
          <span>{m.orq_steps_da_task()}</span>
          {#if stepsDaTask.length}
            <span>{stepsDaTask.filter((s) => s.done).length}/{stepsDaTask.length}</span>
          {/if}
        </div>
        {#if stepsDaTask.length}
          {#each stepsDaTask as s, i (i)}
            <div class="step" class:feito={s.done}>
              <span class="ck">{s.done ? '✓' : '·'}</span>{s.title}
            </div>
          {/each}
        {:else if semPlano}
          <p class="nota">{m.orq_sem_plano()}</p>
        {:else}
          <p class="nota">{m.orq_steps_carregando()}</p>
        {/if}
      </div>

      <div class="caixa">
        <div class="lbl"><span>{m.orq_trabalhando()}</span></div>
        {#each citadas as s (s.nome)}
          <button class="sessao" class:morta={!s.estado} onclick={() => onNavigateToChat?.(s.nome)}>
            <span class="dot {s.estado ?? 'off'}"></span>
            <span class="nome">{s.nome}</span>
            <span class="est">{s.estado ?? m.orq_fora()}</span>
          </button>
        {/each}
        {#if trocaRecente}
          <p class="nota">{m.orq_troca()}: {trocaRecente.de} → {trocaRecente.para}</p>
        {/if}
      </div>
    </div>
  </section>
{/if}

<style>
  .agora {
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    background: var(--surface-card);
  }
  .topo { display: flex; align-items: center; gap: var(--space-3); flex-wrap: wrap; }
  .task-agora { display: flex; align-items: baseline; gap: var(--space-2); min-width: 0; flex: 1; flex-wrap: wrap; }
  /* Estreito: título e pipeline em linhas próprias. Lado a lado, o título de uma task longa
     quebrava em três linhas e o pipeline passava por cima dele. */
  @container (max-width: 620px) {
    .topo { flex-direction: column; align-items: stretch; gap: var(--space-2); }
    .pipe { align-self: flex-start; }
  }
  .task-agora .rot { color: var(--text-muted); font-size: var(--text-xs); text-transform: uppercase; letter-spacing: 0.07em; }
  .rodada { color: var(--warning); font-size: var(--text-xs); font-weight: 600; }

  .pipe { display: flex; }
  .pipe .st {
    padding: 3px 10px;
    border: 1px solid var(--border-subtle);
    background: var(--surface-raised);
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .pipe .st:first-child { border-radius: 999px 0 0 999px; }
  .pipe .st:last-child { border-radius: 0 999px 999px 0; }
  .pipe .st.done { color: var(--success); }
  .pipe .st.cur { background: var(--accent-dim); border-color: var(--accent); color: var(--accent); font-weight: 600; }

  .caixas { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: var(--space-3); margin-top: var(--space-3); }
  @container (max-width: 700px) { .caixas { grid-template-columns: minmax(0, 1fr); } }
  .caixa { padding: var(--space-2) var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--surface-inset); }
  .lbl { display: flex; justify-content: space-between; color: var(--text-muted); font-size: var(--text-xs); text-transform: uppercase; letter-spacing: 0.07em; margin-bottom: var(--space-1); }
  .step { display: flex; gap: var(--space-2); padding: 2px 0; color: var(--text-muted); font-size: var(--text-sm); }
  .step.feito { color: var(--text-secondary); }
  .step .ck { flex: none; width: 14px; text-align: center; }
  .step.feito .ck { color: var(--success); }
  .nota { color: var(--text-muted); font-size: var(--text-xs); }

  .sessao { display: flex; align-items: center; gap: var(--space-2); width: 100%; padding: var(--space-1) 0; background: transparent; color: inherit; text-align: left; }
  .sessao:hover .nome { color: var(--text-primary); }
  .sessao.morta { opacity: 0.55; }
  .sessao .nome { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); font-size: var(--text-sm); color: var(--text-secondary); }
  .sessao .est { color: var(--text-muted); font-size: var(--text-xs); }
  .dot { flex: none; width: 8px; height: 8px; border-radius: 50%; background: var(--text-muted); }
  .dot.working { background: var(--accent); }
  .dot.awaiting_input { background: var(--warning); }
  .dot.idle { background: var(--text-muted); }
</style>
