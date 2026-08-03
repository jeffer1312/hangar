<script lang="ts">
  // Detalhe do plano: barra + Tasks. Só a Task atual abre os steps — "próximo passo" não é campo, é o
  // primeiro ○ da lista. O markdown cru vem do próprio /plan (o /file não serve este arquivo).
  import PlanBar from './PlanBar.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import { getPlans, setPlanPin, type PlanListItem } from '../lib/api';
  import type { PlanDetail, SessionInfo } from '../lib/types';

  interface Props {
    session: SessionInfo;
    detail: PlanDetail | null;
    loading?: boolean;
    // Falha REAL ao ler o plano (500/rede/token). "Sem plano" é 404 e chega como detail=null.
    error?: boolean;
  }
  let { session, detail, loading = false, error = false }: Props = $props();
  let showMd = $state(false);

  // Mesmo sentinela do backend (planprog.PIN_NONE): "nenhum" e gravado como pin.
  const PIN_NONE = '!none';

  // Lista de Tasks fechavel: num plano de 6 Tasks a Task atual sozinha ja passa de 10 linhas, e o
  // painel come a tela mesmo com tudo pronto. Preferencia GLOBAL (nao por sessao): quem fecha quer
  // o painel enxuto sempre, nao naquele plano. localStorage direto — uma chave nao merece store.
  const CHAVE = 'cp_plan_tasks';
  let showTasks = $state(localStorage.getItem(CHAVE) !== '0');
  function alternarTasks() {
    showTasks = !showTasks;
    localStorage.setItem(CHAVE, showTasks ? '1' : '0');
  }

  const current = $derived(detail ? detail.task - 1 : -1);

  // Seletor de plano. Carrega ao montar, NÃO no foco: o `pinned` vem junto da lista, e sem ele o
  // campo mostraria "automático" mesmo com um plano fixado — a etiqueta mentiria até o usuário
  // clicar. O painel só existe no contexto da sessão aberta, então é uma requisição por abertura.
  let plans = $state<PlanListItem[]>([]);
  let pinned = $state<string | null>(null);
  let pickerErr = $state('');

  // Mesma guarda do planDetail no Chat.svelte: trocar de sessão rápido deixa a requisição anterior
  // no ar, e a resposta atrasada sobrescreveria a lista com os planos da sessão errada. Vale pro
  // `trocar` também — dois cliques seguidos no select chegam fora de ordem.
  let vez = 0;
  const erro = (e: unknown) => (e instanceof Error ? e.message.replace(/^\d+:\s*/, '') : 'falhou');

  $effect(() => {
    const nome = session.name;   // recarrega ao trocar de sessão
    const minha = ++vez;
    (async () => {
      try {
        const r = await getPlans(nome);
        if (minha !== vez) return;   // chegou tarde: já tem requisição mais nova no ar, descarta
        plans = r.plans;
        pinned = r.pinned;
      } catch (e) { if (minha === vez) pickerErr = erro(e); }
    })();
  });

  async function trocar(stem: string) {
    const alvo = stem || null;   // '' = a opção "automático"
    pickerErr = '';
    const minha = ++vez;
    try {
      const r = await setPlanPin(session.name, alvo);
      if (minha === vez) pinned = r.pinned;
    } catch (e) { if (minha === vez) pickerErr = erro(e); }
  }
</script>

<div class="plan">
  <div class="plan-head">
    <!-- select nativo de propósito: funciona igual nas duas views, sem popover nem z-index, e é o
         mesmo padrão do "mensagens recentes" do CommitBox. -->
    <!-- O caret é obrigatório: com `appearance: none` o select fica idêntico ao rótulo de texto que
         havia antes, e ninguém descobre que ali troca de plano. `▾` (abre lista) contrasta com o
         `›` do botão ao lado (expande o markdown) — glifos diferentes pra ações diferentes. -->
    <div class="pick-wrap">
      <select class="plan-pick" value={pinned ?? ''} aria-label="Trocar o plano exibido"
        title="Trocar o plano exibido"
        onchange={(e) => trocar(e.currentTarget.value)}>
        <!-- Sem pin, o nome do plano eleito vai JUNTO: é o rótulo do painel, e some se a opção
             dissesse só "automático". Com pin, quem aparece é a opção do plano fixado. -->
        <option value="">automático{session.plan_name ? ` · ${session.plan_name}` : ''}</option>
        <!-- "nenhum" e uma escolha de verdade, nao ausencia de escolha: sem esta opcao a eleicao
             automatica sempre reacende um plano, e nao havia como deixar o painel/chip apagados. -->
        <option value={PIN_NONE}>nenhum — esconder o plano</option>
        {#each plans as p (p.stem)}
          <option value={p.stem}>{p.name} · {p.done}/{p.total}{p.complete ? ' ✓' : ''}</option>
        {/each}
      </select>
      <span class="caret" aria-hidden="true">▾</span>
    </div>
    <button class="chev-btn" onclick={() => (showMd = !showMd)}
      aria-label={showMd ? 'Esconder o plano inteiro' : 'Ver o plano inteiro'}>
      <span class="chev" class:open={showMd}>›</span>
    </button>
  </div>
  {#if pickerErr}<p class="muted err">{pickerErr}</p>{/if}

  <!-- A barra E o botao de abrir/fechar as Tasks: ela ja resume o progresso, entao e o lugar
       obvio pra tocar. O chevron a direita e o que revela que ali tem clique. -->
  <button class="bar-btn" onclick={alternarTasks} aria-expanded={showTasks}
    aria-label={showTasks ? 'Esconder as tasks' : 'Mostrar as tasks'}
    title={showTasks ? 'Esconder as tasks' : 'Mostrar as tasks'}>
    <span class="bar-wrap"><PlanBar {session} /></span>
    <span class="chev" class:open={showTasks}>›</span>
  </button>

  {#if !showTasks}
    <!-- nada: o usuario fechou -->
  {:else if loading && !detail}
    <p class="muted">carregando o plano…</p>
  {:else if detail}
    <ul class="tasks">
      {#each detail.tasks as t, i}
        <li class="task" class:done={t.total > 0 && t.done >= t.total} class:cur={i === current}>
          <span class="mark">{t.total > 0 && t.done >= t.total ? '✓' : i === current ? '◐' : '○'}</span>
          <span class="ttl">{t.title}</span>
          <span class="cnt">{t.done}/{t.total}</span>
        </li>
        {#if i === current}
          <li class="steps">
            <ul>
              {#each t.steps as s}
                <li class:done={s.done}>
                  <span class="mark">{s.done ? '✓' : '○'}</span>
                  <span class="ttl">{s.title}</span>
                  {#if s.manual}<span class="manual" title="precisa de conferência humana">🙋</span>{/if}
                </li>
              {/each}
            </ul>
          </li>
        {/if}
      {/each}
    </ul>
  {:else if error}
    <p class="muted">não deu pra ler o plano</p>
  {/if}

  <!-- Fora do gate das Tasks: fechar a lista nao pode desarmar o `›` do cabecalho, que e outro
       controle. Markdown NUNCA aparece cru (regra do CLAUDE.md). -->
  {#if showMd && detail}
    <div class="md">{@html renderMarkdown(detail.markdown)}</div>
  {/if}
</div>

<style>
  .plan { display: flex; flex-direction: column; gap: var(--space-1); }

  .plan-head { display: flex; align-items: center; gap: var(--space-1); min-width: 0; }

  .pick-wrap { position: relative; flex: 1; min-width: 0; display: flex; align-items: center; }
  .plan-pick {
    flex: 1;
    min-width: 0;
    /* espaco a direita pro caret, que fica por cima (o select nativo nao aceita ::after) */
    padding: 2px 1.1em 2px var(--space-1);
    margin-left: calc(-1 * var(--space-1));   /* alinha o texto com o resto do painel */
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-secondary);
    font-family: inherit;
    font-size: var(--text-xs);
    font-weight: 600;
    cursor: pointer;
    appearance: none;
    text-overflow: ellipsis;
  }
  /* Vira controle visivel no hover: sem isto ele parece o rotulo estatico que era antes. */
  @media (hover: hover) {
    .plan-pick:hover { border-color: var(--border-default); background: var(--bg-hover); }
  }
  .plan-pick:focus-visible { outline: 1px solid var(--accent); outline-offset: 1px; }
  .caret {
    position: absolute; right: 4px;
    color: var(--text-muted); font-size: 10px; line-height: 1;
    pointer-events: none;   /* o clique tem que chegar no select */
  }
  .chev-btn { flex-shrink: 0; padding: 0; border: 0; background: transparent; cursor: pointer; }

  /* Botao SEM superficie propria: quem carrega o material aqui e o painel (regra da transparencia
     no CLAUDE.md) — um bg-elevated cru viraria retangulo chapado sobre o papel de parede. */
  .bar-btn {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    width: 100%;
    padding: 2px 0;
    border: 0;
    background: transparent;
    cursor: pointer;
    text-align: left;
  }
  .bar-wrap { flex: 1; min-width: 0; }
  .bar-btn .chev { transform: rotate(0deg); }
  .bar-btn .chev.open { transform: rotate(90deg); }
  .err { color: var(--error); }
  .chev {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--text-base);
    line-height: 1;
    transition: transform 160ms var(--ease-out);
  }
  .chev.open { transform: rotate(90deg); }

  .muted { margin: var(--space-2) 0 0; color: var(--text-muted); font-size: var(--text-xs); }

  .tasks { margin: var(--space-2) 0 0; padding: 0; list-style: none; }

  .task {
    display: flex;
    align-items: baseline;
    gap: var(--space-1);
    padding: 3px 0;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .task.cur { color: var(--text-primary); font-weight: 600; }
  .task.done { color: var(--text-secondary); }
  .task .ttl { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .task .cnt { flex-shrink: 0; font-variant-numeric: tabular-nums; color: var(--text-muted); }
  .task .mark, .steps .mark { flex-shrink: 0; width: 1.2em; text-align: center; }
  .task.done .mark { color: var(--success); }
  .task.cur .mark { color: var(--accent); }

  .steps { padding: 0 0 0 var(--space-4); }
  .steps ul { margin: 0; padding: 0; list-style: none; }
  .steps li {
    display: flex;
    align-items: baseline;
    gap: var(--space-1);
    padding: 2px 0;
    color: var(--text-muted);
    font-size: 11px;
  }
  .steps li.done { color: var(--text-secondary); text-decoration: line-through; text-decoration-color: var(--border-default); }
  .steps li.done .mark { color: var(--success); text-decoration: none; }
  .steps .ttl { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .manual { flex-shrink: 0; }

  .md {
    margin-top: var(--space-2);
    max-height: 50vh;
    overflow: auto;
    padding: var(--space-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: var(--surface-card);
    color: var(--text-secondary);
    font-size: var(--text-xs);
    line-height: 1.55;
    word-break: break-word;
  }
  /* Mesma tipografia do contrato do par (PairSheet.contract-body) — é o mesmo caso: markdown cru
     de um arquivo do disco, renderizado num painel lateral estreito. */
  .md :global(h1) { margin: 0 0 var(--space-2); font-size: var(--text-sm); color: var(--text-primary); }
  .md :global(h2) { margin: var(--space-3) 0 var(--space-1); font-size: var(--text-xs); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted); }
  .md :global(h3) { margin: var(--space-2) 0 var(--space-1); font-size: var(--text-xs); color: var(--text-secondary); }
  .md :global(p) { margin: 0 0 var(--space-2); }
  .md :global(strong) { color: var(--text-primary); font-weight: 650; }
  .md :global(ul), .md :global(ol) { margin: 0 0 var(--space-2); padding-left: 1.2em; }
  .md :global(li) { margin: 2px 0; }
  .md :global(code) { padding: 0 4px; border-radius: 3px; background: var(--surface-raised); font-family: var(--font-mono); font-size: 11px; color: var(--text-primary); }
  .md :global(pre) { margin: 0 0 var(--space-2); padding: var(--space-2); overflow-x: auto; border-radius: var(--radius-sm); background: var(--surface-inset); }
  .md :global(a) { color: var(--accent); }
  .md :global(hr) { margin: var(--space-3) 0; border: 0; border-top: 1px solid var(--border-subtle); }
</style>
