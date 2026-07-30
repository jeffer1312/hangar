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

  const current = $derived(detail ? detail.task - 1 : -1);

  // Seletor de plano. Carrega ao montar, NÃO no foco: o `pinned` vem junto da lista, e sem ele o
  // campo mostraria "automático" mesmo com um plano fixado — a etiqueta mentiria até o usuário
  // clicar. O painel só existe no contexto da sessão aberta, então é uma requisição por abertura.
  let plans = $state<PlanListItem[]>([]);
  let pinned = $state<string | null>(null);
  let pickerErr = $state('');

  $effect(() => {
    const nome = session.name;   // recarrega ao trocar de sessão
    (async () => {
      try { const r = await getPlans(nome); plans = r.plans; pinned = r.pinned; }
      catch (e) { pickerErr = e instanceof Error ? e.message.replace(/^\d+:\s*/, '') : 'falhou'; }
    })();
  });

  async function trocar(stem: string) {
    const alvo = stem || null;   // '' = a opção "automático"
    pickerErr = '';
    try { pinned = (await setPlanPin(session.name, alvo)).pinned; }
    catch (e) { pickerErr = e instanceof Error ? e.message.replace(/^\d+:\s*/, '') : 'falhou'; }
  }
</script>

<div class="plan">
  <div class="plan-head">
    <!-- select nativo de propósito: funciona igual nas duas views, sem popover nem z-index, e é o
         mesmo padrão do "mensagens recentes" do CommitBox. -->
    <select class="plan-pick" value={pinned ?? ''} aria-label="Plano exibido"
      onchange={(e) => trocar(e.currentTarget.value)}>
      <!-- Sem pin, o nome do plano eleito vai JUNTO: é o rótulo do painel, e some se a opção
           dissesse só "automático". Com pin, quem aparece é a opção do plano fixado. -->
      <option value="">automático{session.plan_name ? ` · ${session.plan_name}` : ''}</option>
      {#each plans as p (p.stem)}
        <option value={p.stem}>{p.name} · {p.done}/{p.total}{p.complete ? ' ✓' : ''}</option>
      {/each}
    </select>
    <button class="chev-btn" onclick={() => (showMd = !showMd)}
      aria-label={showMd ? 'Esconder o plano inteiro' : 'Ver o plano inteiro'}>
      <span class="chev" class:open={showMd}>›</span>
    </button>
  </div>
  {#if pickerErr}<p class="muted err">{pickerErr}</p>{/if}

  <PlanBar {session} />

  {#if loading && !detail}
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

    {#if showMd}
      <!-- Markdown NUNCA aparece cru (regra do CLAUDE.md): um <pre> com ** e ## à mostra é bug. -->
      <div class="md">{@html renderMarkdown(detail.markdown)}</div>
    {/if}
  {:else if error}
    <p class="muted">não deu pra ler o plano</p>
  {/if}
</div>

<style>
  .plan { display: flex; flex-direction: column; gap: var(--space-1); }

  .plan-head { display: flex; align-items: center; gap: var(--space-1); min-width: 0; }
  .plan-pick {
    flex: 1;
    min-width: 0;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--text-secondary);
    font-family: inherit;
    font-size: var(--text-xs);
    font-weight: 600;
    cursor: pointer;
    /* Sem seta nativa: o chevron ao lado ja e a affordance, e a seta do SO destoa do painel. */
    appearance: none;
    text-overflow: ellipsis;
  }
  .plan-pick:focus-visible { outline: 1px solid var(--accent); outline-offset: 2px; border-radius: 2px; }
  .chev-btn { flex-shrink: 0; padding: 0; border: 0; background: transparent; cursor: pointer; }
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
    background: var(--bg-surface);
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
  .md :global(code) { padding: 0 4px; border-radius: 3px; background: var(--bg-elevated); font-family: var(--font-mono); font-size: 11px; color: var(--text-primary); }
  .md :global(pre) { margin: 0 0 var(--space-2); padding: var(--space-2); overflow-x: auto; border-radius: var(--radius-sm); background: var(--bg-base); }
  .md :global(a) { color: var(--accent); }
  .md :global(hr) { margin: var(--space-3) 0; border: 0; border-top: 1px solid var(--border-subtle); }
</style>
