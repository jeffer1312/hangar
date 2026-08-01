<script lang="ts">
  import NavBar from '../components/NavBar.svelte';
  import { listServers } from '../lib/auth';
  import { fetchCostsForServer } from '../lib/api';
  import { mergeReports, fillDayGaps, type ServerResult, type MergedReport } from '../lib/costs';
  import { dec, tok, money, money2, type Cur } from '../lib/fmt';
  import type { DimBucket, RateInfo } from '../lib/types';

  interface Props { onBack: () => void; }
  let { onBack }: Props = $props();

  // ── Tipos de token: a quebra que existe DENTRO de todo bucket ───────────────
  // O backend manda `cost_input`/`cost_output`/`cost_cache_write`/`cost_cache_read` em CADA corte,
  // então a forma do gasto (projeto de output ≠ projeto de cache) aparece sem precisar clicar.
  type Tipo = 'input' | 'output' | 'cache_write' | 'cache_read';
  const TIPOS: { id: Tipo; label: string; slot: string }[] = [
    { id: 'input', label: 'input', slot: '--s1' },
    { id: 'output', label: 'output', slot: '--s2' },
    { id: 'cache_write', label: 'cache escrito', slot: '--s3' },
    { id: 'cache_read', label: 'cache lido', slot: '--s4' },
  ];
  const tokensDe = (b: DimBucket, t: Tipo) =>
    t === 'input' ? b.input : t === 'output' ? b.output : t === 'cache_write' ? b.cache_write : b.cache_read;
  const custoDe = (b: DimBucket, t: Tipo) =>
    t === 'input' ? b.cost_input : t === 'output' ? b.cost_output
      : t === 'cache_write' ? b.cost_cache_write : b.cost_cache_read;
  const brutos = (b: DimBucket) => b.input + b.output + b.cache_write + b.cache_read;

  // ── Estado ──────────────────────────────────────────────────────────────────
  type Periodo = '7d' | '30d' | '90d' | 'all';
  type Dim = 'provider' | 'source' | 'project' | 'model';

  const PERIODOS: { id: Periodo; label: string; dias: number }[] = [
    { id: '7d', label: '7 dias', dias: 7 },
    { id: '30d', label: '30 dias', dias: 30 },
    { id: '90d', label: '90 dias', dias: 90 },
    { id: 'all', label: 'tudo', dias: 0 },
  ];
  const NOME_DIM: Record<Dim, string> = {
    provider: 'provedor', source: 'fonte', project: 'projeto', model: 'modelo',
  };

  const vazio = (): DimBucket => ({
    key: 'totals', sessions: 0, input: 0, output: 0, cache_write: 0, cache_read: 0,
    cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0,
  });
  const relatorioVazio = (): MergedReport => ({
    partial: false, mismatched: [],
    report: {
      totals: vazio(), by_day: [], by_provider: [], by_source: [], by_project: [], by_model: [],
      by_kind: [], rates: [], sem_tarifa: [], custo_sem_cache: 0, equivalente_cobrado: 0,
      anterior: null, applied: { period: 'all' }, usd_brl: null,
    },
  });

  let loading = $state(true);
  let merged = $state<MergedReport>(relatorioVazio());
  let period = $state<Periodo>('30d');
  let currency = $state<Cur>(localStorage.getItem('cp_costs_currency') === 'BRL' ? 'BRL' : 'USD');
  // Recorte do cliente. UMA dimensão por vez, e não é preguiça de UI: o servidor manda o total de
  // CADA dimensão (marginais), nunca o cruzamento entre elas — "projeto X E fonte Codex" não é
  // derivável do que chega no fio, então oferecer o cruzamento seria inventar número.
  let sel = $state<{ dim: Dim; key: string } | null>(null);
  let tiposOff = $state<Set<Tipo>>(new Set());
  let ocultos = $state<Set<string>>(new Set(lerOcultos()));
  let larguraGrafico = $state(0);
  let diaSobHover = $state<number | null>(null);

  const OCULTOS_KEY = 'cp_proj_ocultos';
  function lerOcultos(): string[] {
    try {
      const v = JSON.parse(localStorage.getItem(OCULTOS_KEY) || '[]');
      return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : [];
    } catch { return []; }
  }
  function salvarOcultos() {
    localStorage.setItem(OCULTOS_KEY, JSON.stringify([...ocultos]));
  }

  function setCurrency(c: Cur) {
    currency = c;
    localStorage.setItem('cp_costs_currency', c);
  }

  // Só o PERÍODO vai ao servidor — é o único corte que o backend aplica (`?period=`).
  let geracao = 0;
  async function load(p: Periodo) {
    const meu = ++geracao;
    loading = true;
    const servers = listServers();
    const results: ServerResult[] = await Promise.all(
      servers.map(async (s) => {
        try { return { report: await fetchCostsForServer(s, p), label: s.label }; }
        // `label` também no erro: o aviso de parcial precisa dizer o NOME da máquina.
        catch { return { report: null, label: s.label }; }
      }),
    );
    if (meu !== geracao) return; // resposta de um período que o usuário já trocou
    merged = mergeReports(results, p);
    loading = false;
  }
  $effect(() => { const p = period; load(p); });

  // ── Derivados ───────────────────────────────────────────────────────────────
  const report = $derived(merged.report);
  const rate = $derived(report.usd_brl);
  const m = (n: number) => money(n, currency, rate);
  const m2 = (n: number) => money2(n, currency, rate);
  const pct = (n: number, total: number) => (total > 0 ? `${dec((n / total) * 100, 1)}%` : '—');

  const listaDa = (d: Dim): DimBucket[] =>
    d === 'provider' ? report.by_provider : d === 'source' ? report.by_source
      : d === 'project' ? report.by_project : report.by_model;

  // O recorte só vale se a chave ainda existe no período carregado: trocar de 30d pra 7d pode
  // apagar o projeto selecionado, e manter o chip apontando pro vazio mostraria zero como se
  // fosse gasto zero.
  const selAtivo = $derived.by(() => {
    if (!sel) return null;
    return listaDa(sel.dim).some((b) => b.key === sel!.key) ? sel : null;
  });
  const foco = $derived.by(() => {
    const s = selAtivo;
    if (!s) return report.totals;
    return listaDa(s.dim).find((b) => b.key === s.key) ?? report.totals;
  });

  function alternar(dim: Dim, key: string) {
    sel = sel && sel.dim === dim && sel.key === key ? null : { dim, key };
  }
  function limpar() {
    sel = null;
    tiposOff = new Set();
  }

  const diasDoPeriodo = $derived(
    PERIODOS.find((p) => p.id === period)!.dias || report.by_day.length || 1,
  );

  // Delta vs. a janela anterior — só sem recorte: `anterior` é o total da malha, e compará-lo
  // com o custo de UM projeto daria uma queda inventada.
  const delta = $derived.by(() => {
    if (selAtivo || !report.anterior || report.anterior.cost <= 0) return '';
    const d = ((report.totals.cost - report.anterior.cost) / report.anterior.cost) * 100;
    if (Math.abs(d) < 1) return 'igual ao período anterior';
    const rot = PERIODOS.find((p) => p.id === period)!.label;
    // Gastar mais não é "bom" nem "ruim" — é um fato. Sem verde/vermelho, que aqui seria
    // julgamento e não informação.
    return `${d > 0 ? '▲' : '▼'} ${dec(Math.abs(d), 0)}% vs. ${rot} anteriores`;
  });

  // ── Série do gráfico ────────────────────────────────────────────────────────
  // `fillDayGaps` devolve os buckets originais nos dias com registro e um zero magro nos buracos.
  // Recupero o DimBucket completo pelo mapa: dia sem sessão é dia de gasto ZERO, não dia
  // inexistente — sem isso um fim de semana parado some e o eixo mente sobre o ritmo.
  const serie = $derived.by(() => {
    const mapa = new Map(report.by_day.map((b) => [b.key, b]));
    const zeroDia = (key: string): DimBucket => ({ ...vazio(), key });
    return fillDayGaps(report.by_day)
      .map((b) => mapa.get(b.key) ?? zeroDia(b.key))
      .reverse(); // by_day vem desc; o eixo do tempo anda pra frente
  });
  const tiposVisiveis = $derived(TIPOS.filter((t) => !tiposOff.has(t.id)));
  const rotuloDia = (k: string) => `${k.slice(8)}/${k.slice(5, 7)}`;

  // Largura MEDIDA do contêiner, nunca um viewBox esticado: com preserveAspectRatio="none" o
  // texto encolhe junto com a escala e no celular os rótulos do eixo viram borrão de 3px.
  // `bind:clientWidth` já é um ResizeObserver — redesenha sozinho quando a janela muda.
  const grafico = $derived.by(() => {
    const dias = serie;
    const W = Math.max(280, larguraGrafico || 640);
    const H = 220, padB = 22, padT = 8;
    const padL = W < 480 ? 46 : 56;
    const colunas = dias.map((d) => {
      let acc = 0;
      const segs = tiposVisiveis.map((t) => {
        const c = custoDe(d, t.id);
        const seg = { slot: t.slot, label: t.label, base: acc, cost: c };
        acc += c;
        return seg;
      }).filter((s) => s.cost > 0);
      return { dia: d, segs, total: acc };
    });
    const max = Math.max(1, ...colunas.map((c) => c.total));
    const nice = Math.pow(10, Math.floor(Math.log10(max)));
    const passo = max / nice > 5 ? nice * 2 : max / nice > 2 ? nice : nice / 2;
    const topo = Math.ceil(max / passo) * passo;
    const plotH = H - padB - padT, plotW = W - padL;
    const bw = Math.max(2, Math.min(26, plotW / Math.max(1, dias.length) - 2));
    const x = (i: number) => padL + (i + 0.5) * (plotW / Math.max(1, dias.length)) - bw / 2;
    const y = (v: number) => padT + plotH - (v / topo) * plotH;
    const linhas: number[] = [];
    if (passo > 0) for (let g = 0; g <= topo + 1e-9; g += passo) linhas.push(g);
    // rótulos do eixo: quantos CABEM (≈52px cada), não um número fixo
    const cada = Math.ceil(dias.length / Math.max(3, Math.floor(plotW / 52)));
    return {
      W, H, padL, padT, plotH, plotW, bw, topo, linhas, cada, colunas,
      barras: colunas.map((c, i) => ({
        ...c,
        x: x(i),
        segs: c.segs.map((s) => ({
          ...s,
          y: y(s.base + s.cost),
          h: Math.max(1, y(s.base) - y(s.base + s.cost) - 2), // 2px de respiro entre fatias
        })),
      })),
      yDe: y,
      xDe: x,
    };
  });

  // ── Painéis ─────────────────────────────────────────────────────────────────
  const fatias = $derived(
    TIPOS.map((t) => ({ ...t, cost: custoDe(foco, t.id), toks: tokensDe(foco, t.id) })),
  );
  const economia = $derived(report.custo_sem_cache - report.totals.cost);
  const taxaCache = $derived(brutos(report.totals) > 0 ? report.totals.cache_read / brutos(report.totals) : 0);

  const projetosVisiveis = $derived(report.by_project.filter((b) => !ocultos.has(b.key)));
  const projetosOcultos = $derived(report.by_project.filter((b) => ocultos.has(b.key)));
  // O pico escala pelos VISÍVEIS: manter a régua num projeto escondido deixaria todas as barras
  // da tela curtas por causa de algo que ninguém vê.
  const picoProjeto = $derived(Math.max(1, ...projetosVisiveis.map((b) => b.cost)));
  const TETO_PROJETOS = 12;
  const projetosNoTeto = $derived(projetosVisiveis.slice(0, TETO_PROJETOS));
  const projetosRestantes = $derived(projetosVisiveis.slice(TETO_PROJETOS));

  const picoModelo = $derived(Math.max(1, ...report.by_model.map((b) => b.cost)));
  const tarifas = $derived.by(() => {
    const mapa = new Map<string, RateInfo>();
    for (const t of report.rates) if (!mapa.has(t.model)) mapa.set(t.model, t);
    return mapa;
  });

  const vazioNoPeriodo = $derived(!loading && report.totals.sessions === 0);
</script>

<NavBar title="Custos" showBack={true} onBack={onBack} />

<div class="costs">
 <div class="inner">
  <div class="filtros">
    <span class="seg" role="group" aria-label="Período">
      {#each PERIODOS as p}
        <button aria-pressed={period === p.id} onclick={() => (period = p.id)}>{p.label}</button>
      {/each}
    </span>

    <span class="fgroup">
      <span class="flabel" id="lbl-prov">provedor</span>
      <select aria-labelledby="lbl-prov" value={selAtivo?.dim === 'provider' ? selAtivo.key : ''}
        onchange={(e) => { const v = e.currentTarget.value; sel = v ? { dim: 'provider', key: v } : null; }}>
        <option value="">todos ({report.by_provider.length})</option>
        {#each report.by_provider as b}<option value={b.key}>{b.key} — {m(b.cost)}</option>{/each}
      </select>
    </span>

    <span class="fgroup">
      <span class="flabel" id="lbl-fonte">fonte</span>
      <select aria-labelledby="lbl-fonte" value={selAtivo?.dim === 'source' ? selAtivo.key : ''}
        onchange={(e) => { const v = e.currentTarget.value; sel = v ? { dim: 'source', key: v } : null; }}>
        <option value="">todas ({report.by_source.length})</option>
        {#each report.by_source as b}<option value={b.key}>{b.key} — {m(b.cost)}</option>{/each}
      </select>
    </span>

    <span class="fgroup">
      <span class="flabel" id="lbl-proj">projeto</span>
      <select aria-labelledby="lbl-proj" value={selAtivo?.dim === 'project' ? selAtivo.key : ''}
        onchange={(e) => { const v = e.currentTarget.value; sel = v ? { dim: 'project', key: v } : null; }}>
        <option value="">todos ({report.by_project.length})</option>
        {#each report.by_project as b}<option value={b.key}>{b.key} — {m(b.cost)}</option>{/each}
      </select>
    </span>

    <span class="fgroup">
      <span class="flabel" id="lbl-mod">modelo</span>
      <select aria-labelledby="lbl-mod" value={selAtivo?.dim === 'model' ? selAtivo.key : ''}
        onchange={(e) => { const v = e.currentTarget.value; sel = v ? { dim: 'model', key: v } : null; }}>
        <option value="">todos ({report.by_model.length})</option>
        {#each report.by_model as b}<option value={b.key}>{b.key} — {m(b.cost)}</option>{/each}
      </select>
    </span>

    <span class="seg" role="group" aria-label="Moeda">
      <button aria-pressed={currency === 'USD'} onclick={() => setCurrency('USD')}>US$</button>
      <button aria-pressed={currency === 'BRL'} onclick={() => setCurrency('BRL')}
        disabled={!rate} title={rate ? undefined : 'cotação indisponível'}>R$</button>
    </span>

    <button class="clear" onclick={limpar}>limpar filtros</button>
  </div>

  {#if merged.partial}
    <p class="warn">
      ⚠ Total parcial.
      {#if merged.mismatched.length}
        {merged.mismatched.join(', ')} respondeu fora do período pedido e ficou de fora da soma.
      {:else}
        Algum servidor não respondeu.
      {/if}
      <button class="retry" onclick={() => load(period)}>Tentar de novo</button>
    </p>
  {/if}

  {#if selAtivo}
    <p class="recorte">
      Recorte: <b>{NOME_DIM[selAtivo.dim]} {selAtivo.key}</b> — os números do topo e a quebra por
      tipo de token são deste recorte. Os painéis abaixo continuam no período inteiro: o servidor
      manda o total de cada dimensão, não o cruzamento entre elas.
      <button class="retry" onclick={() => (sel = null)}>tirar recorte</button>
    </p>
  {/if}

  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if vazioNoPeriodo}
    <p class="muted">Sem dados neste período.</p>
  {:else}
    <dl class="kpis">
      <div class="kpi">
        <dt>custo no período</dt>
        <dd class="hero">{m(foco.cost)}</dd>
        <div class="foot">{m2(foco.cost)} · {foco.sessions} sessões · {m2(foco.cost / diasDoPeriodo)}/dia</div>
        {#if delta}<div class="foot">{delta}</div>{/if}
      </div>
      <div class="kpi">
        <dt>tokens brutos</dt>
        <dd>{tok(brutos(foco))}</dd>
        <div class="foot">passaram pelo modelo</div>
      </div>
      <div class="kpi">
        <dt>equivalente cobrado</dt>
        <!-- Vem do servidor: depende da tarifa de cada modelo, que o front não tem. Existe só
             pro total do período — dentro de um recorte, traço, nunca o número global. -->
        <dd>{selAtivo ? '—' : tok(report.equivalente_cobrado)}</dd>
        <div class="foot">
          {#if selAtivo}só no total do período
          {:else}{pct(report.equivalente_cobrado, brutos(report.totals))} do bruto — o resto é cache barato{/if}
        </div>
      </div>
      <div class="kpi">
        <dt>economia do cache</dt>
        <dd>{selAtivo ? '—' : m(economia)}</dd>
        <div class="foot">
          {#if selAtivo}só no total do período
          {:else}{m2(economia)} · {report.custo_sem_cache > 0
            ? dec(100 - (report.totals.cost / report.custo_sem_cache) * 100, 0)
            : 0}% abaixo do preço cheio{/if}
        </div>
      </div>
    </dl>

    <div class="card">
      <h2>Gasto por dia</h2>
      <p class="hint">Empilhado por tipo de token. Cada coluna é um dia.</p>
      <div class="legend">
        {#each TIPOS as t}
          <button aria-pressed={!tiposOff.has(t.id)}
            onclick={() => {
              const s = new Set(tiposOff);
              if (s.has(t.id)) s.delete(t.id); else s.add(t.id);
              if (s.size === TIPOS.length) s.clear();
              tiposOff = s;
            }}>
            <span class="swatch" style="background: var({t.slot})"></span>{t.label}
          </button>
        {/each}
      </div>
      <div class="chartbox" bind:clientWidth={larguraGrafico}>
        {#if serie.length}
          <svg viewBox="0 0 {grafico.W} {grafico.H}" width={grafico.W} height={grafico.H}
            role="img" aria-label="Gasto por dia, empilhado por tipo de token">
            {#each grafico.linhas as g}
              <line class="grid-line" x1={grafico.padL} x2={grafico.W} y1={grafico.yDe(g)} y2={grafico.yDe(g)} />
              <text x={grafico.padL - 8} y={grafico.yDe(g) + 3} text-anchor="end">{m(g)}</text>
            {/each}
            {#each grafico.barras as b, i}
              {#each b.segs as s}
                <rect x={b.x} y={s.y} width={grafico.bw} height={s.h} rx="2" fill="var({s.slot})" />
              {/each}
              {#if b.total > 0}
                <!-- role="presentation": a área de hover é ponteiro puro, e o conteúdo dela já é
                     lido no aria-label do gráfico e na legenda — não é um segundo controle. -->
                <rect class="hit" role="presentation" x={b.x} y={grafico.padT}
                  width={grafico.bw} height={grafico.plotH}
                  onpointerenter={() => (diaSobHover = i)} onpointerleave={() => (diaSobHover = null)} />
              {/if}
            {/each}
            {#each grafico.barras as b, i}
              {#if i % grafico.cada === 0}
                <text x={b.x + grafico.bw / 2} y={grafico.H - 6} text-anchor="middle">{rotuloDia(b.dia.key)}</text>
              {/if}
            {/each}
            <line class="axis-line" x1={grafico.padL} x2={grafico.W} y1={grafico.yDe(0)} y2={grafico.yDe(0)} />
          </svg>
        {:else}
          <p class="empty">Sem dados no recorte.</p>
        {/if}
      </div>
      <!-- Linha de detalhe FIXA embaixo do gráfico, no lugar do balão flutuante: funciona no
           toque (o celular não tem hover) e não precisa de matemática de posição na viewport. -->
      <p class="caption">
        {#if diaSobHover !== null && grafico.barras[diaSobHover]}
          {@const b = grafico.barras[diaSobHover]}
          <b>{rotuloDia(b.dia.key)}</b> · {m2(b.total)} · {b.dia.sessions} sessões
          {#each b.segs as s}<span class="cap-seg"><i class="swatch" style="background: var({s.slot})"></i>{s.label} {m2(s.cost)}</span>{/each}
        {:else}
          passe o mouse ou toque numa coluna para o detalhe do dia
        {/if}
      </p>
    </div>

    <div class="cols">
      <div class="card">
        <h2>Para onde vai o dólar</h2>
        <p class="hint">O tipo de token que gerou a conta — não o volume bruto.</p>
        <div class="stack100">
          {#each fatias as f}
            {#if f.cost > 0}
              <i style="background: var({f.slot}); flex: {f.cost}" title="{f.label}: {m2(f.cost)}"></i>
            {/if}
          {/each}
        </div>
        <div class="twrap"><table class="breakdown">
          <thead>
            <tr><th></th><th class="n">tokens</th><th class="n">custo</th><th class="n">% da conta</th></tr>
          </thead>
          <tbody>
            {#each fatias as f}
              <tr>
                <td class="name"><span class="swatch" style="background: var({f.slot})"></span>{f.label}</td>
                <td class="n">{tok(f.toks)}</td>
                <td class="n">{m2(f.cost)}</td>
                <td class="n dim">{pct(f.cost, foco.cost)}</td>
              </tr>
            {/each}
          </tbody>
        </table></div>
      </div>

      <div class="card">
        <h2>O que o cache economizou</h2>
        <p class="hint">
          Os mesmos tokens, se nenhum fosse cache.{#if selAtivo} Sempre o período inteiro — o
          servidor calcula isto no total, não por recorte.{/if}
        </p>
        <div class="cmp">
          <div class="cmprow"><span class="dim">pago de verdade</span><b>{m2(report.totals.cost)}</b></div>
          <div class="cmptrack">
            <i style="width: {report.custo_sem_cache > 0
              ? Math.max(2, (report.totals.cost / report.custo_sem_cache) * 100) : 0}%"></i>
          </div>
          <div class="cmprow"><span class="dim">se nada fosse cache</span><b>{m2(report.custo_sem_cache)}</b></div>
        </div>
        <dl class="kpis compacta">
          <div class="kpi"><dt>economizado</dt><dd class="aqua">{m(economia)}</dd></div>
          <div class="kpi"><dt>do volume é cache lido</dt><dd>{dec(taxaCache * 100, 0)}%</dd></div>
        </dl>
      </div>
    </div>

    <div class="cols">
      <div class="card">
        <h2>Por provedor</h2>
        <p class="hint">Quem cobra a conta. Um provedor atravessa várias fontes.</p>
        <div class="rank">
          {#each report.by_provider as b}
            <div class="row">
              <button aria-pressed={selAtivo?.dim === 'provider' && selAtivo.key === b.key}
                onclick={() => alternar('provider', b.key)}>
                <span class="nm">{b.key}</span><span class="vl">{m2(b.cost)}</span>
                <span class="track" style="width: {Math.max(1.5, (b.cost / Math.max(1, report.by_provider[0]?.cost ?? 1)) * 100)}%">
                  {#each TIPOS as t}{#if custoDe(b, t.id) > 0}<i style="background: var({t.slot}); flex: {custoDe(b, t.id)}"></i>{/if}{/each}
                </span>
              </button>
            </div>
          {:else}
            <p class="empty">Sem dados no período.</p>
          {/each}
        </div>
      </div>

      <div class="card">
        <h2>Por fonte</h2>
        <p class="hint">Qual agente rodou. Uma fonte usa vários provedores.</p>
        <div class="rank">
          {#each report.by_source as b}
            <div class="row">
              <button aria-pressed={selAtivo?.dim === 'source' && selAtivo.key === b.key}
                onclick={() => alternar('source', b.key)}>
                <span class="nm">{b.key}</span><span class="vl">{m2(b.cost)}</span>
                <span class="track" style="width: {Math.max(1.5, (b.cost / Math.max(1, report.by_source[0]?.cost ?? 1)) * 100)}%">
                  {#each TIPOS as t}{#if custoDe(b, t.id) > 0}<i style="background: var({t.slot}); flex: {custoDe(b, t.id)}"></i>{/if}{/each}
                </span>
              </button>
            </div>
          {:else}
            <p class="empty">Sem dados no período.</p>
          {/each}
        </div>
      </div>
    </div>

    <div class="card">
      <h2>Por projeto</h2>
      <p class="hint">A pasta onde a sessão rodou. Clique para recortar; o × tira da lista sem tirar da conta.</p>
      <div class="rank">
        {#each projetosNoTeto as b}
          <div class="row">
            <button aria-pressed={selAtivo?.dim === 'project' && selAtivo.key === b.key}
              title="clique para recortar" onclick={() => alternar('project', b.key)}>
              <span class="nm">{b.key}</span><span class="vl">{m2(b.cost)}</span>
              <span class="track" style="width: {Math.max(1.5, (b.cost / picoProjeto) * 100)}%">
                {#each TIPOS as t}{#if custoDe(b, t.id) > 0}<i style="background: var({t.slot}); flex: {custoDe(b, t.id)}"></i>{/if}{/each}
              </span>
            </button>
            <button class="hidebtn" aria-label="tirar {b.key} da lista"
              title="tirar da lista (o gasto continua contando)"
              onclick={() => { const s = new Set(ocultos); s.add(b.key); ocultos = s; salvarOcultos(); }}>×</button>
          </div>
        {:else}
          <p class="empty">Sem dados no período.</p>
        {/each}
        {#if projetosRestantes.length}
          <p class="note">
            + {projetosRestantes.length} projetos somando
            {m2(projetosRestantes.reduce((t, b) => t + b.cost, 0))}
          </p>
        {/if}
        {#if projetosOcultos.length}
          <div class="hiddenbar">
            <span>
              {projetosOcultos.length} fora da lista
              ({m2(projetosOcultos.reduce((t, b) => t + b.cost, 0))}, ainda somando no total):
            </span>
            {#each projetosOcultos as b}
              <button onclick={() => { const s = new Set(ocultos); s.delete(b.key); ocultos = s; salvarOcultos(); }}>
                {b.key} ✕
              </button>
            {/each}
            <button onclick={() => { ocultos = new Set(); salvarOcultos(); }}>mostrar todos</button>
          </div>
        {/if}
      </div>
    </div>

    <!-- Largura cheia: 8 colunas numéricas em meia tela obrigavam rolagem lateral num monitor de
         1280px, e as duas colunas que importam (% e custo) eram as cortadas. -->
    <div class="card">
      <h2>Por modelo</h2>
      <p class="hint">Com a tarifa aplicada e de onde ela veio. Clique para recortar.</p>
      <div class="twrap">
        <table class="data">
          <thead>
            <tr>
              <th>modelo</th><th class="n">in</th><th class="n">out</th><th class="n">cache R</th>
              <th class="n">tarifa in/out</th><th class="n">origem</th><th class="n">% conta</th>
              <th class="n">custo</th><th></th>
            </tr>
          </thead>
          <tbody>
            {#each report.by_model as b}
              {@const t = tarifas.get(b.key)}
              <tr class="click" aria-selected={selAtivo?.dim === 'model' && selAtivo.key === b.key}
                onclick={() => alternar('model', b.key)}>
                <td>{b.key}{#if !t}<span class="tag">sem tarifa</span>{/if}</td>
                <td class="n">{tok(b.input)}</td>
                <td class="n">{tok(b.output)}</td>
                <td class="n">{tok(b.cache_read)}</td>
                <td class="n dim">{t ? `${dec(t.input, 2)}/${dec(t.output, 2)}` : '—'}</td>
                <td class="n dim">{t ? t.origin : '—'}{#if t?.cache_estimado}<span class="tag">cache estimado</span>{/if}</td>
                <td class="n dim">{t ? pct(b.cost, report.totals.cost) : '—'}</td>
                <!-- Sem tarifa mostra TRAÇO, nunca US$ 0,00: zero afirma "não custou nada", que é
                     uma mentira diferente de "não sei o preço". -->
                <td class="c">{#if t}{m2(b.cost)}{:else}<span class="tracinho">—</span>{/if}</td>
                <td class="bar"><span class="mini"><i style="width: {Math.max(2, (b.cost / picoModelo) * 100)}%"></i></span></td>
              </tr>
            {:else}
              <tr><td colspan="9" class="empty">Sem dados no período.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>

    <div class="card">
      <p class="note">
        <b>Fontes:</b> Claude Code (<code>~/.claude/metrics/costs.jsonl</code>) ·
        Codex (<code>~/.codex/sessions</code>) · Pi (<code>~/.pi/agent/sessions</code>).<br />
        <b>Tarifas</b> do models.dev, aplicadas ao histórico inteiro — não há preço histórico,
        então gasto antigo é recalculado com o preço de hoje.<br />
        {#if currency === 'BRL' && rate}<b>Cotação</b> US$ 1 = R$ {dec(rate, 2)}.<br />{/if}
        Custo de tabela da API, <b>não é fatura</b>: plano de assinatura não cobra por token.
        {#if report.sem_tarifa.length}
          <br />{report.sem_tarifa.length} modelo(s) sem tarifa conhecida
          ({report.sem_tarifa.join(', ')}) — aparecem com traço, nunca estimados.
        {/if}
      </p>
    </div>
  {/if}
 </div>
</div>

<style>
  /* Paleta dos slots. Slot 1 é a cor de destaque do app; 2–4 passaram no validador de daltonismo
     nos dois modos. Escuro é o padrão (igual ao app.css), claro sobrescreve. */
  .costs {
    --s1: var(--accent);
    --s2: #d95926;
    --s3: #199e70;
    --s4: #c98500;
    --grid-line: var(--border-subtle);
    --axis-line: var(--border-default);
  }
  :global(html[data-theme='light']) .costs {
    --s2: #eb6834;
    --s3: #1baf7a;
    --s4: #eda100;
  }

  /* flex+min-height+overflow: MESMO idioma do Archive. Sem isto a tela não rola — o #app é
     `overflow: hidden`. padding-top = --navbar-fade porque o ::before da navbar pinta esse tanto
     ABAIXO dela, por cima do conteúdo. */
  .costs {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
    padding: var(--navbar-fade) var(--space-4) var(--space-10);
  }
  .inner { max-width: 1120px; margin-inline: auto; }
  .muted { color: var(--text-secondary); }
  .dim { color: var(--text-secondary); }

  /* ── barra de filtro: uma linha, vale pra tudo abaixo ── */
  /* `--chrome-bg`, não `--surface-card`: a barra é STICKY, e o que passa por baixo dela é o
     conteúdo rolando, não o papel de parede. Com o material de card (mais translúcido) o ranking
     de provedores atravessava a barra e os dois textos viravam um borrão. --chrome-bg é o mesmo
     vidro da NavBar logo acima, e acompanha o slider de Solidez igual. */
  .filtros {
    display: flex; flex-wrap: wrap; gap: var(--space-2) var(--space-3); align-items: center;
    background: var(--chrome-bg); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); padding: var(--space-2) var(--space-3);
    margin-bottom: var(--space-4);
  }
  /* Sticky SÓ no desktop. A 390px a barra ocupa 5 linhas, e vidro translúcido (o app tem papel de
     parede e slider de Solidez) deixava o ranking rolar POR BAIXO dela: dois textos sobrepostos.
     Um terço da tela grudado no topo também não é troca justa num celular. */
  @media (min-width: 820px) {
    .filtros { position: sticky; top: 0; z-index: 5; }
  }
  .seg {
    display: inline-flex; border: 1px solid var(--border-default);
    border-radius: var(--radius-sm); overflow: hidden;
  }
  .seg button {
    background: transparent; border: 0; border-right: 1px solid var(--border-default);
    color: var(--text-secondary); font: inherit; font-size: var(--text-xs);
    padding: 6px 12px; cursor: pointer; min-height: 34px;
  }
  .seg button:last-child { border-right: 0; }
  .seg button[aria-pressed='true'] { background: var(--accent); color: #fff; }
  .seg button:hover:not([aria-pressed='true']) { background: var(--bg-hover); }
  .seg button:disabled { opacity: 0.5; cursor: default; }
  select {
    background: var(--surface-inset); color: var(--text-primary); font: inherit;
    font-size: var(--text-xs); border: 1px solid var(--border-default);
    border-radius: var(--radius-sm); padding: 6px 10px; min-height: 34px; max-width: 190px;
  }
  /* rótulo e controle andam juntos: soltos, a quebra de linha deixava "modelo" no fim de uma
     linha e o seletor dele no começo da outra. */
  .fgroup { display: inline-flex; align-items: center; gap: var(--space-2); min-width: 0; }
  .flabel { font-size: var(--text-xs); color: var(--text-muted); }
  .clear {
    margin-left: auto; background: transparent; border: 1px solid var(--border-default);
    color: var(--text-secondary); border-radius: var(--radius-sm); padding: 6px 10px;
    font: inherit; font-size: var(--text-xs); cursor: pointer; min-height: 34px;
  }
  .clear:hover { background: var(--bg-hover); }

  .warn { color: var(--warning); font-size: var(--text-sm); margin-bottom: var(--space-3); }
  .recorte {
    font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.6;
    background: var(--surface-raised); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm); padding: var(--space-2) var(--space-3);
    margin-bottom: var(--space-3);
  }
  .recorte b { color: var(--text-primary); }
  .retry {
    background: none; border: 1px solid var(--border-default); color: var(--text-secondary);
    border-radius: var(--radius-sm); padding: 2px var(--space-2);
    font: inherit; font-size: var(--text-xs); margin-left: var(--space-2); cursor: pointer;
  }
  .retry:hover { background: var(--bg-hover); }

  /* ── KPIs ── */
  .kpis {
    display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
    gap: var(--space-3); margin-bottom: var(--space-4);
  }
  .kpis > * { min-width: 0; }
  .kpi {
    background: var(--surface-card); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); padding: var(--space-4);
  }
  .kpi dt { font-size: var(--text-xs); color: var(--text-muted); margin-bottom: 6px; }
  .kpi dd { font-size: 30px; font-weight: 650; letter-spacing: -0.02em; line-height: 1.1; }
  .kpi dd.hero { color: var(--accent); font-size: 38px; }
  .kpi dd.aqua { color: var(--s3); }
  .kpi .foot { font-size: var(--text-xs); color: var(--text-secondary); margin-top: 6px; }
  /* `compacta`, não `mini`: a barrinha da tabela por modelo já é `.mini` no mesmo escopo, e a
     colisão trocava o `display: grid` destes cards por `inline-block` — os dois viravam uma
     coluna de 54px de largura. */
  .kpis.compacta { margin: 0; grid-template-columns: 1fr 1fr; }
  .kpis.compacta .kpi { padding: var(--space-3); background: var(--surface-raised); }
  .kpis.compacta dd { font-size: var(--text-xl); }

  /* ── cartões ── */
  .card {
    background: var(--surface-card); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); padding: var(--space-4); margin-bottom: var(--space-4);
  }
  .card > h2 { font-size: var(--text-sm); margin-bottom: 2px; font-weight: 650; }
  .card > .hint { font-size: var(--text-xs); color: var(--text-muted); margin-bottom: var(--space-3); }
  /* min-width:0 nos filhos: filho de grid nasce com min-width:auto e se RECUSA a encolher abaixo
     do conteúdo. Um nome de projeto comprido empurrava o cartão inteiro pra fora e a página ganhava
     rolagem horizontal no celular (765px numa tela de 375). */
  .cols { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-4); }
  .cols > * { min-width: 0; }
  @media (max-width: 820px) { .cols { grid-template-columns: 1fr; } }

  /* ── legenda: identidade nunca só por cor ── */
  .legend { display: flex; flex-wrap: wrap; gap: var(--space-2) var(--space-4); margin-bottom: var(--space-3); }
  .legend button {
    display: inline-flex; align-items: center; gap: 6px; background: none; border: 0;
    padding: 2px 0; color: var(--text-secondary); font: inherit; font-size: var(--text-xs);
    cursor: pointer;
  }
  .legend button[aria-pressed='false'] { opacity: 0.4; }
  .swatch { width: 10px; height: 10px; border-radius: 3px; flex: none; display: inline-block; }

  .chartbox { width: 100%; }
  svg { display: block; max-width: 100%; overflow: visible; }
  .grid-line { stroke: var(--grid-line); stroke-width: 1; }
  .axis-line { stroke: var(--axis-line); stroke-width: 1; }
  text { fill: var(--text-muted); font-size: 10px; font-family: inherit; }
  .hit { fill: transparent; cursor: crosshair; }
  .caption {
    font-size: var(--text-xs); color: var(--text-secondary); margin-top: var(--space-2);
    display: flex; flex-wrap: wrap; gap: var(--space-1) var(--space-3); align-items: center;
    min-height: 2.4em;
  }
  .caption b { color: var(--text-primary); }
  .cap-seg { display: inline-flex; align-items: center; gap: 5px; }

  /* ── barra 100% + quebra ── */
  .stack100 {
    display: flex; height: 34px; border-radius: 6px; overflow: hidden; gap: 2px;
    margin-bottom: var(--space-3);
  }
  .stack100 > i { display: block; }
  .breakdown { width: 100%; border-collapse: collapse; font-size: var(--text-sm); }
  .breakdown th {
    font-size: var(--text-xs); color: var(--text-muted); font-weight: 600;
    text-align: left; padding-bottom: var(--space-1);
  }
  .breakdown td { padding: 5px 0; border-bottom: 1px solid var(--border-subtle); }
  .breakdown tr:last-child td { border-bottom: 0; }
  /* padding-left + nowrap: a 390px as três colunas se encostavam ("19,64 Bi" grudado em
     "US$ 14.257,98", dois números virando um) e, com respiro, o valor quebrava no meio. A tabela
     mora num .twrap, então quando não couber ela rola dentro do cartão — não empurra o corpo. */
  .breakdown .n {
    text-align: right; font-variant-numeric: tabular-nums;
    padding-left: var(--space-3); white-space: nowrap;
  }
  .breakdown td.name { display: flex; align-items: center; gap: 7px; }

  .cmp { display: flex; flex-direction: column; gap: 6px; margin-bottom: var(--space-4); }
  .cmprow { display: flex; justify-content: space-between; gap: var(--space-4); font-size: var(--text-sm); }
  .cmprow b { font-variant-numeric: tabular-nums; }
  .cmptrack { height: 12px; border-radius: 6px; background: var(--surface-inset); overflow: hidden; }
  .cmptrack > i { display: block; height: 100%; background: var(--s1); }

  /* ── ranking ── */
  .rank { display: flex; flex-direction: column; gap: 7px; }
  .rank .row { display: flex; align-items: center; gap: var(--space-1); }
  .rank .row > button:first-child { flex: 1; min-width: 0; }
  .rank button:not(.hidebtn) {
    display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 2px var(--space-3);
    align-items: baseline; background: none; border: 0; padding: 3px 6px;
    border-radius: 6px; color: inherit; font: inherit; text-align: left; cursor: pointer; width: 100%;
  }
  .rank button:not(.hidebtn) > * { min-width: 0; }
  .rank button:not(.hidebtn):hover { background: var(--bg-hover); }
  .rank button[aria-pressed='true'] { background: var(--accent-dim); }
  .rank .nm { font-size: var(--text-sm); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .rank .vl { font-size: var(--text-sm); font-weight: 600; font-variant-numeric: tabular-nums; }
  .rank .track {
    grid-column: 1 / -1; height: 8px; border-radius: 4px; background: var(--surface-inset);
    display: flex; gap: 2px; overflow: hidden;
  }
  .rank .track > i { display: block; }
  .hidebtn {
    flex: none; background: none; border: 0; color: var(--text-muted); cursor: pointer;
    font-size: 15px; line-height: 1; padding: 6px; border-radius: 6px; opacity: 0;
  }
  .rank .row:hover .hidebtn, .hidebtn:focus-visible { opacity: 1; }
  .hidebtn:hover { background: var(--bg-hover); color: var(--text-primary); }
  @media (hover: none) { .hidebtn { opacity: 0.6; } }
  .hiddenbar {
    display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2);
    margin-top: var(--space-2); padding-top: var(--space-2); border-top: 1px solid var(--border-subtle);
    font-size: var(--text-xs); color: var(--text-muted);
  }
  .hiddenbar button {
    background: none; border: 1px solid var(--border-default); color: var(--text-secondary);
    border-radius: var(--radius-full); padding: 3px 9px; font: inherit; font-size: var(--text-xs);
    cursor: pointer;
  }
  .hiddenbar button:hover { background: var(--bg-hover); }

  /* ── tabela por modelo ── */
  .twrap { overflow-x: auto; -webkit-overflow-scrolling: touch; }
  table.data { width: 100%; border-collapse: collapse; font-size: var(--text-sm); }
  table.data th {
    text-align: left; font-size: var(--text-xs); text-transform: uppercase; letter-spacing: 0.03em;
    color: var(--text-muted); font-weight: 600; padding: 0 var(--space-2) var(--space-2);
    white-space: nowrap;
  }
  table.data td { padding: 7px var(--space-2); border-top: 1px solid var(--border-subtle); white-space: nowrap; }
  table.data th.n, table.data td.n { text-align: right; font-variant-numeric: tabular-nums; }
  table.data td.c { font-weight: 650; color: var(--accent); text-align: right; font-variant-numeric: tabular-nums; }
  table.data td.c .tracinho { color: var(--text-muted); font-weight: 400; }
  table.data tr.click { cursor: pointer; }
  table.data tr.click:hover td { background: var(--bg-hover); }
  table.data tr[aria-selected='true'] td { background: var(--accent-dim); }
  .tag {
    display: inline-block; font-size: 10px; padding: 1px 6px; border-radius: var(--radius-full);
    border: 1px solid var(--border-default); color: var(--text-muted); margin-left: 6px;
    vertical-align: 1px;
  }
  .mini {
    display: inline-block; width: 54px; height: 6px; border-radius: 3px;
    background: var(--surface-inset); overflow: hidden; vertical-align: middle;
  }
  .mini > i { display: block; height: 100%; background: var(--accent); }
  .bar { width: 66px; }

  .note { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.6; }
  .note b { color: var(--text-secondary); font-weight: 600; }
  .empty { color: var(--text-muted); font-size: var(--text-sm); padding: var(--space-5) 0; text-align: center; }
</style>
