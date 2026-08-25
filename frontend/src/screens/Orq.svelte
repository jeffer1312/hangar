<script lang="ts">
  import { untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import { listServers, onServersChanged, type Server } from '../lib/auth';
  import { getOrqForServer, getOrqDetalheForServer } from '../lib/api';
  import type { OrqExecucao, OrqFicha } from '../lib/types';
  import { duracaoLegivel } from '../lib/orq';
  import Spinner from '../components/Spinner.svelte';
  import OrqAgora from '../components/OrqAgora.svelte';

  interface Props {
    onBack?: () => void;                       // só no celular: a tela é rota própria lá
    onNavigateToChat?: (name: string) => void;
  }
  let { onBack, onNavigateToChat }: Props = $props();

  // Uma execução carrega o servidor de onde veio: o detalhe precisa perguntar à MESMA máquina, e a
  // malha pode ter duas execuções de mesmo id em servidores diferentes.
  interface ExecComServidor { exec: OrqExecucao; servidor: Server }

  let servidores = $state<Server[]>(listServers());
  $effect(() => onServersChanged(() => { servidores = listServers(); }));

  let carregando = $state(true);
  let linhas = $state<ExecComServidor[]>([]);
  let fichas = $state<OrqFicha[]>([]);
  let falhas = $state<string[]>([]);           // rótulos das máquinas que não responderam

  let geracao = 0;
  async function carregar(alvo: Server[]) {
    const meu = ++geracao;
    carregando = true;
    const respostas = await Promise.all(alvo.map(async (s) => {
      try { return { servidor: s, dados: await getOrqForServer(s), erro: false }; }
      // Falha de UM servidor vira aviso — nunca tela vazia com a malha inteira funcionando.
      catch { return { servidor: s, dados: null, erro: true }; }
    }));
    if (meu !== geracao) return;
    const juntas: ExecComServidor[] = [];
    const porPar = new Map<string, OrqFicha>();
    const ruins: string[] = [];
    for (const r of respostas) {
      if (!r.dados) { ruins.push(r.servidor.label); continue; }
      for (const exec of r.dados.execucoes) juntas.push({ exec, servidor: r.servidor });
      // Fichas somam entre servidores: o par (provider · modelo) é o mesmo trabalhador em qualquer
      // máquina — é justamente o número que o usuário compara pra escolher quem executa.
      for (const f of r.dados.fichas) {
        const atual = porPar.get(f.par);
        if (!atual) { porPar.set(f.par, { ...f }); continue; }
        const tarefas = atual.aceitas + atual.nao_aceitas + f.aceitas + f.nao_aceitas;
        const mediaPonderada = tarefas
          ? (atual.rodadas_media * (atual.aceitas + atual.nao_aceitas)
            + f.rodadas_media * (f.aceitas + f.nao_aceitas)) / tarefas
          : 0;
        porPar.set(f.par, {
          par: f.par,
          aceitas: atual.aceitas + f.aceitas,
          nao_aceitas: atual.nao_aceitas + f.nao_aceitas,
          aprovadas_primeira: atual.aprovadas_primeira + f.aprovadas_primeira,
          rodadas_media: Math.round(mediaPonderada * 10) / 10,
        });
      }
    }
    // Mais recente primeiro pelo id (`<data>-<gid>`), a mesma ordem que o backend usa por servidor.
    juntas.sort((a, b) => b.exec.id.localeCompare(a.exec.id));
    linhas = juntas;
    fichas = [...porPar.values()].sort((a, b) => (b.aceitas + b.nao_aceitas) - (a.aceitas + a.nao_aceitas));
    falhas = ruins;
    carregando = false;
  }

  // Mesma chave de identidade do Costs: token entra (consertar credencial recarrega), rótulo não.
  const chaveServidores = $derived(servidores.map((s) => `${s.id}|${s.baseUrl}|${s.token}`).join('\n'));
  $effect(() => { chaveServidores; carregar(untrack(() => servidores)); });

  // ── Execução viva ──────────────────────────────────────────────────────────
  // `fim === null` = ninguém escreveu execucao_fim ainda. A faixa precisa dos EVENTOS (rodada,
  // veredito), que só vêm no detalhe — então ela busca o detalhe daquela execução, uma vez.
  const viva = $derived(linhas.find((l) => !l.exec.fim) ?? null);
  let vivaComEventos = $state<ExecComServidor | null>(null);
  $effect(() => {
    const alvo = viva;
    if (!alvo) { vivaComEventos = null; return; }
    let vivo = true;
    (async () => {
      try {
        const completo = await getOrqDetalheForServer(alvo.servidor, alvo.exec.id);
        if (vivo) vivaComEventos = { exec: completo, servidor: alvo.servidor };
      } catch {
        // Sem o detalhe a faixa simplesmente não aparece — a lista abaixo continua inteira.
        if (vivo) vivaComEventos = null;
      }
    })();
    return () => { vivo = false; };
  });

  // ── Detalhe ────────────────────────────────────────────────────────────────
  let aberta = $state<ExecComServidor | null>(null);
  let detalhe = $state<OrqExecucao | null>(null);
  let erroDetalhe = $state('');
  let tasksAbertas = $state<Set<number>>(new Set());

  async function abrir(linha: ExecComServidor) {
    aberta = linha;
    detalhe = null;
    erroDetalhe = '';
    tasksAbertas = new Set();
    try {
      detalhe = await getOrqDetalheForServer(linha.servidor, linha.exec.id);
    } catch (e) {
      erroDetalhe = e instanceof Error ? e.message : String(e);
    }
  }

  function fechar() {
    aberta = null;
    detalhe = null;
  }

  function alternarTask(n: number) {
    const s = new Set(tasksAbertas);
    if (s.has(n)) s.delete(n); else s.add(n);
    tasksAbertas = s;
  }

  // ── Derivados da visão geral ───────────────────────────────────────────────
  const totalTasks = $derived(linhas.reduce((a, l) => a + l.exec.tasks.length, 0));
  const totalPrimeira = $derived(linhas.reduce((a, l) => a + l.exec.aprovadas_primeira, 0));
  const totalVoltas = $derived(linhas.reduce((a, l) => a + l.exec.voltas, 0));
  const tempoSomado = $derived.by(() => {
    const min = linhas.reduce((acc, l) => {
      if (!l.exec.inicio || !l.exec.fim) return acc;
      const ms = new Date(l.exec.fim).getTime() - new Date(l.exec.inicio).getTime();
      return Number.isFinite(ms) && ms > 0 ? acc + ms / 60_000 : acc;
    }, 0);
    const h = Math.floor(Math.round(min) / 60);
    return h ? `${h}h` : `${Math.round(min)}min`;
  });

  const hora = (iso: string | null) => {
    if (!iso) return '';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '';
    return d.toLocaleString(undefined, { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
  };

  const pct = (parte: number, total: number) => (total > 0 ? `${Math.round((parte / total) * 100)}%` : '—');

  // Par (provider · modelo) daquele executor nesta execução — vazio quando a mineração não fechou
  // o modelo com evidência (aí a linha cai no rótulo genérico "executor").
  function parDe(e: OrqExecucao, executor: string): string {
    return e.tasks.find((t) => t.executor === executor && t.par)?.par ?? '';
  }

  function classeVeredito(resultado: string | undefined): string {
    if (resultado === 'aprova') return 'ok';
    if (resultado === 'reprova') return 'err';
    if (resultado === 'devolvido') return 'warn';
    return '';
  }
</script>

<div class="orq">
  <header class="orq-topo">
    {#if onBack}
      <button class="voltar" onclick={aberta ? fechar : onBack} aria-label={m.orq_voltar()}>←</button>
    {:else if aberta}
      <button class="voltar" onclick={fechar} aria-label={m.orq_voltar()}>←</button>
    {/if}
    <h1>{aberta ? aberta.exec.id : m.shell_orq()}</h1>
    {#if !aberta && !carregando}
      <span class="contagem">{linhas.length} {m.orq_execucoes()}</span>
    {/if}
  </header>

  {#if falhas.length}
    <p class="aviso">{m.orq_parcial()} {falhas.join(', ')}</p>
  {/if}

  {#if carregando}
    <div class="centro"><Spinner /></div>
  {:else if aberta}
    <!-- ── DETALHE ─────────────────────────────────────────────────────── -->
    {#if erroDetalhe}
      <p class="aviso">{erroDetalhe}</p>
    {:else if !detalhe}
      <div class="centro"><Spinner /></div>
    {:else}
      {@const d = detalhe}
      <section class="kpis">
        <div class="kpi"><span class="v">{duracaoLegivel(d.inicio, d.fim) || '—'}</span><span class="l">{hora(d.inicio)}</span></div>
        <div class="kpi"><span class="v">{d.tasks.length}</span><span class="l">{m.orq_tasks()}</span></div>
        <div class="kpi"><span class="v">{d.aprovadas_primeira}</span><span class="l">{m.orq_de_primeira()}</span></div>
        <div class="kpi"><span class="v" class:alerta={d.voltas > 0}>{d.voltas}</span><span class="l">{m.orq_voltas()}</span></div>
      </section>

      <div class="colunas">
        <section>
          <h2>{m.orq_linha_do_tempo()}</h2>
          {#each d.tasks as t (t.task)}
            <div class="tl-task" class:aberta={tasksAbertas.has(t.task)}>
              <button class="cab" onclick={() => alternarTask(t.task)} aria-expanded={tasksAbertas.has(t.task)}>
                <span class="n">T{t.task}</span>
                <span class="t">{t.titulo || m.orq_sem_titulo()}</span>
                <span class="mini">{duracaoLegivel(t.inicio, t.fim)}</span>
                <span class="badge" class:b-ok={t.rodadas === 1} class:b-warn={t.rodadas > 1}>
                  {t.rodadas === 0
                    ? m.orq_rodadas_desconhecidas()
                    : `${t.rodadas} ${t.rodadas === 1 ? m.orq_rodada() : m.orq_rodadas()}`}
                </span>
              </button>
              {#if tasksAbertas.has(t.task)}
                <div class="eventos">
                  {#each t.eventos ?? [] as ev, i (ev.ts + i)}
                    <div class="ev {classeVeredito(ev.resultado)}">
                      <span class="hora">{hora(ev.ts)}{ev.ts_aproximado ? ' ~' : ''}</span>
                      <div class="quem">{ev.sessao || ev.executor || ''}{ev.par ? ` · ${ev.par}` : ''}</div>
                      <div class="oq">
                        {#if ev.tipo === 'veredito'}
                          {(ev.resultado ?? '').toUpperCase()}{ev.rodada ? ` — ${m.orq_rodada()} ${ev.rodada}` : ''}
                        {:else if ev.tipo === 'entrega'}
                          {m.orq_entrega()}{ev.rodada ? ` — ${m.orq_rodada()} ${ev.rodada}` : ''}
                        {:else}
                          {m.orq_task_inicio()}
                        {/if}
                      </div>
                      {#if ev.motivo || ev.commit}
                        <div class="det">
                          {#if ev.motivo}<span>{ev.motivo}</span>{/if}
                          {#if ev.commit}<span class="commit">{ev.commit}</span>{/if}
                        </div>
                      {/if}
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {/each}
        </section>

        <section>
          <h2>{m.orq_quem_trabalhou()}</h2>
          {#each [...new Set(d.tasks.map((t) => t.executor).filter(Boolean))] as quem (quem)}
            <button class="card sess" onclick={() => onNavigateToChat?.(quem)}>
              <span class="nome">{quem}</span>
              <span class="papel">{parDe(d, quem) || m.orq_executor()}</span>
            </button>
          {/each}
          {#each (d.eventos_execucao ?? []).filter((e) => e.tipo === 'sessao_trocada') as ev, i (ev.ts + i)}
            <p class="troca">{m.orq_troca()}: {ev.de} → {ev.para}{ev.motivo ? ` (${ev.motivo})` : ''}</p>
          {/each}

          <h2>{m.orq_ficha_execucao()}</h2>
          <div class="card">
            <table class="ficha">
              <thead>
                <tr><th>{m.orq_par()}</th><th>{m.orq_tasks()}</th><th>{m.orq_de_primeira()}</th><th>{m.orq_rodadas_media()}</th></tr>
              </thead>
              <tbody>
                {#each fichasDaExecucao(d) as f (f.par)}
                  <tr>
                    <td class="par">{f.par}</td>
                    <td>{f.aceitas + f.nao_aceitas}</td>
                    <td>{pct(f.aprovadas_primeira, f.aceitas + f.nao_aceitas)}</td>
                    <td>{f.rodadas_media || '—'}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          {#if d.reconstruida}
            <p class="nota">{m.orq_reconstruida()}</p>
          {/if}
        </section>
      </div>
    {/if}
  {:else if linhas.length === 0}
    <div class="vazio">
      <p>{m.orq_vazio()}</p>
      <p class="nota">{m.orq_vazio_dica()}</p>
    </div>
  {:else}
    <!-- ── VISÃO GERAL ─────────────────────────────────────────────────── -->
    {#if vivaComEventos}
      <!-- Numa div DESTE componente: o `max-width` de `.orq > *` é CSS escopado, e Svelte só marca
           elementos do próprio arquivo — a faixa vinha larga demais, fora do bloco. -->
      <div class="faixa-agora">
        <OrqAgora execucao={vivaComEventos.exec} servidor={vivaComEventos.servidor} {onNavigateToChat} />
      </div>
    {/if}

    <section class="kpis">
      <div class="kpi"><span class="v">{linhas.length}</span><span class="l">{m.orq_execucoes()}</span></div>
      <div class="kpi"><span class="v">{tempoSomado}</span><span class="l">{m.orq_tempo_somado()}</span></div>
      <div class="kpi"><span class="v">{totalPrimeira}/{totalTasks}</span><span class="l">{m.orq_de_primeira()}</span></div>
      <div class="kpi"><span class="v" class:alerta={totalVoltas > 0}>{totalVoltas}</span><span class="l">{m.orq_voltas()}</span></div>
    </section>

    {#each linhas as linha (linha.servidor.id + '::' + linha.exec.id)}
      {@const e = linha.exec}
      <button class="exec" onclick={() => abrir(linha)}>
        <span class="hd">
          <span class="nome">{e.id}</span>
          <span class="branch">{e.branch || '—'}</span>
          <span class="estado" class:viva={!e.fim}>
            {e.fim ? (e.resultado === 'abortada' ? m.orq_abortada() : m.orq_concluida()) : m.orq_em_curso()}
          </span>
        </span>
        <span class="linha-metricas">
          <span class="mt">{m.orq_tasks()} <b>{e.tasks.length}</b></span>
          <span class="mt">{m.orq_de_primeira()} <b>{e.aprovadas_primeira}</b></span>
          <span class="mt" class:alerta={e.voltas > 0}>{m.orq_voltas()} <b>{e.voltas}</b></span>
          <span class="mt">{duracaoLegivel(e.inicio, e.fim) || hora(e.inicio)}</span>
          {#if servidores.length > 1}<span class="mt srv">{linha.servidor.label}</span>{/if}
        </span>
      </button>
    {/each}

    {#if fichas.length}
      <h2>{m.orq_fichas()}</h2>
      <div class="card">
        <table class="ficha">
          <thead>
            <tr><th>{m.orq_par()}</th><th>{m.orq_aceitas()}</th><th>{m.orq_nao_aceitas()}</th><th>{m.orq_de_primeira()}</th><th>{m.orq_rodadas_media()}</th></tr>
          </thead>
          <tbody>
            {#each fichas as f (f.par)}
              <tr>
                <td class="par">{f.par}</td>
                <td>{f.aceitas}</td>
                <td>{f.nao_aceitas}</td>
                <td>{pct(f.aprovadas_primeira, f.aceitas + f.nao_aceitas)}</td>
                <td>{f.rodadas_media || '—'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</div>

<script lang="ts" module>
  import type { OrqExecucao as ExecTipo, OrqFicha as FichaTipo } from '../lib/types';

  // Ficha DESTA execução, calculada dos próprios dados: o endpoint da lista agrega a malha inteira,
  // e filtrar aquilo por execução não dá — o par não carrega de qual execução veio. No `module`
  // porque não depende de estado do componente (e assim o teste pode importá-la).
  export function fichasDaExecucao(e: ExecTipo): FichaTipo[] {
    const acc = new Map<string, FichaTipo & { _soma: number; _n: number }>();
    for (const t of e.tasks) {
      if (!t.par) continue;
      const f = acc.get(t.par) ?? { par: t.par, aceitas: 0, nao_aceitas: 0, aprovadas_primeira: 0, rodadas_media: 0, _soma: 0, _n: 0 };
      if (t.resultado === 'aprova') {
        f.aceitas += 1;
        if (t.rodadas === 1) f.aprovadas_primeira += 1;
      } else {
        f.nao_aceitas += 1;
      }
      if (t.rodadas >= 1) { f._soma += t.rodadas; f._n += 1; }
      acc.set(t.par, f);
    }
    return [...acc.values()]
      .map((f) => ({ ...f, rodadas_media: f._n ? Math.round((f._soma / f._n) * 10) / 10 : 0 }))
      .sort((a, b) => (b.aceitas + b.nao_aceitas) - (a.aceitas + a.nao_aceitas));
  }
</script>

<style>
  .orq {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    height: 100%;
    padding: var(--space-4);
    overflow-y: auto;
    container-type: inline-size;   /* quem aperta a linha é a LARGURA DESTE painel, não a da janela */
  }

  /* Teto de largura: numa tela de 1440px a tabela de fichas espalhava 5 colunas por 1300px, e a
     linha do olho até o número virava uma travessia. O bloco fica centrado no painel. */
  .orq > * { width: 100%; max-width: 1100px; margin-inline: auto; }

  .orq-topo { display: flex; align-items: center; gap: var(--space-2); }
  .orq-topo h1 { flex: 1; min-width: 0; font-size: var(--text-lg); font-weight: 620; }
  .contagem { color: var(--text-muted); font-size: var(--text-xs); }
  .voltar {
    width: 34px; height: 34px; min-height: 0; padding: 0;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-lg);
    background: var(--surface-raised); color: var(--text-secondary);
  }

  .centro { display: grid; place-items: center; padding: var(--space-6); }
  .aviso { color: var(--warning); font-size: var(--text-sm); }
  .vazio { padding: var(--space-6); text-align: center; color: var(--text-secondary); }
  .nota { color: var(--text-muted); font-size: var(--text-xs); }

  .kpis { display: grid; grid-template-columns: repeat(4, 1fr); gap: var(--space-2); }
  @container (max-width: 560px) { .kpis { grid-template-columns: repeat(2, 1fr); } }
  .kpi {
    display: flex; flex-direction: column; gap: 2px;
    padding: var(--space-3);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-lg);
    background: var(--surface-card);
  }
  .kpi .v { font-size: var(--text-lg); font-weight: 620; }
  .kpi .v.alerta { color: var(--warning); }
  .kpi .l { color: var(--text-muted); font-size: var(--text-xs); }

  .exec {
    display: flex; flex-direction: column; align-items: flex-start; gap: var(--space-1);
    width: 100%; padding: var(--space-3); text-align: left;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-lg);
    background: var(--surface-card); color: inherit;
  }
  .exec:hover { background: var(--bg-hover); }
  .exec .hd { display: flex; align-items: baseline; gap: var(--space-2); flex-wrap: wrap; width: 100%; }
  .exec .nome { font-weight: 560; }
  .exec .branch, .mt.srv { color: var(--text-muted); font-family: var(--font-mono); font-size: var(--text-xs); }
  .estado {
    margin-left: auto; padding: 2px 8px; border-radius: 999px;
    background: var(--surface-raised); color: var(--text-secondary);
    font-size: var(--text-xs); font-weight: 600;
  }
  .estado.viva { background: var(--accent-dim); color: var(--accent); }
  .linha-metricas { display: flex; flex-wrap: wrap; gap: var(--space-3); color: var(--text-secondary); font-size: var(--text-xs); }
  .mt.alerta { color: var(--warning); }

  h2 { margin-top: var(--space-2); color: var(--text-muted); font-size: var(--text-xs); font-weight: 600; text-transform: uppercase; letter-spacing: 0.07em; }

  /* `minmax(0, …)` e não `1.4fr 1fr` cru: um item de grid não encolhe abaixo do próprio min-content,
     e a linha da task (número + título + duração + selo) empurrava a coluna pra 472px numa tela de
     390 — a tela inteira ganhava barra de rolagem lateral. O `min-width: 0` no filho é a outra
     metade da mesma regra. */
  .colunas { display: grid; grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr); gap: var(--space-4); align-items: start; }
  .colunas > section { min-width: 0; }
  @container (max-width: 720px) { .colunas { grid-template-columns: minmax(0, 1fr); } }

  .tl-task { margin-bottom: var(--space-2); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); background: var(--surface-card); }
  .tl-task .cab { display: flex; align-items: center; gap: var(--space-2); width: 100%; padding: var(--space-2) var(--space-3); text-align: left; background: transparent; color: inherit; }
  .tl-task .n { flex: none; width: 30px; color: var(--text-muted); font-family: var(--font-mono); font-size: var(--text-xs); }
  .tl-task .t { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tl-task .mini { flex: none; color: var(--text-muted); font-size: var(--text-xs); }
  .badge { flex: none; padding: 2px 8px; border-radius: 999px; background: var(--surface-raised); color: var(--text-secondary); font-size: var(--text-xs); font-weight: 600; }
  .badge.b-ok { color: var(--success); }
  .badge.b-warn { color: var(--warning); }

  .eventos { display: flex; flex-direction: column; gap: var(--space-2); padding: 0 var(--space-3) var(--space-3) calc(var(--space-3) + 30px); }
  .ev { padding-left: var(--space-2); border-left: 2px solid var(--border-default); }
  .ev.ok { border-left-color: var(--success); }
  .ev.err { border-left-color: var(--error); }
  .ev.warn { border-left-color: var(--warning); }
  .ev .hora { color: var(--text-muted); font-family: var(--font-mono); font-size: var(--text-xs); }
  .ev .quem { color: var(--text-secondary); font-family: var(--font-mono); font-size: var(--text-xs); }
  .ev .oq { font-size: var(--text-sm); font-weight: 560; }
  .ev .det { color: var(--text-muted); font-size: var(--text-xs); display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .commit { font-family: var(--font-mono); }

  .card { padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); background: var(--surface-card); }
  .card.sess { display: flex; align-items: center; gap: var(--space-2); width: 100%; margin-bottom: var(--space-2); text-align: left; color: inherit; }
  .card.sess:hover { background: var(--bg-hover); }
  .card.sess .nome { flex: 1; min-width: 0; font-family: var(--font-mono); font-size: var(--text-sm); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .papel { color: var(--text-secondary); font-size: var(--text-xs); text-transform: uppercase; letter-spacing: 0.05em; }
  .troca { color: var(--text-muted); font-size: var(--text-xs); margin-bottom: var(--space-1); }

  table.ficha { width: 100%; border-collapse: collapse; font-size: var(--text-xs); }
  table.ficha th { padding: 4px 6px; text-align: right; color: var(--text-muted); font-weight: 500; border-bottom: 1px solid var(--border-subtle); }
  table.ficha td { padding: 5px 6px; text-align: right; color: var(--text-secondary); font-family: var(--font-mono); border-bottom: 1px solid var(--border-subtle); }
  table.ficha th:first-child, table.ficha td:first-child { text-align: left; }
  table.ficha td.par { color: var(--text-primary); font-family: inherit; }
  table.ficha tr:last-child td { border-bottom: 0; }
</style>
