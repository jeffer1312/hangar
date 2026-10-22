import { useCallback, useEffect, useState } from 'react';
import { ScrollView, View } from 'react-native';
import { useIsFocused, useRouter } from 'expo-router';
import { custoDesconhecido, custoSemCacheDe, equivalenteDe, fillDayGaps, intlLocale, isFree, mergeReports, mergeUso, partirOcultos, precoParcial, tarifasPorModelo, type DimBucket } from '@hangar/core';
import { Sheet } from '../../ui/Sheet';
import { Action, Bar, Card, Choice, Columns, compactNumber, Composition, Copy, dollars, Grid, Metric, number, Paged, percent, ReportFrame, ReportStatus, useChartColors, useReportSelection, useUsagePreferences } from './ReportUI';
import { comparison, costView, dimensionBuckets, dimensions, filterCombos, groupCombos, rawTokens, setCostFilter, sumCombos, type CostFilter, type Dimension } from './costs-model';
import { useReports } from './useReports';
import * as m from '../../paraglide/messages';
import { SPACE } from '../../theme/scale';

const dimensionNames = () => ({ provider: m.custos_dim_provedor(), source: m.custos_dim_fonte(), project: m.custos_dim_projeto(), model: m.custos_dim_modelo(), servidor: m.custos_dim_maquina() });
const typeNames = () => ({ input: m.custos_input_sem_cache(), output: m.custos_output_total(), cache_write: m.custos_tipo_cache_escrito(), cache_read: m.custos_tipo_cache_lido() });
const tokenTypes = ['input', 'output', 'cache_write', 'cache_read'] as const;
type TokenType = typeof tokenTypes[number];

function ProjectRanking({ visible, buckets, cost, labelPrice, money, onSelect }: {
  visible: boolean;
  buckets: DimBucket[];
  cost: (bucket: DimBucket) => string;
  labelPrice: (bucket: DimBucket) => string;
  money: (value: number) => string;
  onSelect: (key: string) => void;
}) {
  const [hidden, setHidden] = useState(new Set<string>());
  const [projectLimit, setProjectLimit] = useState(12);
  if (!visible) return null;
  const split = partirOcultos([...buckets].sort((a, b) => b.cost - a.cost || a.key.localeCompare(b.key)), hidden);
  const max = Math.max(1, ...split.visiveis.map((bucket) => bucket.cost));
  const hiddenTotal = split.escondidos.reduce((sum, bucket) => sum + bucket.cost, 0);
  return <Card flat><Copy title>{m.custos_por_projeto()}</Copy><Copy muted>{m.custos_pasta_sessao()}</Copy>
    {projectLimit > 12 && <Action label={m.custos_mostrar_menos()} onPress={() => setProjectLimit(12)} />}
    {!split.visiveis.length && <Copy muted>{m.uso_vazio_secao()}</Copy>}{split.visiveis.slice(0, projectLimit).map((bucket) => <View key={bucket.key} style={{ gap: SPACE[1.5] }}>
      <View style={{ flexDirection: 'row', alignItems: 'center', gap: SPACE[2] }}><View style={{ flex: 1 }}><Bar compact label={bucket.label ?? bucket.key} value={bucket.cost} max={max} detail={`${cost(bucket)} · ${number(rawTokens(bucket))}`} onPress={() => onSelect(bucket.key)} /></View><Action label={m.custos_tirar_da_lista({ nome: bucket.key })} icon="EyeOff" iconOnly onPress={() => setHidden(new Set([...hidden, bucket.key]))} /></View>
      {!!labelPrice(bucket) && <Copy muted>{labelPrice(bucket)}</Copy>}
    </View>)}
    {split.visiveis.length > projectLimit && <Action label={projectLimit === 12 ? m.custos_ver_projetos({ n: split.visiveis.length }) : m.uso_mostrar_mais({ n: Math.min(50, split.visiveis.length - projectLimit) })} onPress={() => setProjectLimit(Math.min(projectLimit + 50, split.visiveis.length))} />}
    {split.escondidos.length > 0 && <><Copy muted>{m.custos_fora_da_lista({ n: split.escondidos.length, valor: hiddenTotal === 0 && split.escondidos.some(custoDesconhecido) ? '—' : money(hiddenTotal) })}</Copy>
      {split.escondidos.map((bucket) => <Action key={bucket.key} label={bucket.key} onPress={() => setHidden(new Set([...hidden].filter((key) => key !== bucket.key)))} />)}<Action label={m.custos_mostrar_todos()} onPress={() => setHidden(new Set())} /></>}
  </Card>;
}

export function CostsScreen() {
  const router = useRouter();
  const back = useCallback(() => router.back(), [router]);
  const openStats = useCallback(() => router.push('/stats' as never), [router]);
  const focused = useIsFocused();
  const { period, own, selected } = useReportSelection();
  const [filter, setFilter] = useState<CostFilter>({});
  const [methodOpen, setMethodOpen] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [brl, setBrl] = useState(false);
  const [sessionsExpanded, setSessionsExpanded] = useState(false);
  const [metric, setMetric] = useState<'cost' | 'tokens'>('tokens');
  const [hiddenLayers, setHiddenLayers] = useState<string[]>(['cache_read']);
  const colors = useChartColors();
  const [day, setDay] = useState<string | null>(null);
  const [compareDimension, setCompareDimension] = useState<Dimension>('provider');
  const [compareKeys, setCompareKeys] = useState<string[]>([]);
  const [compareMetric, setCompareMetric] = useState<'tokens' | 'cost'>('tokens');
  const load = useReports('costs', selected, period, {}, focused);
  const report = mergeReports(load.parts, period).report;
  const areaServers = selected.filter((server) => !filter.servidor?.length || filter.servidor.includes(server.id));
  const areaLoad = useReports('usage', areaServers, period, { projeto: filter.project ?? [], modelo: filter.model ?? [] }, focused);
  const areas = mergeUso(areaLoad.parts, period).report;
  const areaOrder = ['front', 'back', 'banco', 'infra', 'docs', 'outros', 'conversa'];
  const orderedAreas = [...areas.by_area].filter((item) => rawTokens(item) > 0).sort((a, b) => {
    const position = (key: string) => areaOrder.includes(key) ? areaOrder.indexOf(key) : areaOrder.indexOf('outros');
    return position(a.key) - position(b.key) || a.key.localeCompare(b.key);
  });
  const areaTokens = orderedAreas.reduce((sum, item) => sum + rawTokens(item), 0);
  const view = costView(report, filter);
  const activeFilter = dimensions.some((d) => filter[d]?.length) || filter.sub !== undefined;
  const emptySelection = view.cross && activeFilter && !view.rows.length;
  const missingPrices = report.sem_tarifa.filter((model) => !isFree(model) && (!view.cross || view.rows.some((row) => row.model === model)));
  const rates = tarifasPorModelo(report.rates);
  const validRate = report.usd_brl !== null && Number.isFinite(report.usd_brl) && report.usd_brl > 0;
  const money = (value: number) => brl && validRate ? new Intl.NumberFormat(intlLocale(), { style: 'currency', currency: 'BRL', minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(value * report.usd_brl!) : dollars(value);
  const moneyAxis = (value: number) => brl && validRate ? `R$ ${compactNumber(value * report.usd_brl!)}` : `US$ ${compactNumber(value)}`;
  const freeOnly = view.cross && view.rows.length > 0 && view.rows.every((row) => isFree(row.model));
  const unknownCost = (bucket: Pick<DimBucket, 'input' | 'output' | 'cache_write' | 'cache_read' | 'cost'>, model?: string) => (bucket === view.total && emptySelection) || (rawTokens(bucket) > 0 && bucket.cost === 0) && !isFree(model ?? '') && !((bucket === view.total || view.days.some((item) => item === bucket)) && freeOnly);
  const cost = (bucket: Pick<DimBucket, 'input' | 'output' | 'cache_write' | 'cache_read' | 'cost'>, model?: string) => unknownCost(bucket, model) ? '—' : money(bucket.cost);
  const labelPrice = (bucket: DimBucket, model?: string) => bucket === view.total && emptySelection ? '' : isFree(model ?? '') || bucket === view.total && freeOnly ? m.custos_gratis() : unknownCost(bucket, model) ? m.custos_sem_tarifa() : '';
  const names = dimensionNames();
  const types = typeNames();
  const total = view.total;
  const raw = rawTokens(total);
  const newTokens = total.input + total.output + total.cache_write;
  const noCache = view.cross ? custoSemCacheDe(view.rows, rates) : activeFilter ? null : report.custo_sem_cache;
  const equivalent = view.cross ? equivalenteDe(view.rows, rates) : activeFilter ? null : report.equivalente_cobrado;
  const periodDays = ({ '1d': 1, '7d': 7, '30d': 30, '90d': 90 } as Record<string, number>)[period] ?? Math.max(1, report.by_day.length);
  const periodLabel = ({ '1d': m.custos_periodo_1d(), '7d': m.custos_periodo_7d(), '30d': m.custos_periodo_30d(), '90d': m.custos_periodo_90d(), all: m.custos_periodo_tudo() } as Record<string, string>)[period];
  const savingsKnown = noCache !== null && (noCache > 0 || freeOnly) && !unknownCost(total);
  const sessionsText = (bucket: DimBucket, rows = view.rows) => {
    const word = (value: number, one: string, many: string) => `${number(value)} ${value === 1 ? one : many}`;
    if (!view.cross) return word(bucket.sessions, m.custos_sessao_ou_subagente(), m.custos_sessoes_e_subagentes());
    if (filter.sub === true) return word(bucket.sessions, m.custos_subagente(), m.custos_subagentes());
    const children = sumCombos(rows.filter((row) => row.subagente)).sessions;
    return `${word(bucket.sessions - children, m.sessao_singular(), m.lista_sessoes_plural())}${children ? ` · ${word(children, m.custos_subagente(), m.custos_subagentes())}` : ''}`;
  };
  const subCost = filter.sub === undefined ? sumCombos(view.rows.filter((row) => row.subagente)).cost : 0;
  const delta = !activeFilter && report.anterior && report.anterior.cost > 0 ? (report.totals.cost - report.anterior.cost) / report.anterior.cost * 100 : null;
  const namesOfServers = new Map(own.map((s) => [s.id, s.label]));
  const labelOf = (dim: Dimension, key: string) => {
    if (dim === 'servidor') return namesOfServers.get(key) ?? key;
    if (dim !== 'provider') return key;
    const name = report.by_provider.find((bucket) => bucket.key === key)?.label ?? key;
    const cli = key.startsWith('anthropic:') ? m.custos_claude_code() : key.startsWith('codex:') ? 'Codex' : '';
    return cli && !name.startsWith(`${cli} · `) ? `${cli} · ${name}` : name;
  };

  useEffect(() => {
    if (!load.accepted || load.loading || load.parts.some((part) => part.error)) return;
    const restricted: CostFilter = view.cross ? { sub: filter.sub } : {};
    for (const dim of dimensions) {
      const values = filter[dim]?.filter((key) => view.cross ? report.combos.some((row) => row[dim] === key) : dimensionBuckets(report, dim).some((bucket) => bucket.key === key));
      if (values?.length) {
        restricted[dim] = view.cross ? values : values.slice(0, 1);
        if (!view.cross) break;
      }
    }
    if (JSON.stringify(restricted) !== JSON.stringify(filter)) setFilter(restricted);
  }, [load.accepted, load.loading, load.parts, view.cross, filter]);

  const optionBuckets = (dim: Dimension) => {
    const rest = { ...filter }; delete rest[dim];
    const buckets = view.cross ? groupCombos(filterCombos(report.combos, rest), dim) : dimensionBuckets(report, dim);
    return buckets.map((item) => ({ key: item.key, label: labelOf(dim, item.key) }));
  };
  const clearFilters = () => {
    setFilter({});
    useUsagePreferences.setState({ off: [] });
    setHiddenLayers(metric === 'tokens' ? ['cache_read'] : []);
  };
  const filters = <Card>
    <Choice label={m.custos_servidores_relatorio()} options={own.map((server) => ({ key: server.id, label: server.label }))} values={selected.map((server) => server.id)} onChange={(keys) => useUsagePreferences.setState({ off: own.filter((server) => !keys.includes(server.id)).map((server) => server.id) })} />
    <Grid>{dimensions.map((dim) => <View key={dim}><Choice label={names[dim]} options={optionBuckets(dim)} values={filter[dim] ?? []} max={view.cross ? undefined : 1} onChange={(values) => setFilter(setCostFilter(filter, dim, values, view.cross))} /></View>)}</Grid>
    {view.cross && <View style={{ flexDirection: 'row', flexWrap: 'wrap' }}>{[[undefined, m.custos_tudo()], [false, m.custos_so_conversa()], [true, m.custos_so_subagente()]].map(([value, label]) => <Action key={String(value)} label={String(label)} selected={filter.sub === value} onPress={() => setFilter({ ...filter, sub: value as boolean | undefined })} />)}</View>}
    <Action label={m.custos_limpar_filtros()} onPress={clearFilters} />
  </Card>;
  const rankingTitles = { provider: m.custos_por_provedor(), source: m.custos_por_fonte(), servidor: m.custos_por_maquina() };
  const typeCost = (bucket: DimBucket, type: TokenType) => bucket === total && emptySelection || custoDesconhecido(bucket) && bucket[type] > 0 ? '—' : money(bucket[`cost_${type}`]);
  const costParts = (bucket: DimBucket) => tokenTypes.map((type, index) => ({ label: types[type], value: bucket[`cost_${type}`], color: colors[index], detail: typeCost(bucket, type) }));
  const renderTypes = (bucket: DimBucket) => <View style={{ gap: SPACE[2] }}>{tokenTypes.map((type) => <Bar key={type} label={types[type]} value={bucket[`cost_${type}`]} max={bucket.cost} detail={`${number(bucket[type])} · ${typeCost(bucket, type)} · ${m.custos_col_pct_conta()}: ${custoDesconhecido(bucket) && bucket[type] > 0 ? '—' : percent(bucket[`cost_${type}`], bucket.cost)}`} />)}</View>;
  const toggleFilter = (dim: Dimension, key: string) => setFilter(setCostFilter(filter, dim, filter[dim]?.includes(key) ? filter[dim]!.filter((value) => value !== key) : view.cross ? [...(filter[dim] ?? []), key] : [key], view.cross));

  function renderDaily() {
    const byType = metric === 'tokens' || !view.cross;
    const layers = byType ? tokenTypes.map((key, index) => ({ key, label: types[key], color: colors[index] })) : view.groups.source.map((bucket, index) => ({ key: bucket.key, label: bucket.label ?? bucket.key, color: colors[index % colors.length] }));
    const visible = layers.filter((item) => !hiddenLayers.includes(item.key));
    const days = fillDayGaps(view.days).reverse().map((item) => {
      const bucket = view.days.find((bucket) => bucket.key === item.key) ?? { ...sumCombos([]), key: item.key };
      return { key: item.key, values: visible.map((layer) => {
        if (byType) {
          const type = layer.key as TokenType;
          return metric === 'tokens' ? bucket[type] : custoDesconhecido(bucket) && bucket[type] > 0 ? null : bucket[`cost_${type}`];
        }
        const part = sumCombos(view.rows.filter((row) => row.dia === item.key && row.source === layer.key));
        return custoDesconhecido(part) ? null : part.cost;
      }) };
    });
    const changeMetric = (value: 'cost' | 'tokens') => { setMetric(value); setHiddenLayers(value === 'tokens' ? ['cache_read'] : []); };
    return <Card flat><Copy title>{m.custos_uso_por_dia()}</Copy>
      <View accessibilityLabel={m.custos_metrica_diaria()} style={{ flexDirection: 'row', flexWrap: 'wrap' }}><Action label={m.ctx_tokens()} selected={metric === 'tokens'} onPress={() => changeMetric('tokens')} /><Action label={m.custos_estimativa_api()} selected={metric === 'cost'} onPress={() => changeMetric('cost')} /></View>
      <Copy muted>{m.custos_empilhado_por({ modo: byType ? m.custos_tipo_token() : m.custos_dim_fonte() })}</Copy><Copy muted>{m.custos_hover_detalhe()}</Copy>
      {metric === 'tokens' && <Copy muted>{m.custos_cache_lido_desligado()}</Copy>}
      <View style={{ flexDirection: 'row', flexWrap: 'wrap' }}>{layers.map((layer) => <View key={layer.key} style={{ flexDirection: 'row', alignItems: 'center' }}><View style={{ width: 8, height: 8, backgroundColor: layer.color }} /><Action label={layer.label} selected={!hiddenLayers.includes(layer.key)} onPress={() => setHiddenLayers((previous) => {
        const next = previous.includes(layer.key) ? previous.filter((key) => key !== layer.key) : [...previous, layer.key];
        return layers.every((item) => next.includes(item.key)) ? [] : next;
      })} /></View>)}</View>
      <Columns fit label={m.custos_grafico_diario({ metrica: metric === 'tokens' ? m.ctx_tokens() : m.custos_estimativa_api() })} days={days} series={visible} format={metric === 'tokens' ? number : money} axisFormat={metric === 'tokens' ? compactNumber : moneyAxis} onPress={setDay} />
    </Card>;
  }

  function renderRanking(dim: keyof typeof rankingTitles) {
    const buckets = [...(dim === 'servidor' ? report.by_servidor : view.groups[dim])].sort((a, b) => b.cost - a.cost || a.key.localeCompare(b.key));
    const max = Math.max(1, ...buckets.map((bucket) => bucket.cost));
    const row = (bucket: DimBucket) => <View key={bucket.key} style={{ gap: SPACE[1.5] }}>
      <Bar compact label={dim === 'provider' ? labelOf(dim, bucket.key) : bucket.label ?? labelOf(dim, bucket.key)} value={bucket.cost} max={max} detail={`${cost(bucket)} · ${number(rawTokens(bucket))}`} onPress={() => toggleFilter(dim, bucket.key)} />
      {!!labelPrice(bucket) && <Copy muted>{labelPrice(bucket)}</Copy>}
    </View>;
    return <Card flat><Copy title>{rankingTitles[dim]}</Copy><Copy muted>{dim === 'provider' ? m.custos_quem_cobra() : dim === 'source' ? m.custos_qual_agente() : m.custos_onde_sessao()}</Copy>
      {!buckets.length && <Copy muted>{m.uso_vazio_secao()}</Copy>}{buckets.map(row)}
    </Card>;
  }

  function renderModels() {
    const buckets = [...view.groups.model].sort((a, b) => b.cost - a.cost || a.key.localeCompare(b.key));
    return <Card flat><Copy title>{m.custos_por_modelo()}</Copy><Copy muted>{m.custos_com_tarifa()}</Copy><Copy muted>{m.custos_col_entrada_ajuda()}</Copy>
      <Paged items={buckets} render={(bucket) => <Card flat>
        <Bar compact label={bucket.key} value={bucket.cost} max={Math.max(1, ...buckets.map((item) => item.cost))} detail={cost(bucket, bucket.key)} onPress={() => toggleFilter('model', bucket.key)} />
        <Copy>{`${m.custos_col_entrada()}: ${number(bucket.input + bucket.cache_write + bucket.cache_read)} / ${number(bucket.input)}${bucket.cache_write === 0 && bucket.cache_read > 0 ? '*' : ''}`}</Copy>
        <Copy>{`${m.custos_output_total()}: ${number(bucket.output)} · ${m.custos_tipo_cache_lido()}: ${number(bucket.cache_read)}`}</Copy>
        <Copy>{`${m.custos_col_pct_conta()}: ${custoDesconhecido(bucket) ? '—' : percent(bucket.cost, total.cost)}`}</Copy>
        {!!labelPrice(bucket, bucket.key) && <Copy muted>{labelPrice(bucket, bucket.key)}</Copy>}
        {precoParcial(bucket.key, rates.has(bucket.key), report.sem_tarifa) && <Copy>{m.custos_preco_parcial()}</Copy>}
        {!report.rates.some((rate) => rate.model === bucket.key) && <Copy muted>{`${m.custos_tarifa_in_out()}: — · ${m.custos_origem()}: —`}</Copy>}
        {report.rates.filter((rate) => rate.model === bucket.key).map((rate, index) => <Copy key={index} muted>{`${rate.provider} · ${m.custos_tarifa_in_out()}: ${money(rate.input)} / ${money(rate.output)} · ${m.custos_tipo_cache_escrito()}: ${money(rate.cache_write)} · ${m.custos_tipo_cache_lido()}: ${money(rate.cache_read)} · ${m.custos_origem()}: ${rate.origin}${rate.cache_estimado ? ` · ${m.custos_cache_estimado()}` : ''}`}</Copy>)}
      </Card>} />
      {buckets.some((bucket) => bucket.cache_write === 0 && bucket.cache_read > 0) && <Copy muted>{m.custos_sem_cache_escrito_ajuda()}</Copy>}
    </Card>;
  }

  function renderSessions() {
    const sessions = report.sessoes.filter((session) => dimensions.every((dim) => !filter[dim]?.length || filter[dim]!.includes(session[dim]))).sort((a, b) => b.cost - a.cost);
    return <Card flat><Copy title>{m.custos_sessoes_caras()}</Copy><Copy muted>{m.custos_sessoes_hint()}</Copy>
      {!report.sessoes.length && <Copy muted>{load.parts.some((p) => p.report && !Array.isArray(p.report.sessoes)) ? m.custos_sessoes_atualize() : m.custos_sessoes_vazio()}</Copy>}
      {sessions.slice(0, sessionsExpanded ? 30 : 10).map((session) => <Card flat key={`${session.servidor}:${session.session_id}`}><View style={{ flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'space-between', gap: SPACE[2] }}><Copy title>{session.project || session.session_id}</Copy><Copy>{cost(session, session.model)}</Copy></View><Copy>{`${session.source} · ${labelOf('provider', session.provider)} · ${session.model} · ${labelOf('servidor', session.servidor)}`}</Copy><Copy>{`${m.custos_sessao_tokens_novos({ tokens: number(session.input + session.output + session.cache_write) })} · ${m.custos_subagentes()}: ${number(session.subagentes)}`}</Copy>{session.custo_regravado > 0 && <Copy>{m.custos_sessao_cache_perdido({ valor: money(session.custo_regravado) })}</Copy>}<Copy muted>{`${session.session_id} · ${session.inicio} — ${session.fim}`}</Copy></Card>)}
      {sessions.length > 10 && <Action label={sessionsExpanded ? m.custos_mostrar_menos() : m.custos_ver_sessoes({ n: Math.min(30, sessions.length) })} onPress={() => setSessionsExpanded(!sessionsExpanded)} />}
    </Card>;
  }

  function renderComparison() {
    const comparisons = comparison(report, filter, compareDimension, compareKeys);
    const weekly = period === '90d' || period === 'all';
    const dateKey = (key: string) => {
      if (!weekly) return key;
      const date = new Date(`${key}T12:00:00Z`);
      date.setUTCDate(date.getUTCDate() - (date.getUTCDay() + 6) % 7);
      return date.toISOString().slice(0, 10);
    };
    const maps = comparisons.map((entity) => {
      const groups = new Map<string, DimBucket[]>();
      for (const day of entity.days) { const key = dateKey(day.key); groups.set(key, [...(groups.get(key) ?? []), day]); }
      return new Map([...groups].map(([key, buckets]) => [key, compareMetric === 'cost' && buckets.some(custoDesconhecido) ? null : buckets.reduce((sum, bucket) => sum + (compareMetric === 'tokens' ? rawTokens(bucket) : bucket.cost), 0)]));
    });
    const keys = [...new Set(maps.flatMap((map) => [...map.keys()]))].sort();
    const points = [];
    if (keys.length) for (let time = Date.parse(keys[0]); time <= Date.parse(keys.at(-1)!); time += (weekly ? 7 : 1) * 86_400_000) {
      const key = new Date(time).toISOString().slice(0, 10);
      points.push({ key, values: maps.map((map) => map.has(key) ? map.get(key)! : 0) });
    }
    const comparable = comparisons.reduce((sum, entity) => sum + (compareMetric === 'tokens' ? rawTokens(entity.total) : entity.total.cost), 0);
    const metricLabel = compareMetric === 'tokens' ? m.ctx_tokens() : m.custos_estimativa_api();
    return <Card flat><Copy title>{m.lista_comparar()}</Copy><Copy muted>{m.custos_comparar_hint({ n: 4 })}</Copy>
      <View style={{ flexDirection: 'row', flexWrap: 'wrap' }}>{dimensions.map((dim) => <Action key={dim} label={`${m.custos_comparar_por()} ${names[dim]}`} selected={compareDimension === dim} onPress={() => { setCompareDimension(dim); setCompareKeys([]); }} />)}</View>
      <View style={{ flexDirection: 'row', flexWrap: 'wrap' }}><Action label={m.ctx_tokens()} selected={compareMetric === 'tokens'} onPress={() => setCompareMetric('tokens')} /><Action label={m.custos_estimativa_api()} selected={compareMetric === 'cost'} onPress={() => setCompareMetric('cost')} /></View>
      <Choice label={m.custos_entidades_comparadas()} options={optionBuckets(compareDimension)} values={compareKeys} max={4} onChange={setCompareKeys} />
      {!view.cross && <Copy muted>{m.custos_sem_detalhamento()}</Copy>}
      {compareKeys.length < 2 && <Copy>{m.custos_marque_duas()}</Copy>}
      {comparisons.length >= 2 && <>
        <Grid>{comparisons.map((entity, index) => <Card key={entity.key}><Copy title>{labelOf(compareDimension, entity.key)}</Copy><View style={{ height: 3, backgroundColor: colors[index] }} /><Copy>{compareMetric === 'tokens' ? number(rawTokens(entity.total)) : cost(entity.total, compareDimension === 'model' ? entity.key : undefined)}</Copy>
          <Copy muted>{compareMetric === 'tokens' ? cost(entity.total, compareDimension === 'model' ? entity.key : undefined) : number(rawTokens(entity.total))}</Copy><Copy>{`${m.uso_col_sessoes()}: ${number(entity.total.sessions)}`}</Copy>
          <Copy muted>{compareMetric === 'cost' && custoDesconhecido(entity.total) ? '—' : m.custos_do_comparado({ pct: percent(compareMetric === 'tokens' ? rawTokens(entity.total) : entity.total.cost, comparable) })}</Copy>
          <Copy muted>{`${compareMetric === 'tokens' ? number(rawTokens(entity.total) / periodDays) : custoDesconhecido(entity.total) ? '—' : money(entity.total.cost / periodDays)} ${m.custos_por_dia()}`}</Copy>
        </Card>)}</Grid>
        {view.cross && <><Copy muted>{weekly ? m.custos_ponto_semana() : m.custos_ponto_dia()}</Copy><Columns fit lines label={m.custos_grafico_comp_aria({ metrica: metricLabel, modo: weekly ? m.custos_semana() : m.custos_dia() })} days={points} series={comparisons.map((entity, index) => ({ key: entity.key, label: labelOf(compareDimension, entity.key), color: colors[index] }))} format={compareMetric === 'tokens' ? number : money} axisFormat={compareMetric === 'tokens' ? compactNumber : moneyAxis} /></>}
      </>}
    </Card>;
  }

  const dayBucket = day ? view.days.find((item) => item.key === day) ?? { ...sumCombos([]), key: day } : undefined;
  return <ReportFrame compact title={m.nav_custos()} back={back} sibling={m.uso_painel_titulo()} onSibling={openStats}>
    <Copy muted>{`${periodLabel} · ${m.custos_somando_maquinas({ n: selected.length, m: own.length })}`}</Copy>
    <View style={{ flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', justifyContent: 'space-between', gap: SPACE[2] }}>
      <View><Choice label={m.custos_periodo()} triggerLabel={periodLabel} options={[['1d', m.custos_periodo_1d()], ['7d', m.custos_periodo_7d()], ['30d', m.custos_periodo_30d()], ['90d', m.custos_periodo_90d()], ['all', m.custos_periodo_tudo()]].map(([key, label]) => ({ key, label }))} values={[period]} max={1} closeOnSelect onChange={(keys) => { if (keys[0]) useUsagePreferences.setState({ period: keys[0] }); }} /></View>
      <View accessibilityLabel={m.custos_moeda()} style={{ flexDirection: 'row' }}><Action label="USD" selected={!brl || !validRate} onPress={() => setBrl(false)} /><Action label="BRL" disabled={!validRate} selected={brl && validRate} onPress={() => setBrl(true)} /><Action label={m.custos_atualizar()} icon="RefreshCw" iconOnly onPress={() => { load.refresh(); areaLoad.refresh(); }} /></View>
    </View>
    <Action label={m.mobile_costs_filters()} accessibilityLabel={m.custos_filtros()} icon={filtersOpen ? 'ChevronDown' : 'ListFilter'} selected={filtersOpen} onPress={() => setFiltersOpen(!filtersOpen)} />
    {filtersOpen && filters}
    {(activeFilter || selected.length < own.length) && <View style={{ gap: SPACE[1] }}>
      <Copy muted>{`${m.custos_recorte_inicio()}${[...dimensions.filter((dim) => filter[dim]?.length).map((dim) => `${names[dim]}: ${filter[dim]!.map((key) => labelOf(dim, key)).join(', ')}`), ...(filter.sub !== undefined ? [filter.sub ? m.custos_so_subagente() : m.custos_so_conversa()] : []), ...(selected.length < own.length ? [m.custos_somando_maquinas({ n: selected.length, m: own.length })] : [])].join(' · ')}`}</Copy>
      <Action label={m.custos_tirar_recorte()} icon="X" onPress={clearFilters} />
    </View>}
    <ReportStatus {...load} selected={selected.length} />
    {load.accepted && <>
      {missingPrices.length > 0 && <Copy muted>{`${m.custos_estimativa_incompleta({ n: missingPrices.length })} ${missingPrices.join(', ')}`}</Copy>}
      {!view.cross && <Copy muted>{m.custos_recorte_parcial()}</Copy>}
      <ScrollView horizontal><View style={{ flexDirection: 'row' }}><Action label={m.custos_todas_fontes()} selected={!filter.source?.length} onPress={() => setFilter(setCostFilter(filter, 'source', [], view.cross))} />{optionBuckets('source').map((source) => {
        const bucket = costView(report, { ...filter, source: [source.key] }).total;
        return <Action key={source.key} label={`${source.label} · ${number(bucket.input + bucket.output + bucket.cache_write)}`} selected={filter.source?.includes(source.key)} onPress={() => toggleFilter('source', source.key)} />;
      })}</View></ScrollView>
      <Grid>
        <Metric primary label={m.custos_custo_periodo()} value={cost(total)}><Copy muted>{`${sessionsText(total)} · ${unknownCost(total) ? '—' : money(total.cost / periodDays)} ${m.custos_por_dia()}`}</Copy>
          {unknownCost(total) && !emptySelection && <Copy muted>{m.custos_sem_tarifa_volume()}</Copy>}{freeOnly && <Copy muted>{m.custos_modelo_gratis_nada()}</Copy>}
          {view.cross && activeFilter && !view.rows.length && <Copy muted>{m.custos_recorte_vazio()}</Copy>}
          {subCost > 0 && total.cost > 0 && <Copy muted>{m.custos_veio_subagente({ pct: percent(subCost, total.cost) })}</Copy>}
          {delta !== null && <Copy muted>{Math.abs(delta) < 1 ? m.custos_delta_igual() : `${delta > 0 ? '▲' : '▼'} ${m.custos_delta_vs({ n: number(Math.abs(delta)), rot: periodLabel })}`}</Copy>}
          {selected.length < own.length && <Copy muted>{m.custos_somando_maquinas({ n: selected.length, m: own.length })}</Copy>}</Metric>
        <Metric primary label={m.custos_tokens_novos()} value={emptySelection ? '—' : compactNumber(newTokens)}>{!emptySelection && <Copy muted>{number(newTokens)}</Copy>}<Copy muted>{m.custos_tokens_novos_pe()}</Copy></Metric>
        <Metric label={m.custos_tokens_processados()} value={emptySelection ? '—' : compactNumber(raw)}>{!emptySelection && <Copy muted>{number(raw)}</Copy>}<Copy muted>{m.custos_tokens_processados_pe({ relidos: number(total.cache_read), pct: percent(total.cache_read, raw) })}</Copy></Metric>
        <Metric label={m.custos_cache_na_entrada()} value={emptySelection ? '—' : percent(total.cache_read, total.input + total.cache_write + total.cache_read)}>{total.cache_write > 0 && <Copy muted>{m.custos_cada_gravado_lido({ x: number(total.cache_read / total.cache_write, 1) })}</Copy>}</Metric>
        <Metric label={m.custos_economia_cache()} value={savingsKnown ? money(Math.max(0, noCache! - total.cost)) : '—'}><Copy muted>{!view.cross && activeFilter ? m.custos_so_total_periodo() : savingsKnown ? m.custos_abaixo_preco_cheio({ n: number(noCache! > 0 ? Math.max(0, 100 - total.cost / noCache! * 100) : 0) }) : m.custos_mesmos_tokens()}</Copy></Metric>
        <Metric label={m.custos_custo_por_milhao()} value={raw && !unknownCost(total) ? money(total.cost / raw * 1e6) : '—'}><Copy muted>{m.custos_por_milhao_novos({ valor: newTokens && !unknownCost(total) ? money(total.cost / newTokens * 1e6) : '—' })}</Copy></Metric>
        <Metric label={m.custos_cache_perdido()} value={report.cache_detalhado && !unknownCost(total) ? money(total.custo_regravado ?? 0) : '—'}><Copy muted>{report.cache_detalhado ? m.custos_cache_perdido_pe({ tokens: number(total.regravado ?? 0) }) : m.custos_cache_sem_dado()}</Copy></Metric>
        <Metric label={m.custos_cache_1h()} value={report.cache_detalhado && !emptySelection ? percent(total.cache_write_1h ?? 0, total.cache_write) : '—'}><Copy muted>{report.cache_detalhado ? m.custos_cache_1h_pe({ h1: number(total.cache_write_1h ?? 0), total: number(total.cache_write) }) : m.custos_cache_sem_dado()}</Copy></Metric>
      </Grid>
      {raw === 0 && <Copy>{activeFilter ? m.custos_sem_dados_recorte() : m.custos_sem_dados_periodo()}</Copy>}
      {renderDaily()}
      <Card flat><Copy title>{m.custos_para_onde_dolar()}</Copy><Copy muted>{m.custos_tipo_token_conta()}</Copy><Composition label={m.custos_para_onde_dolar()} items={costParts(total)} />{renderTypes(total)}{!!labelPrice(total) && <Copy muted>{labelPrice(total)}</Copy>}</Card>
      {(() => {
        const cacheTotal = view.cross ? total : report.totals;
        const without = view.cross ? noCache : report.custo_sem_cache;
        const known = without !== null && without > 0 && !custoDesconhecido(cacheTotal);
        const saved = known ? Math.max(0, without - cacheTotal.cost) : 0;
        return <Card flat><Copy title>{m.custos_o_que_cache()}</Copy><Copy muted>{m.custos_mesmos_tokens()}</Copy>{!view.cross && activeFilter && <Copy muted>{m.custos_ressalva_cache()}</Copy>}
          <Copy>{`${m.custos_pago_de_verdade()}: ${cost(cacheTotal)}`}</Copy>
          {known && <Composition label={m.custos_o_que_cache()} items={[{ label: m.custos_pago_de_verdade(), value: cacheTotal.cost, color: colors[0], detail: money(cacheTotal.cost) }, { label: m.custos_economizado(), value: saved, color: colors[2], detail: money(saved) }]} />}
          <Copy>{`${m.custos_se_nada_cache()}: ${known ? money(without) : '—'}`}</Copy><Copy>{`${m.custos_economizado()}: ${known ? money(saved) : '—'}`}</Copy><Copy>{`${percent(cacheTotal.cache_read, rawTokens(cacheTotal))} ${m.custos_do_volume_cache_lido()}`}</Copy>
        </Card>;
      })()}
      {renderRanking('provider')}{renderRanking('source')}
      {selected.length > 1 && renderRanking('servidor')}
      <Card flat>
        <Copy title>{m.uso_graf_areas()}</Copy><Copy muted>{m.uso_graf_areas_nota()}</Copy><Copy muted>{m.mobile_costs_area_scope()}</Copy>
        <ReportStatus {...areaLoad} selected={areaServers.length} />
        {areaLoad.accepted && <><Composition label={m.uso_graf_areas()} items={orderedAreas.map((item, index) => ({ label: item.label ?? item.key, value: rawTokens(item), color: colors[index % colors.length] }))} /><Copy muted>{`${m.uso_col_nome()} · ${m.ctx_tokens()} · ${m.custos_custo()} · ${m.custos_areas_pct()}`}</Copy><Paged items={orderedAreas} render={(item) => <Bar label={item.label ?? item.key} value={rawTokens(item)} max={areaTokens} detail={`${number(rawTokens(item))} · ${cost(item)} · ${percent(rawTokens(item), areaTokens)}`} />} /></>}
      </Card>
    </>}
    <ProjectRanking visible={load.accepted} buckets={view.groups.project} cost={cost} labelPrice={labelPrice} money={money} onSelect={(key) => toggleFilter('project', key)} />
    {load.accepted && <>
      {renderSessions()}{renderModels()}{renderComparison()}
      <Action label={m.custos_como_calculamos()} selected={methodOpen} icon={methodOpen ? 'ChevronDown' : 'ChevronRight'} onPress={() => setMethodOpen(!methodOpen)} />
      {methodOpen && <Card><Copy>{m.custos_aviso_estimativa()}</Copy><Copy>{m.custos_preco_tabela()}</Copy><Copy>{m.custos_metodo_standard()}</Copy><Copy>{m.custos_metodo_codex()}</Copy><Copy>{`${m.custos_equivalente_cobrado()}: ${equivalent === null || unknownCost(total) ? '—' : number(equivalent)}`}</Copy><Copy>{m.custos_equivalente_metodo()}</Copy>
        <Copy>{`${m.custos_fontes()} ${m.custos_claude_code()} · Codex · Pi · omp · Kimi`}</Copy><Copy>{m.custos_tarifas()}{m.custos_tarifas_models()}</Copy>
        <Copy>{`${m.custos_cotacao()}: ${validRate ? m.custos_usd_brl({ taxa: number(report.usd_brl!, 4) }) : m.custos_cotacao_indisponivel()}`}</Copy>
        <Copy>{m.custos_custo_tabela()}{m.custos_nao_fatura()}{m.custos_assinatura()}</Copy>
        {report.sem_tarifa.some(isFree) && <Copy>{`${m.custos_modelos_gratis()}: ${report.sem_tarifa.filter(isFree).join(', ')} · ${m.custos_modelo_gratis_nenhum_dolar()}`}</Copy>}
        {report.sem_tarifa.some((model) => !isFree(model)) && <Copy>{`${m.custos_modelos_sem_tarifa()}: ${m.custos_aparecem_traco({ lista: report.sem_tarifa.filter((model) => !isFree(model)).join(', ') })}`}</Copy>}
      </Card>}
      {day !== null && <Sheet open onDismiss={() => setDay(null)} sizes={['large']} scrollable><ScrollView contentContainerStyle={{ padding: SPACE[4], gap: SPACE[3] }}><Copy title>{day}</Copy>{dayBucket && <><Copy>{`${number(rawTokens(dayBucket))} · ${cost(dayBucket)}`}</Copy><Copy>{sessionsText(dayBucket, view.rows.filter((row) => row.dia === day))}</Copy><Copy muted>{m.custos_dia_reuso({ pct: percent(dayBucket.cache_read, dayBucket.input + dayBucket.cache_write + dayBucket.cache_read) })}</Copy>{renderTypes(dayBucket)}</>}<Action label={m.native_close()} onPress={() => setDay(null)} /></ScrollView></Sheet>}
    </>}
  </ReportFrame>;
}
