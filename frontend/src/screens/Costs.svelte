<script lang="ts">
  import NavBar from '../components/NavBar.svelte';
  import Select from '../components/Select.svelte';
  import { listServers, onServersChanged, type Server } from '../lib/auth';
  import { fetchCostsForServer } from '../lib/api';
  import {
    mergeReports, fillDayGaps, tarifasPorModelo, custoDesconhecido, precoParcial, partirOcultos,
    custoSemCacheDe, equivalenteDe, isFree,
    type ServerResult, type MergedReport,
  } from '../lib/costs';
  import { agruparPor, aplicar, filtrar, somar, type Filtro } from '../lib/cubo';
  import { dec, tok, money, money2, type Cur } from '../lib/fmt';
  import { projectLabel } from '../lib/format';
  import type { ComboLocal, DimBucket } from '../lib/types';

  interface Props { onBack: () => void; }
  let { onBack }: Props = $props();

  // ── Tipos de token: a quebra que existe DENTRO de todo bucket ───────────────
  // O backend manda `cost_input`/`cost_output`/`cost_cache_write`/`cost_cache_read` em CADA corte,
  // então a forma do gasto (projeto de output ≠ projeto de cache) aparece sem precisar clicar.
  type Tipo = 'input' | 'output' | 'cache_write' | 'cache_read';
  // `--chart-N` vem do app.css (bloco escuro + bloco claro), não daqui: cor de tema tem que
  // trocar junto com o tema, e hex dentro do componente fica de fora dessa troca.
  const TIPOS: { id: Tipo; label: string; slot: string }[] = [
    { id: 'input', label: 'input', slot: '--chart-1' },
    { id: 'output', label: 'output', slot: '--chart-2' },
    { id: 'cache_write', label: 'cache escrito', slot: '--chart-3' },
    { id: 'cache_read', label: 'cache lido', slot: '--chart-4' },
  ];
  const tokensDe = (b: DimBucket, t: Tipo) =>
    t === 'input' ? b.input : t === 'output' ? b.output : t === 'cache_write' ? b.cache_write : b.cache_read;
  const custoDe = (b: DimBucket, t: Tipo) =>
    t === 'input' ? b.cost_input : t === 'output' ? b.cost_output
      : t === 'cache_write' ? b.cost_cache_write : b.cost_cache_read;
  const brutos = (b: DimBucket) => b.input + b.output + b.cache_write + b.cache_read;

  // ── Estado ──────────────────────────────────────────────────────────────────
  type Periodo = '7d' | '30d' | '90d' | 'all';
  type Dim = 'provider' | 'source' | 'project' | 'model' | 'servidor';

  const PERIODOS: { id: Periodo; label: string; dias: number }[] = [
    { id: '7d', label: '7 dias', dias: 7 },
    { id: '30d', label: '30 dias', dias: 30 },
    { id: '90d', label: '90 dias', dias: 90 },
    { id: 'all', label: 'tudo', dias: 0 },
  ];
  const DIMS: Dim[] = ['provider', 'source', 'project', 'model', 'servidor'];
  const NOME_DIM: Record<Dim, string> = {
    provider: 'provedor', source: 'fonte', project: 'projeto', model: 'modelo',
    servidor: 'máquina',
  };

  // ── Empilhamento do gráfico por FONTE ───────────────────────────────────────
  // Cor por fonte, não por posição na lista: com a paleta seguindo a ordem do ranking, um dia em
  // que o Codex passasse o Claude trocaria as duas cores no meio da série.
  const SLOT_FONTE: Record<string, string> = {
    claude: '--chart-1', codex: '--chart-2', pi: '--chart-3',
  };
  // ponytail: fonte fora das três que o backend produz cai no slot 4; duas delas colidiriam na
  // cor — quando existir uma quarta fonte, o lugar de nomeá-la é este mapa.
  const corDaFonte = (k: string) => SLOT_FONTE[k] ?? '--chart-4';

  const vazio = (): DimBucket => ({
    key: 'totals', sessions: 0, input: 0, output: 0, cache_write: 0, cache_read: 0,
    cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0,
  });
  const relatorioVazio = (): MergedReport => ({
    partial: false, mismatched: [], failed: [],
    report: {
      totals: vazio(), by_day: [], by_provider: [], by_source: [], by_project: [], by_model: [],
      by_servidor: [], by_kind: [], rates: [], sem_tarifa: [], custo_sem_cache: 0,
      equivalente_cobrado: 0, anterior: null, combos: [], applied: { period: 'all' }, usd_brl: null,
    },
  });

  let loading = $state(true);
  let merged = $state<MergedReport>(relatorioVazio());
  let period = $state<Periodo>('30d');
  let currency = $state<Cur>(localStorage.getItem('cp_costs_currency') === 'BRL' ? 'BRL' : 'USD');
  // Recorte do cliente, agora CRUZADO: provedor, fonte, projeto, modelo e subagente valem ao mesmo
  // tempo. Antes era uma dimensão por vez porque o servidor mandava só o total de CADA dimensão
  // (marginais) e "projeto X E fonte Codex" não era derivável do fio; agora ele manda o
  // detalhamento (`combos`), e todo recorte vira uma soma.
  let filtro = $state<Filtro>({});
  // Camadas desligadas na legenda do gráfico: fonte quando há detalhamento, tipo de token quando
  // não há. Set de string porque o que a legenda lista muda com o modo.
  let camadasOff = $state<Set<string>>(new Set());
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

  // Guarda os DESMARCADOS, não os marcados: máquina nova cadastrada depois entra por padrão.
  // Mesma regra do cp_proj_ocultos. Escolha por navegador, de propósito — a lista de servidores
  // já é do cliente (cp_servers), o backend não sabe que ela existe.
  const SERVERS_OFF_KEY = 'cp_costs_servers_off';
  function lerServidoresOff(): string[] {
    try {
      const v = JSON.parse(localStorage.getItem(SERVERS_OFF_KEY) || '[]');
      return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : [];
    } catch { return []; }
  }

  // `listServers()` lê localStorage e NÃO é reativo: a hidratação do vault chega depois do mount
  // (App.svelte mantém um contador só por causa disso, e `setServers` nem dispara o aviso). Um
  // snapshot no init deixaria os chips com a lista velha. Assinar é uma linha.
  let servidores = $state<Server[]>(listServers());
  $effect(() => onServersChanged(() => { servidores = listServers(); }));

  let servidoresOff = $state<Set<string>>(new Set(lerServidoresOff()));
  const marcados = $derived(servidores.filter((s) => !servidoresOff.has(s.id)));
  // Desmarcar tudo não é estado válido: um relatório vazio seria indistinguível de "sem dados no
  // período". Cai em todos.
  const servidoresAtivos = $derived(marcados.length ? marcados : servidores);
  let mostrarServidores = $state(false);

  function alternarServidor(id: string) {
    const s = new Set(servidoresOff);
    if (s.has(id)) s.delete(id); else s.add(id);
    servidoresOff = s;
    localStorage.setItem(SERVERS_OFF_KEY, JSON.stringify([...s]));
  }
  function todosServidores() {
    servidoresOff = new Set();
    localStorage.setItem(SERVERS_OFF_KEY, '[]');
  }

  function setCurrency(c: Cur) {
    currency = c;
    localStorage.setItem('cp_costs_currency', c);
  }

  // Só o PERÍODO vai ao servidor — é o único corte que o backend aplica (`?period=`).
  let geracao = 0;
  async function load(p: Periodo, alvo: Server[]) {
    const meu = ++geracao;
    loading = true;
    const results: ServerResult[] = await Promise.all(
      alvo.map(async (s) => {
        try { return { report: await fetchCostsForServer(s, p), label: s.label, id: s.id }; }
        // `label` também no erro: o aviso de parcial precisa dizer o NOME da máquina.
        catch { return { report: null, label: s.label, id: s.id }; }
      }),
    );
    if (meu !== geracao) return; // resposta de um período que o usuário já trocou
    merged = mergeReports(results, p);
    loading = false;
  }
  // Desmarcar uma máquina recarrega: ela deixa de ser CHAMADA (não paga o timeout de 4s do
  // lib/api.ts:176) e some do aviso de parcial, que hoje lista 6 máquinas offline como se fosse
  // notícia. O `geracao` continua correto com duas dependências — é contador monotônico.
  $effect(() => { const p = period; const alvo = servidoresAtivos; load(p, alvo); });

  // ── Derivados ───────────────────────────────────────────────────────────────
  const report = $derived(merged.report);
  const rate = $derived(report.usd_brl);
  const m = (n: number) => money(n, currency, rate);
  const m2 = (n: number) => money2(n, currency, rate);
  const pct = (n: number, total: number) => (total > 0 ? `${dec((n / total) * 100, 1)}%` : '—');

  // Detalhamento cruzado. Vazio = servidor antigo da malha (ou período sem dado): a tela cai no
  // recorte de UMA dimensão a partir dos `by_*`, que é o que ela fazia antes desta fase.
  const base = $derived<ComboLocal[]>(report.combos ?? []);
  const temCombos = $derived(base.length > 0);

  // `camadasOff` guarda ids de DOIS vocabulários diferentes: fonte (claude/codex/pi) com
  // detalhamento, tipo de token (input/output/…) sem. Se o modo virar no meio da sessão (troca
  // de período que muda `temCombos`), ids do vocabulário velho ficavam no Set e a heurística de
  // reset em "clique" (`s.size === camadas.length`) podia nunca disparar — uma camada sumia do
  // gráfico até o usuário clicar em "limpar filtros". Zera aqui, na troca de modo em si.
  let modoAnterior: boolean | null = null;
  $effect(() => {
    if (modoAnterior !== null && modoAnterior !== temCombos) camadasOff = new Set();
    modoAnterior = temCombos;
  });

  const listaCrua = (d: Dim): DimBucket[] =>
    d === 'provider' ? report.by_provider : d === 'source' ? report.by_source
      : d === 'project' ? report.by_project : d === 'model' ? report.by_model
        : report.by_servidor;

  // O filtro só vale se a chave ainda existe no período carregado: trocar de 30d pra 7d pode
  // apagar o projeto selecionado, e manter o chip apontando pro vazio mostraria zero como se
  // fosse gasto zero. Checa em `base` (todos os combos), SEM cruzar com os outros filtros: cruzar
  // com o `filtro` cru deixava um valor velho de período/subagente envenenar o pick novo em
  // OUTRA dimensão (revisão 06/08 — trocar de período e escolher um provedor rejeitava o pick
  // porque ele era cruzado contra o projeto morto que ainda morava no filtro cru).
  const existe = (d: Dim, v: string) =>
    temCombos ? base.some((c) => c[d] === v) : listaCrua(d).some((b) => b.key === v);
  const manter = (d: Dim) => {
    const v = filtro[d];
    return v && existe(d, v) ? v : undefined;
  };
  const filtroAtivo = $derived.by<Filtro>(() => ({
    provider: manter('provider'), source: manter('source'),
    project: manter('project'), model: manter('model'),
    servidor: manter('servidor'),
    // Separar subagente exige o detalhamento: os `by_*` já vêm somados com ele dentro.
    subagente: temCombos ? filtro.subagente : undefined,
  }));
  const temFiltro = $derived(Object.values(filtroAtivo).some((v) => v !== undefined));
  const recorte = $derived(filtrar(base, filtroAtivo));

  // O que se LÊ de um corte. A chave da conta Anthropic é 'anthropic:<uuid>' — identidade que não
  // colide entre servidores da malha, mas ilegível na tela: a linha de topo do "Por provedor",
  // com 87% do gasto, aparecia como o uuid cru. O backend manda o e-mail no `label` dos `by_*`;
  // o detalhamento manda só a chave, então o rótulo se busca neste mapa.
  const rotulos = $derived(new Map(
    report.by_provider.filter((b) => b.label).map((b) => [b.key, b.label as string])));
  const rot = (b: DimBucket) => b.label || rotulos.get(b.key) || b.key;
  // Nome legível de uma chave no rótulo do recorte: provedor usa o e-mail (rotulos), projeto o
  // basename — o caminho cru de 80 chars no texto do recorte não é "nome amigável" de nada.
  // O rótulo vem do próprio by_servidor (que o mergeReports preencheu com o Server.label); o
  // fallback pega o caso da máquina que não respondeu e por isso não virou bucket.
  const nomeServidor = (id: string) =>
    report.by_servidor.find((b) => b.key === id)?.label
    ?? servidores.find((s) => s.id === id)?.label ?? id;

  const nomeDa = (d: Dim, key: string) =>
    d === 'provider' ? (rotulos.get(key) ?? key)
      : d === 'project' ? projectLabel(key)
        : d === 'servidor' ? nomeServidor(key) : key;

  // A lista de UMA dimensão é o RECORTE COMPLETO, incluindo o filtro da própria dimensão:
  // com o modelo X selecionado, a tabela "Por modelo" mostra só o X — decidido 2026-08-06, o
  // desenho antigo (nunca o próprio filtro) mostrava todos os modelos com custos do período
  // inteiro, e a tela parecia ignorar o filtro. Quem usa é o PAINEL da dimensão.
  const listaDa = (d: Dim): DimBucket[] =>
    temCombos ? agruparPor(filtrar(base, filtroAtivo), d) : listaCrua(d);

  // O que cada SELETOR lista: cruzado com os OUTROS filtros, nunca com o próprio. Se o seletor
  // também filtasse a si mesmo, ao escolher um valor ele encolheria pra 1 opção e não haveria
  // como trocar de modelo/projeto sem "limpar filtros" — o filtro funciona, a navegação é que
  // quebrava (medido na revisão de 06/08). O valor selecionado continua na lista, porque o filtro
  // da própria dimensão não é aplicado ao listá-la.
  const opcoesDa = (d: Dim): DimBucket[] => {
    if (!temCombos) return listaCrua(d);
    const cruzado = agruparPor(filtrar(base, { ...filtroAtivo, [d]: undefined }), d);
    // Valor selecionado que NÃO sobrevive ao cruzamento (subagente/prazo mudou e o valor só
    // existia no outro modo): anexa um balde zerado pra o `<select value=...>` nunca ficar em
    // branco — o recorte vazio é sinalizado como "sem dados" no KPI, não como estado sumindo da
    // UI. Nunca afeta os painéis (eles usam `listaDa`, não `opcoesDa`).
    const sel = filtroAtivo[d];
    if (sel && !cruzado.some((b) => b.key === sel) && base.some((c) => c[d] === sel)) {
      cruzado.push({ ...vazio(), key: sel });
    }
    return cruzado;
  };
  const opcoesProvedor = $derived(opcoesDa('provider'));
  const opcoesFonte = $derived(opcoesDa('source'));
  const opcoesProjeto = $derived(opcoesDa('project'));
  const opcoesModelo = $derived(opcoesDa('model'));
  // Do by_servidor, não do cruzamento: máquina que respondeu SEM detalhamento (versão antiga)
  // não tem combo nenhum e sumiria da lista — ficando impossível de filtrar.
  const opcoesServidor = $derived(report.by_servidor);

  // Os números do topo saem do CRUZAMENTO. Sem detalhamento sai do balde de UMA dimensão nos
  // `by_*`, exatamente como a tela fazia antes — e o `aplicar` garante que ali só existe uma
  // dimensão no filtro, senão este `find` descartaria a segunda em silêncio, com o rótulo do
  // recorte ainda anunciando as duas.
  const foco = $derived.by(() => {
    if (temCombos) return somar(recorte);
    const d = DIMS.find((x) => filtroAtivo[x]);
    if (!d) return report.totals;
    return listaCrua(d).find((b) => b.key === filtroAtivo[d]) ?? report.totals;
  });

  // O recorte também se lê pelo rótulo: com um provedor de conta selecionado, o aviso dizia
  // "Recorte: provedor anthropic:758a9521-…".
  const descricaoFiltro = $derived([
    ...DIMS.filter((d) => filtroAtivo[d])
      .map((d) => `${NOME_DIM[d]} ${nomeDa(d, filtroAtivo[d] as string)}`),
    ...(filtroAtivo.subagente === undefined
      ? [] : [filtroAtivo.subagente ? 'só subagente' : 'só conversa']),
  ].join(' · '));

  // As duas escritas passam pelo `aplicar`, que é quem sabe se o recorte pode CRUZAR: sem
  // detalhamento ele volta a ser de uma dimensão só, senão o rótulo diria "provedor X · projeto Y"
  // enquanto `foco` (que só acha um balde nos `by_*`) mostraria o número de X sozinho.
  function alternar(dim: Dim, key: string) {
    filtro = aplicar(filtro, dim, filtro[dim] === key ? undefined : key, temCombos);
  }
  function setFiltro(dim: Dim, valor: string) {
    filtro = aplicar(filtro, dim, valor || undefined, temCombos);
  }
  function limpar() {
    filtro = {};
    camadasOff = new Set();
  }

  // `sessions` é +1 por LINHA de transcript (backend/app/costs.py:_somar), e desde a fase 2 cada
  // Task disparada é uma linha própria — a palavra tem que seguir os TRÊS estados do filtro de
  // subagente, não só "exclui ou não": "só subagente" filtra pra SÓ linha de Task (`filtrar` em
  // cubo.ts), e "sessões e subagentes" ali afirmaria conversa que não está na conta. Sem
  // detalhamento (servidor antigo) não existe o filtro, e a soma é sempre mistura.
  const sess = (n: number) => {
    const um = n === 1;
    if (temCombos && filtroAtivo.subagente === false) return `${n} ${um ? 'sessão' : 'sessões'}`;
    if (temCombos && filtroAtivo.subagente === true) return `${n} ${um ? 'subagente' : 'subagentes'}`;
    return `${n} ${um ? 'sessão ou subagente' : 'sessões e subagentes'}`;
  };

  const diasDoPeriodo = $derived(
    PERIODOS.find((p) => p.id === period)!.dias || report.by_day.length || 1,
  );

  // Delta vs. a janela anterior — só sem recorte: `anterior` é o total da malha, e compará-lo
  // com o custo de UM projeto daria uma queda inventada.
  const delta = $derived.by(() => {
    if (temFiltro || !report.anterior || report.anterior.cost <= 0) return '';
    const d = ((report.totals.cost - report.anterior.cost) / report.anterior.cost) * 100;
    if (Math.abs(d) < 1) return 'igual ao período anterior';
    const rot = PERIODOS.find((p) => p.id === period)!.label;
    // Gastar mais não é "bom" nem "ruim" — é um fato. Sem verde/vermelho, que aqui seria
    // julgamento e não informação.
    return `${d > 0 ? '▲' : '▼'} ${dec(Math.abs(d), 0)}% vs. ${rot} anteriores`;
  });

  // ── Série do gráfico ────────────────────────────────────────────────────────
  // Com detalhamento, cada dia é empilhado por FONTE — o cruzamento dia × fonte, que os `by_*`
  // não carregavam (é por isso que a tela empilhava por tipo de token). Sem detalhamento, o
  // `by_day` sozinho só responde a quebra por tipo, e é nela que o gráfico cai.
  const camadas = $derived(
    temCombos
      ? agruparPor(recorte, 'source')
          .map((b) => ({ id: b.key, label: b.key, slot: corDaFonte(b.key) }))
      : TIPOS.map((t) => ({ id: t.id as string, label: t.label, slot: t.slot })),
  );
  const camadasVisiveis = $derived(camadas.filter((c) => !camadasOff.has(c.id)));

  interface DiaSerie { key: string; bucket: DimBucket; custos: Map<string, number> }
  // `fillDayGaps` devolve os buckets originais nos dias com registro e um zero magro nos buracos.
  // Recupero o dia completo pelo mapa: dia sem sessão é dia de gasto ZERO, não dia inexistente —
  // sem isso um fim de semana parado some e o eixo mente sobre o ritmo.
  const serie = $derived.by<DiaSerie[]>(() => {
    let dias: DiaSerie[];
    if (temCombos) {
      const porDia = new Map<string, ComboLocal[]>();
      for (const c of recorte) {
        const l = porDia.get(c.dia);
        if (l) l.push(c); else porDia.set(c.dia, [c]);
      }
      dias = [...porDia].map(([key, linhas]) => ({
        key, bucket: { ...somar(linhas), key },
        custos: new Map(agruparPor(linhas, 'source').map((b) => [b.key, b.cost])),
      })).sort((a, b) => b.key.localeCompare(a.key));
    } else {
      dias = report.by_day.map((b) => ({
        key: b.key, bucket: b,
        custos: new Map<string, number>(TIPOS.map((t) => [t.id as string, custoDe(b, t.id)])),
      }));
    }
    const mapa = new Map(dias.map((d) => [d.key, d]));
    const zeroDia = (key: string): DiaSerie =>
      ({ key, bucket: { ...vazio(), key }, custos: new Map() });
    return fillDayGaps(dias.map((d) => d.bucket))
      .map((b) => mapa.get(b.key) ?? zeroDia(b.key))
      .reverse(); // os dias vêm desc; o eixo do tempo anda pra frente
  });
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
      const segs = camadasVisiveis.map((cam) => {
        const c = d.custos.get(cam.id) ?? 0;
        const seg = { slot: cam.slot, label: cam.label, base: acc, cost: c };
        acc += c;
        return seg;
      }).filter((s) => s.cost > 0);
      return { dia: d, segs, total: acc };
    });
    // `Math.max(0, …) || 1`, não `Math.max(1, …)`: o piso em US$ 1 achatava dias de centavos
    // contra uma régua de 1 dólar (uma coluna de 3px pra US$ 0,04). O `|| 1` só salva o caso
    // degenerado de tudo zerado, onde a divisão por `topo` daria NaN.
    const max = Math.max(0, ...colunas.map((c) => c.total)) || 1;
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
  // Cada ranking é a SUA dimensão cruzada com os outros filtros — clicar num projeto reescreve o
  // "Por fonte" com as fontes daquele projeto, que é a pergunta que a fase 1 não respondia.
  const provedores = $derived(listaDa('provider'));
  const fontes = $derived(listaDa('source'));
  const projetos = $derived(listaDa('project'));
  const modelos = $derived(listaDa('model'));

  const fatias = $derived(
    TIPOS.map((t) => ({ ...t, cost: custoDe(foco, t.id), toks: tokensDe(foco, t.id) })),
  );

  // Quanto do recorte veio de subagente — o gasto que ficava misturado com a conversa até esta
  // fase. Só faz sentido com o filtro de subagente em "tudo": ativo, o recorte JÁ é um dos lados.
  const custoSubagente = $derived(
    temCombos && filtroAtivo.subagente === undefined
      ? somar(filtrar(recorte, { subagente: true })).cost : 0);

  // Partição + régua em lib/costs.ts, com teste: a garantia é "esconder não muda total nenhum".
  const partido = $derived(partirOcultos(projetos, ocultos));
  const projetosOcultos = $derived(partido.escondidos);
  const picoProjeto = $derived(partido.pico);

  // Pico pelo MAIOR da lista, não pelo primeiro item: presumir que a lista já veio ordenada
  // acopla a barra a uma garantia que mora noutro arquivo (`ordenar`, em lib/costs.ts).
  const picoProvedor = $derived(Math.max(1, ...provedores.map((b) => b.cost)));
  const picoFonte = $derived(Math.max(1, ...fontes.map((b) => b.cost)));
  const picoModelo = $derived(Math.max(1, ...modelos.map((b) => b.cost)));
  // Pico pelo by_servidor do período inteiro, não pelo cruzamento: o painel "Por máquina" é o
  // único que não acompanha o recorte (a máquina sem detalhamento não tem combo pra cruzar).
  const picoServidor = $derived(Math.max(1, ...report.by_servidor.map((b) => b.cost)));
  // "% conta" da tabela por modelo é dentro do RECORTE, não da malha inteira: com um projeto
  // escolhido, as linhas já são só daquele projeto, e dividir pelo total global daria percentuais
  // que nunca somam 100 sem nada dizer a respeito.
  const totalModelos = $derived(modelos.reduce((t, b) => t + b.cost, 0));

  // `has` = existe preço; `get` = qual preço (null com mais de um provedor). Ver tarifasPorModelo.
  const tarifas = $derived(tarifasPorModelo(report.rates));

  // Os dois escalares do servidor (custo_sem_cache/equivalente_cobrado) valem só pro total do
  // período. Com recorte o cliente recalcula com as tarifas que viajam nos `rates` (mesma
  // aritmética do costs.py); sem detalhamento (servidor antigo) não existe recorte e os valores
  // do período inteiro continuam valendo — a ressalva do painel explica.
  const semCache = $derived(
    temCombos && temFiltro ? custoSemCacheDe(recorte, tarifas) : report.custo_sem_cache);
  const equivalenteRecorte = $derived(
    temCombos && temFiltro ? equivalenteDe(recorte, tarifas) : report.equivalente_cobrado);
  // `fonteCache` é o alvo do par "pago de verdade × se nada fosse cache": com detalhamento o
  // recorte, sem ele os totais do período (o escalar do servidor é global — comparar recorte
  // com período inteiro daria uma "economia" sem sentido, apontado na revisão).
  const fonteCache = $derived(temCombos ? foco : report.totals);
  const economia = $derived(semCache - fonteCache.cost);
  const taxaCache = $derived(brutos(fonteCache) > 0 ? fonteCache.cache_read / brutos(fonteCache) : 0);
  // Recorte sem tarifa: o `cost` que vem do servidor é 0 porque ele pulou a conta, não porque foi
  // de graça. Todo número em dinheiro deste recorte vira traço. A pergunta é feita ao BALDE, não
  // à dimensão: perguntar "é um modelo?" deixava passar o provedor, a fonte e o projeto cujos
  // modelos sejam todos desconhecidos — mesma mentira, um eixo acima.
  // Recorte inteiramente de modelo grátis (`:free`/`-free`): o custo zero NÃO é "não sei o
  // preço" — é grátis de verdade. O traço do "sem tarifa" só vale quando há volume sem preço
  // conhecido; no recorte grátis o número certo é US$ 0,00. `tudoGratis` sai dos combos, não do
  // balde, porque o balde agregado não guarda os ids dos modelos.
  const tudoGratis = $derived(
    temCombos && recorte.length > 0 && recorte.every((c) => isFree(c.model)));
  const semTarifa = $derived(custoDesconhecido(foco) && !tudoGratis);
  // Recorte VAZIO com filtro ativo (os filtros não têm sobreposição, ex: projeto só-subagente com
  // "só conversa" ligado): o balde zerado não é custo zero — é "não há dados". O KPI mostra traço
  // em vez de US$ 0,00, que leria como gasto real.
  const recorteVazio = $derived(temCombos && temFiltro && recorte.length === 0);
  // Rodapé: os grátis saem da lista de "sem tarifa conhecida" — o rótulo ali contradiz o "grátis"
  // da linha do modelo. Cada grupo ganha a própria frase.
  const semTarifaFooter = $derived(report.sem_tarifa.filter((m) => !isFree(m)));
  const freeFooter = $derived(report.sem_tarifa.filter((m) => isFree(m)));
  const mFoco = (n: number) => (semTarifa || recorteVazio ? '—' : m(n));
  const m2Foco = (n: number) => (semTarifa || recorteVazio ? '—' : m2(n));
  // Par do painel de cache: com detalhamento o recorte, sem ele os totais do período — o traço
  // segue o MESMO alvo do par (sem detalhamento + filtro, o painel continua período inteiro).
  const semTarifaCache = $derived(
    temCombos ? (semTarifa || recorteVazio) : custoDesconhecido(report.totals));
  const mPainel = (n: number) => (semTarifaCache ? '—' : m(n));
  const m2Painel = (n: number) => (semTarifaCache ? '—' : m2(n));

  // Ressalva dos painéis que NÃO obedecem ao recorte. Com detalhamento eles obedecem, e a ressalva
  // some — ela só sobra pro servidor da malha em versão antiga, onde de fato só existe o total de
  // cada dimensão. Sem ela ali, "Gasto por dia" com recorte ativo se lê como "gasto por dia DO
  // recorte": o número está certo e o rótulo engana.
  const RESSALVA = $derived(temCombos ? ''
    : ' Sempre o período inteiro: sem o detalhamento do servidor só existe o total de cada dimensão, não o cruzamento entre elas.');
  // Sem detalhamento o painel de cache segue no período inteiro — `custo_sem_cache` é escalar do
  // servidor e não existe o cruzamento pra recalcular —, então a ressalva dele é a própria. Com
  // detalhamento o recorte recalcula e ela some. (Espaço na string: Svelte come o espaço que fica
  // colado no `{#if}`, e "cache.Sempre" saía grudado.)
  const RESSALVA_CACHE = ' Sempre o período inteiro — o servidor calcula isto no total, não por recorte.';

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

    <!-- Cada seletor lista a SUA dimensão dentro do RECORTE completo (listaDa aplica até o
         próprio filtro): com um filtro ativo a lista fica só com o que existe nele, e trocar de
         valor é via "limpar filtros". -->
    <span class="fgroup">
      <span class="flabel" id="lbl-prov">provedor</span>
      <Select ariaLabel="provedor" value={filtroAtivo.provider ?? ''}
        opcoes={[{ value: '', label: `todos (${opcoesProvedor.length})` },
                 ...opcoesProvedor.map((b) => ({ value: b.key, label: rot(b), hint: custoDesconhecido(b) ? '—' : m(b.cost) }))]}
        onchange={(v) => setFiltro('provider', v)} />
    </span>

    <span class="fgroup">
      <span class="flabel" id="lbl-fonte">fonte</span>
      <Select ariaLabel="fonte" value={filtroAtivo.source ?? ''}
        opcoes={[{ value: '', label: `todas (${opcoesFonte.length})` },
                 ...opcoesFonte.map((b) => ({ value: b.key, label: b.key, hint: custoDesconhecido(b) ? '—' : m(b.cost) }))]}
        onchange={(v) => setFiltro('source', v)} />
    </span>

    <span class="fgroup">
      <span class="flabel" id="lbl-proj">projeto</span>
      <!-- title: dois projetos com o mesmo basename (raro, mas possível) ficam distinguíveis por
           hover; no celular não há hover, então o painel e o chip mostram o basename e o estado
           sempre usa a chave cheia. O filtro por digitação da lista também busca na chave. -->
      <Select ariaLabel="projeto" value={filtroAtivo.project ?? ''}
        opcoes={[{ value: '', label: `todos (${opcoesProjeto.length})` },
                 ...opcoesProjeto.map((b) => ({ value: b.key, label: projectLabel(b.key), title: b.key,
                                                hint: custoDesconhecido(b) ? '—' : m(b.cost) }))]}
        onchange={(v) => setFiltro('project', v)} />
    </span>

    <span class="fgroup">
      <span class="flabel" id="lbl-mod">modelo</span>
      <!-- Mesma regra da tabela: modelo sem tarifa não vale "US$ 0,00" nem aqui. -->
      <Select ariaLabel="modelo" value={filtroAtivo.model ?? ''}
        opcoes={[{ value: '', label: `todos (${opcoesModelo.length})` },
                 ...opcoesModelo.map((b) => ({ value: b.key, label: b.key,
                   hint: tarifas.has(b.key) ? m(b.cost) : isFree(b.key) ? 'grátis' : 'sem tarifa' }))]}
        onchange={(v) => setFiltro('model', v)} />
    </span>

    <!-- Só com malha: com uma máquina só, "quais máquinas" não é escolha. -->
    {#if servidores.length > 1}
      <span class="fgroup">
        <span class="flabel">máquina</span>
        <Select ariaLabel="máquina" value={filtroAtivo.servidor ?? ''}
          opcoes={[{ value: '', label: `todas (${opcoesServidor.length})` },
                   ...opcoesServidor.map((b) => ({ value: b.key, label: nomeServidor(b.key),
                     hint: custoDesconhecido(b) ? '—' : m(b.cost) }))]}
          onchange={(v) => setFiltro('servidor', v)} />
      </span>
    {/if}

    <!-- Só com malha: com uma máquina só, escolher QUAIS entram no relatório não é escolha. -->
    {#if servidores.length > 1}
      <span class="fgroup">
        <span class="flabel">servidores</span>
        <button class="chip" aria-expanded={mostrarServidores}
          onclick={() => (mostrarServidores = !mostrarServidores)}>
          {servidoresAtivos.length} de {servidores.length} ▾
        </button>
      </span>
    {/if}

    <!-- Subagente é a única dimensão que não é uma lista: é um recorte de três estados. Só existe
         com detalhamento — os `by_*` já vêm somados com o subagente dentro. -->
    {#if temCombos}
      <span class="fgroup">
        <span class="flabel">subagente</span>
        <span class="seg" role="group" aria-label="Subagente">
          <button aria-pressed={filtroAtivo.subagente === undefined}
            onclick={() => (filtro = { ...filtro, subagente: undefined })}>tudo</button>
          <button aria-pressed={filtroAtivo.subagente === false}
            onclick={() => (filtro = { ...filtro, subagente: false })}>só conversa</button>
          <button aria-pressed={filtroAtivo.subagente === true}
            onclick={() => (filtro = { ...filtro, subagente: true })}>só subagente</button>
        </span>
      </span>
    {/if}

    <span class="seg" role="group" aria-label="Moeda">
      <button aria-pressed={currency === 'USD'} onclick={() => setCurrency('USD')}>US$</button>
      <button aria-pressed={currency === 'BRL'} onclick={() => setCurrency('BRL')}
        disabled={!rate} title={rate ? undefined : 'cotação indisponível'}>R$</button>
    </span>

    <button class="clear" onclick={limpar}>limpar filtros</button>
  </div>

  {#if mostrarServidores && servidores.length > 1}
    <div class="chips" role="group" aria-label="Servidores no relatório">
      {#each servidores as s (s.id)}
        <button class="chip" aria-pressed={!servidoresOff.has(s.id)}
          onclick={() => alternarServidor(s.id)}>{s.label}</button>
      {/each}
      {#if servidoresOff.size}
        <button class="chip todos" onclick={todosServidores}>todos</button>
      {/if}
    </div>
  {/if}

  {#if merged.partial}
    <p class="warn">
      <!-- As duas causas são independentes e podem acontecer JUNTAS: quando aconteciam, só a
           segunda era nomeada e o servidor caído sumia num "algum servidor" sem nome nem
           contagem. Cada oração sai por conta própria, com quantos e quais. -->
      ⚠ Total parcial.
      {#if merged.failed.length}
        {merged.failed.length === 1
          ? '1 servidor não respondeu'
          : `${merged.failed.length} servidores não responderam`}
        ({merged.failed.join(', ')}).
      {/if}
      {#if merged.mismatched.length}
        {merged.mismatched.length === 1
          ? '1 servidor respondeu fora do período pedido e ficou de fora da soma'
          : `${merged.mismatched.length} servidores responderam fora do período pedido e ficaram de fora da soma`}
        ({merged.mismatched.join(', ')}).
      {/if}
      <button class="retry" onclick={() => load(period, servidoresAtivos)}>Tentar de novo</button>
    </p>
  {/if}

  {#if temFiltro}
    <p class="recorte">
      Recorte: <b>{descricaoFiltro}</b> —
      {#if temCombos}a tela inteira é deste recorte, menos a comparação com o período
        anterior — o servidor só calcula ela no total.
      {:else}os números do topo e a quebra por tipo de token são deste recorte. Os painéis abaixo
        continuam no período inteiro: sem o detalhamento do servidor só existe o total de cada
        dimensão, não o cruzamento entre elas.
      {/if}
      <!-- Só o recorte: a legenda do gráfico é escolha à parte, e apagá-la aqui surpreende. -->
      <button class="retry" onclick={() => (filtro = {})}>tirar recorte</button>
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
        <!-- Traço, nunca US$ 0,00, quando o recorte é um modelo sem tarifa ou vazio: o zero que o
             backend manda ali é "não sei o preço"/"não há dados", e como número principal da tela
             ele afirmaria "não custou nada". -->
        <dd class="hero" class:tracinho={semTarifa || recorteVazio}>{mFoco(foco.cost)}</dd>
        <div class="foot">
          {m2Foco(foco.cost)} · {sess(foco.sessions)} · {m2Foco(foco.cost / diasDoPeriodo)}/dia
        </div>
        {#if semTarifa}<div class="foot">sem tarifa conhecida — só o volume é medido</div>
        {:else if tudoGratis}<div class="foot">modelo grátis — nada é cobrado</div>
        {:else if recorteVazio}<div class="foot">sem dados neste recorte — os filtros não têm sobreposição</div>{/if}
        <!-- O gasto de subagente ficava misturado com o da conversa: a fase 1 não tinha como
             separá-los. Só aparece com o filtro em "tudo" — ativo, o número acima JÁ é um dos
             dois lados e a fração seria 0% ou 100%. -->
        {#if custoSubagente > 0 && foco.cost > 0}
          <div class="foot">{pct(custoSubagente, foco.cost)} veio de subagente</div>
        {/if}
        {#if delta}<div class="foot">{delta}</div>{/if}
        <!-- Diferente do × do painel de projetos, que esconde sem tirar da conta: aqui desmarcar
             TIRA. A linha não é opcional — é a diferença entre "meu gasto" e "parte do meu
             gasto". -->
        {#if servidoresAtivos.length < servidores.length}
          <div class="foot">somando {servidoresAtivos.length} de {servidores.length} máquinas</div>
        {/if}
      </div>
      <div class="kpi">
        <dt>tokens brutos</dt>
        <dd>{tok(brutos(foco))}</dd>
        <div class="foot">passaram pelo modelo</div>
      </div>
      <div class="kpi">
        <dt>equivalente cobrado</dt>
        <!-- Com detalhamento o front recalcula do recorte (tarifas viajam nos rates); sem ele o
             escalar do servidor vale só pro total do período, e dentro de um recorte é traço,
             nunca o número global. -->
        <dd>{temFiltro && (!temCombos || semTarifa || recorteVazio) ? '—' : tok(equivalenteRecorte)}</dd>
        <div class="foot">
          {#if temFiltro && !temCombos}só no total do período
          {:else if recorteVazio}sem dados neste recorte
          {:else if semTarifa}só o volume é medido
          {:else if tudoGratis}modelo grátis — nada é cobrado
          {:else}{pct(equivalenteRecorte, brutos(foco))} do bruto — o resto é cache barato{/if}
        </div>
      </div>
      <div class="kpi">
        <dt>economia do cache</dt>
        <dd>{temFiltro && !temCombos ? '—' : mFoco(economia)}</dd>
        <div class="foot">
          {#if temFiltro && !temCombos}só no total do período
          {:else if recorteVazio}sem dados neste recorte
          {:else}{m2Foco(economia)} · {semCache > 0
            ? dec(100 - (foco.cost / semCache) * 100, 0)
            : 0}% abaixo do preço cheio{/if}
        </div>
      </div>
    </dl>

    <div class="card">
      <h2>Gasto por dia</h2>
      <p class="hint">
        Empilhado por {temCombos ? 'fonte' : 'tipo de token'}. Cada coluna é um
        dia.{#if temFiltro}{RESSALVA}{/if}
      </p>
      <div class="legend">
        {#each camadas as c}
          <button aria-pressed={!camadasOff.has(c.id)}
            onclick={() => {
              const s = new Set(camadasOff);
              if (s.has(c.id)) s.delete(c.id); else s.add(c.id);
              if (s.size === camadas.length) s.clear();
              camadasOff = s;
            }}>
            <span class="swatch" style="background: var({c.slot})"></span>{c.label}
          </button>
        {/each}
      </div>
      <div class="chartbox" bind:clientWidth={larguraGrafico}>
        {#if serie.length}
          <svg viewBox="0 0 {grafico.W} {grafico.H}" width={grafico.W} height={grafico.H}
            role="img"
            aria-label="Gasto por dia, empilhado por {temCombos ? 'fonte' : 'tipo de token'}">
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
          <b>{rotuloDia(b.dia.key)}</b> · {m2(b.total)} · {sess(b.dia.bucket.sessions)}
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
        <!-- Sem tarifa, os quatro custos vêm zerados do servidor: a barra 100% seria uma faixa
             vazia que se lê como "nada custou". Some, e a tabela abaixo fica só com o volume. -->
        {#if semTarifa}
          <!-- Sem recorte o traço também é possível (malha inteira sem um modelo com tarifa), e aí
               não há filtro nenhum — o texto tem que continuar dizendo do que ele fala. -->
          <p class="hint">
            Sem tarifa conhecida para
            <b>{temFiltro ? descricaoFiltro : 'nenhum modelo do período'}</b>
            — aqui só o volume é medido.
          </p>
        {:else if tudoGratis}
          <!-- Recorte de modelo grátis: custo zero é verdade, não "não sei". A barra 100% seria uma
               faixa vazia (nada foi cobrado) — o texto explica em vez de desenhar o vazio. -->
          <p class="hint">Modelo grátis — nenhum dólar foi gasto.</p>
        {:else if recorteVazio}
          <!-- Recorte vazio: sem sobreposição entre os filtros. Mesmo motivo do tudoGratis — a barra
               100% vazia + a tabela de US$ 0,00 leriam como "nada custou", logo abaixo do KPI que
               já diz "sem dados neste recorte". -->
          <p class="hint">Sem dados neste recorte — os filtros não têm sobreposição.</p>
        {:else}
          <div class="stack100">
            {#each fatias as f}
              {#if f.cost > 0}
                <i style="background: var({f.slot}); flex: {f.cost}" title="{f.label}: {m2(f.cost)}"></i>
              {/if}
            {/each}
          </div>
        {/if}
        <div class="twrap"><table class="breakdown">
          <thead>
            <tr><th></th><th class="n">tokens</th><th class="n">custo</th><th class="n">% da conta</th></tr>
          </thead>
          <tbody>
            {#each fatias as f}
              <tr>
                <td class="name"><span class="swatch" style="background: var({f.slot})"></span>{f.label}</td>
                <td class="n">{tok(f.toks)}</td>
                <td class="n">{m2Foco(f.cost)}</td>
                <td class="n dim">{semTarifa ? '—' : pct(f.cost, foco.cost)}</td>
              </tr>
            {/each}
          </tbody>
        </table></div>
      </div>

      <div class="card">
        <h2>O que o cache economizou</h2>
        <p class="hint">Os mesmos tokens, se nenhum fosse cache.{#if !temCombos && temFiltro}{RESSALVA_CACHE}{/if}</p>
        <div class="cmp">
          <!-- `fonteCache`: com detalhamento o recorte, sem ele os totais do período — o par
               precisa vir da MESMA fonte do `semCache`, senão compara recorte com período
               inteiro. O traço acompanha o mesmo alvo (m2Painel/mPainel). -->
          <div class="cmprow"><span class="dim">pago de verdade</span><b>{m2Painel(fonteCache.cost)}</b></div>
          <div class="cmptrack">
            <i style="width: {semCache > 0
              ? Math.max(2, (fonteCache.cost / semCache) * 100) : 0}%"></i>
          </div>
          <div class="cmprow"><span class="dim">se nada fosse cache</span><b>{m2Painel(semCache)}</b></div>
        </div>
        <dl class="kpis compacta">
          <div class="kpi"><dt>economizado</dt><dd class="aqua">{mPainel(economia)}</dd></div>
          <div class="kpi"><dt>do volume é cache lido</dt><dd>{dec(taxaCache * 100, 0)}%</dd></div>
        </dl>
      </div>
    </div>

    <div class="cols">
      <div class="card">
        <h2>Por provedor</h2>
        <p class="hint">Quem cobra a conta. Um provedor atravessa várias fontes.{#if temFiltro}{RESSALVA}{/if}</p>
        <div class="rank">
          {#each provedores as b}
            <div class="row">
              <button aria-pressed={filtroAtivo.provider === b.key}
                onclick={() => alternar('provider', b.key)}>
                <span class="nm">{rot(b)}</span><span class="vl">{custoDesconhecido(b) ? '—' : m2(b.cost)}</span>
                <span class="track" style="width: {Math.max(1.5, (b.cost / picoProvedor) * 100)}%">
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
        <p class="hint">Qual agente rodou. Uma fonte usa vários provedores.{#if temFiltro}{RESSALVA}{/if}</p>
        <div class="rank">
          {#each fontes as b}
            <div class="row">
              <button aria-pressed={filtroAtivo.source === b.key}
                onclick={() => alternar('source', b.key)}>
                <span class="nm">{b.key}</span><span class="vl">{custoDesconhecido(b) ? '—' : m2(b.cost)}</span>
                <span class="track" style="width: {Math.max(1.5, (b.cost / picoFonte) * 100)}%">
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

    {#if servidores.length > 1}
      <div class="card">
        <h2>Por máquina</h2>
        <p class="hint">
          Onde a sessão rodou. Sempre o período inteiro de cada máquina — este corte vem do total
          que ela respondeu, e é o único que também enxerga máquina sem detalhamento. Clique para
          recortar o resto da tela.
        </p>
        <div class="rank">
          {#each report.by_servidor as b (b.key)}
            <div class="row">
              <button aria-pressed={filtroAtivo.servidor === b.key}
                onclick={() => alternar('servidor', b.key)}>
                <span class="nm">{nomeServidor(b.key)}</span>
                <span class="vl">{custoDesconhecido(b) ? '—' : m2(b.cost)}</span>
                <span class="track" style="width: {Math.max(1.5, (b.cost / picoServidor) * 100)}%">
                  {#each TIPOS as t}{#if custoDe(b, t.id) > 0}<i style="background: var({t.slot}); flex: {custoDe(b, t.id)}"></i>{/if}{/each}
                </span>
              </button>
            </div>
          {:else}
            <p class="empty">Sem dados no período.</p>
          {/each}
        </div>
      </div>
    {/if}

    <div class="card">
      <h2>Por projeto</h2>
      <p class="hint">
        A pasta onde a sessão rodou. Clique para recortar; o × tira da lista sem tirar da conta.{#if temFiltro}{RESSALVA}{/if}
      </p>
      <div class="rank">
        {#each partido.visiveis as b}
          <div class="row">
            <button aria-pressed={filtroAtivo.project === b.key}
              title="clique para recortar" onclick={() => alternar('project', b.key)}>
              <span class="nm" title={b.key}>{projectLabel(b.key)}</span><span class="vl">{custoDesconhecido(b) ? '—' : m2(b.cost)}</span>
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
        {#if projetosOcultos.length}
          <div class="hiddenbar">
            <span>
              {projetosOcultos.length} fora da lista
              ({m2(projetosOcultos.reduce((t, b) => t + b.cost, 0))}, ainda somando no total):
            </span>
            {#each projetosOcultos as b}
              <button title={b.key} onclick={() => { const s = new Set(ocultos); s.delete(b.key); ocultos = s; salvarOcultos(); }}>
                {projectLabel(b.key)} ✕
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
      <p class="hint">Com a tarifa aplicada e de onde ela veio. Clique para recortar.{#if temFiltro}{RESSALVA}{/if}</p>
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
            {#each modelos as b}
              <!-- Duas perguntas diferentes: `comPreco` é "existe tarifa?" (manda no custo e no %,
                   que o backend só calcula quando há preço); `t` é "qual tarifa mostrar?", e vem
                   null quando o mesmo modelo aparece em mais de um provedor — aí o custo da linha
                   soma os dois e exibir o preço de um deles ao lado dele seria inventar a origem
                   daquele número. -->
              {@const comPreco = tarifas.has(b.key)}
              {@const t = tarifas.get(b.key)}
              <!-- Um servidor da malha sabe a tarifa e outro (catálogo mais velho) não: a linha
                   sai com o volume dos DOIS e o custo de UM, e sem marca isso lê como preço
                   completo. Só acontece na mescla — dentro de um servidor é impossível. -->
              {@const parcial = precoParcial(b.key, comPreco, report.sem_tarifa)}
              <tr class="click" aria-selected={filtroAtivo.model === b.key}
                onclick={() => alternar('model', b.key)}>
                <td>{b.key}{#if !comPreco}<span class="tag">{isFree(b.key) ? 'grátis' : 'sem tarifa'}</span>{:else if parcial}<span
                  class="tag" title="um servidor da malha não conhece a tarifa deste modelo — o volume dele conta, o custo não">preço parcial</span>{/if}</td>
                <td class="n">{tok(b.input)}</td>
                <td class="n">{tok(b.output)}</td>
                <td class="n">{tok(b.cache_read)}</td>
                <td class="n dim">{t ? `${dec(t.input, 2)}/${dec(t.output, 2)}` : '—'}</td>
                <td class="n dim">{t ? t.origin : '—'}{#if t?.cache_estimado}<span class="tag">cache estimado</span>{/if}</td>
                <td class="n dim">{comPreco ? pct(b.cost, totalModelos) : '—'}</td>
                <!-- Sem tarifa mostra TRAÇO, nunca US$ 0,00: zero afirma "não custou nada", que é
                     uma mentira diferente de "não sei o preço". -->
                <td class="c">{#if comPreco}{m2(b.cost)}{:else}<span class="tracinho">—</span>{/if}</td>
                <td class="bar">
                  {#if comPreco}
                    <span class="mini"><i style="width: {Math.max(2, (b.cost / picoModelo) * 100)}%"></i></span>
                  {/if}
                </td>
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
        <b>Fontes:</b> Claude Code (<code>~/.claude/projects/**/*.jsonl</code>) ·
        Codex (<code>~/.codex/sessions</code>) · Pi (<code>~/.pi/agent/sessions</code>).<br />
        <b>Tarifas</b> do models.dev, aplicadas ao histórico inteiro — não há preço histórico,
        então gasto antigo é recalculado com o preço de hoje.<br />
        {#if currency === 'BRL' && rate}<b>Cotação</b> US$ 1 = R$ {dec(rate, 2)}.<br />{/if}
        Custo de tabela da API, <b>não é fatura</b>: plano de assinatura não cobra por token.
        {#if freeFooter.length}
          <br />{freeFooter.length}
          {freeFooter.length === 1 ? 'modelo' : 'modelos'} grátis
          ({freeFooter.join(', ')}).
        {/if}
        {#if semTarifaFooter.length}
          <br />{semTarifaFooter.length}
          {semTarifaFooter.length === 1 ? 'modelo' : 'modelos'} sem tarifa conhecida
          ({semTarifaFooter.join(', ')}) — aparecem com traço, nunca estimados.
        {/if}
      </p>
    </div>
  {/if}
 </div>
</div>

<style>
  /* A paleta (`--chart-1..4`) mora no app.css, nos dois temas — aqui só se referencia. */
  .costs {
    --grid-line: var(--border-subtle);
    --axis-line: var(--border-default);
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
  .chips { display: flex; flex-wrap: wrap; gap: var(--space-1); margin-bottom: var(--space-2); }
  /* surface-raised, não bg-elevated: com papel de parede o chip tem que entrar no mesmo véu do
     painel, senão vira retângulo chapado boiando sobre a foto. */
  .chip {
    background: var(--surface-raised);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    font-family: var(--font-mono); font-size: 12px;
    padding: 5px var(--space-2);
    cursor: pointer;
  }
  .chip[aria-pressed='true'] { color: var(--text-primary); border-color: var(--accent); }
  .chip:disabled { opacity: 0.45; cursor: default; }
  .chip.todos { color: var(--accent); }
  /* :global porque o campo agora é o <button> de dentro do Select.svelte (o nativo abria a lista
     pra cima e ela era cortada pelo overflow do painel). A barra de filtros é COMPACTA — 4 controles
     na mesma linha —, então continua sobrescrevendo altura/fonte/largura do padrão do componente. */
  .fgroup :global(.sel-campo) {
    font-size: var(--text-xs); height: 34px; max-width: 190px; width: auto;
    padding: 0 10px;
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
  /* O traço do "não sei o preço" não pode herdar o tamanho e a cor de destaque do número: a 38px
     em índigo ele lê como barra de carregamento, não como texto. */
  .kpi dd.hero.tracinho { color: var(--text-muted); font-size: 30px; }
  .kpi dd.aqua { color: var(--chart-3); }
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
  .cmptrack > i { display: block; height: 100%; background: var(--chart-1); }

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
